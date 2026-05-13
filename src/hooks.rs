use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::codex;
use crate::config::{self, AppConfig};
use crate::maintenance;
use crate::recovery::{RecoveryDecision, RecoveryKind, classify_error, sanitized_recovery_text};
use crate::telegram;

const SENTINEL_MARKER: &str = "codex-sentinel";
const STOP_TIMEOUT_SECONDS: u64 = 240;
const STOP_LOG_LOOKBACK_SECONDS: i64 = 300;
const STOP_LOG_LOOKBACK_WITH_TURN_SECONDS: i64 = 1800;
const STOP_HOOK_MAX_INLINE_DELAY_SECONDS: u64 = 10;
const STOP_HOOK_DEDUP_WINDOW_SECONDS: i64 = 300;
const STOP_HOOK_COMPLETION_DEDUP_WINDOW_SECONDS: i64 = 300;
const RECENT_HOOK_EVENT_LIMIT: usize = 5;
const HOOK_EVENTS_TAIL_SCAN_BYTES: u64 = 512 * 1024;
const INSTALLED_APP_EXECUTABLE: &str =
    "/Applications/Codex Sentinel.app/Contents/MacOS/codex-sentinel";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStatus {
    pub feature_enabled: bool,
    pub config_path: String,
    pub hooks_path: String,
    pub hooks_file_exists: bool,
    pub stop_installed: bool,
    pub installed_app_command: bool,
    pub current_executable: String,
    pub installed_commands: Vec<String>,
    pub recent_events: Vec<HookEventSummary>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInstallResult {
    pub status: HookStatus,
    pub changed_files: Vec<String>,
    pub backup_files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct HookInput {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    turn_id: Option<String>,
    stop_hook_active: Option<bool>,
    last_assistant_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventSummary {
    pub ts: DateTime<Utc>,
    pub event: Option<String>,
    pub action: String,
    pub event_key: String,
    pub source: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub delay_seconds: u64,
    pub decision_label: String,
    pub decision_kind: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HookEventLine {
    ts: DateTime<Utc>,
    event: Option<String>,
    action: String,
    event_key: String,
    source: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    delay_seconds: Option<u64>,
    decision: Option<RecoveryDecision>,
    body: Option<String>,
    latest_feedback: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct HookCooldownLine {
    ts: DateTime<Utc>,
    event_key: String,
}

#[derive(Debug, Clone)]
struct StopClassification {
    decision: RecoveryDecision,
    source: String,
    body: String,
    latest_feedback: Option<HookFeedbackSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HookFeedbackSnapshot {
    thread_id: String,
    title: String,
    timestamp: Option<String>,
    text: String,
}

pub fn codex_config_path() -> PathBuf {
    codex::codex_home().join("config.toml")
}

pub fn hooks_path() -> PathBuf {
    codex::codex_home().join("hooks.json")
}

pub fn inspect_hooks() -> Result<HookStatus> {
    let current_executable = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let config_path = codex_config_path();
    let hooks_path = hooks_path();
    let feature_enabled = read_codex_hooks_feature(&config_path).unwrap_or(false);
    let hooks_file_exists = hooks_path.exists();
    let mut installed_commands = Vec::new();
    let mut stop_installed = false;
    let mut installed_app_command = false;

    if hooks_file_exists {
        let raw = fs::read_to_string(&hooks_path)
            .with_context(|| format!("failed to read {}", hooks_path.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", hooks_path.display()))?;
        installed_commands = collect_sentinel_commands(&value);
        stop_installed = event_has_sentinel_command(&value, "Stop");
        installed_app_command = installed_app_command_present(&installed_commands);
    }

    let mut notes = Vec::new();
    if !feature_enabled {
        notes.push(
            "Codex hooks 功能未启用，请在 ~/.codex/config.toml 开启 features.hooks。".to_string(),
        );
    }
    if !stop_installed {
        notes.push("Stop hook 未安装，Codex Sentinel 无法在任务停止事件里自动续跑。".to_string());
    }
    if stop_installed && !installed_app_command {
        notes.push(format!(
            "Stop hook 当前没有指向已安装包：{INSTALLED_APP_EXECUTABLE}。请从 /Applications 安装或修复 Hook。"
        ));
    }
    Ok(HookStatus {
        feature_enabled,
        config_path: config_path.display().to_string(),
        hooks_path: hooks_path.display().to_string(),
        hooks_file_exists,
        stop_installed,
        installed_app_command,
        current_executable,
        installed_commands,
        recent_events: recent_hook_events(RECENT_HOOK_EVENT_LIMIT).unwrap_or_default(),
        notes,
    })
}

pub fn install_hooks() -> Result<HookInstallResult> {
    let codex_home = codex::codex_home();
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("failed to create {}", codex_home.display()))?;

    let mut changed_files = Vec::new();
    let mut backup_files = Vec::new();
    let config_path = codex_config_path();
    let hooks_path = hooks_path();

    if enable_codex_hooks_feature(&config_path, &mut backup_files)? {
        changed_files.push(config_path.display().to_string());
    }

    let mut hooks_json = if hooks_path.exists() {
        let raw = fs::read_to_string(&hooks_path)
            .with_context(|| format!("failed to read {}", hooks_path.display()))?;
        serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("failed to parse {}", hooks_path.display()))?
    } else {
        json!({ "hooks": {} })
    };

    ensure_hooks_shape(&mut hooks_json)?;
    remove_existing_sentinel_hooks(&mut hooks_json);
    add_stop_hook(&mut hooks_json)?;

    let next = serde_json::to_string_pretty(&hooks_json)?;
    if write_if_changed(&hooks_path, &next, &mut backup_files)? {
        changed_files.push(hooks_path.display().to_string());
    }

    Ok(HookInstallResult {
        status: inspect_hooks()?,
        changed_files,
        backup_files,
    })
}

pub fn run_stop_hook_from_stdin() -> Result<()> {
    let input = read_hook_input()?;
    let cfg = config::load_or_create().unwrap_or_default();
    let collect_latest_feedback = should_collect_latest_feedback(&cfg);
    let classification = classify_stop_input_with_feedback(&input, collect_latest_feedback);
    let body_hash = stable_hash_hex(&[&classification.body]);
    let event_key = stop_event_key(&input, &classification, &body_hash);
    let duplicate = recent_key_seen(&event_key, STOP_HOOK_DEDUP_WINDOW_SECONDS).unwrap_or(false);
    let response = stop_hook_response(
        &input,
        &cfg,
        &classification.decision,
        should_sleep_in_hook(),
        duplicate,
    )?;
    let action = stop_hook_action(&input, &cfg, &classification, duplicate);
    let completion_duplicate = if action == "completed" {
        recent_key_seen(&event_key, STOP_HOOK_COMPLETION_DEDUP_WINDOW_SECONDS).unwrap_or(false)
    } else {
        false
    };

    if should_log_stop_event(&cfg, &classification, action) {
        if let Err(err) = log_stop_hook_event(
            &cfg,
            &input,
            &classification,
            action,
            &body_hash,
            &event_key,
        ) {
            tracing::warn!("failed to write Stop hook event: {err:#}");
        }
    }
    if should_write_cooldown_key(&classification, action) {
        if let Err(err) = append_hook_cooldown_key(&cfg, &event_key) {
            tracing::warn!("failed to write Stop hook cooldown key: {err:#}");
        }
    }
    if action == "completed"
        && cfg.observability.completion_notifications_enabled
        && !completion_duplicate
    {
        if let Err(err) = spawn_completion_notification(&input, &classification) {
            tracing::warn!("failed to spawn completion notification: {err:#}");
        }
    }
    print_json(&response)?;
    Ok(())
}

pub async fn run_notify_completion_from_stdin() -> Result<()> {
    let notification = read_completion_notification()?;
    let cfg = config::load_or_create().unwrap_or_default();
    if !should_notify_completion(&cfg, &notification) {
        tracing::debug!(
            session_id = ?notification.session_id,
            turn_id = ?notification.turn_id,
            "completion notification skipped because no Codex thread feedback snapshot is available"
        );
        return Ok(());
    }
    let text = format_completion_notification(&notification);
    telegram::notify_configured_text_with_keyboard(
        &cfg,
        &text,
        completion_notification_keyboard(&notification),
    )
    .await
}

#[cfg(test)]
fn classify_stop_input(input: &HookInput) -> StopClassification {
    classify_stop_input_with_feedback(input, true)
}

fn classify_stop_input_with_feedback(
    input: &HookInput,
    collect_latest_feedback: bool,
) -> StopClassification {
    let message = input.last_assistant_message.as_deref().unwrap_or_default();
    let decision = classify_stop_assistant_text(message);
    if decision.kind != RecoveryKind::None {
        return stop_classification_with_feedback(
            input,
            decision,
            "last_assistant_message",
            message,
            collect_latest_feedback,
        );
    }
    if !message.trim().is_empty() {
        return stop_classification_with_feedback(
            input,
            decision,
            "last_assistant_message",
            message,
            collect_latest_feedback,
        );
    }

    let transcript_message = latest_transcript_message(input);
    let prefer_log_fallback = should_prefer_log_fallback(input);
    if prefer_log_fallback {
        if let Some(classification) =
            classify_stop_from_log(input, &decision, collect_latest_feedback)
        {
            return classification;
        }
    }

    if let Some(transcript_message) = transcript_message.as_deref() {
        let transcript_decision = classify_stop_assistant_text(transcript_message);
        if transcript_decision.kind != RecoveryKind::None {
            return stop_classification_with_feedback(
                input,
                transcript_decision,
                "transcript_path",
                transcript_message,
                collect_latest_feedback,
            );
        }
    }

    if !prefer_log_fallback {
        if let Some(classification) =
            classify_stop_from_log(input, &decision, collect_latest_feedback)
        {
            return classification;
        }
    }

    if let Some(transcript_message) = transcript_message {
        return stop_classification_with_feedback(
            input,
            decision,
            "transcript_path",
            &transcript_message,
            collect_latest_feedback,
        );
    }

    stop_classification_with_feedback(input, decision, "empty", message, collect_latest_feedback)
}

fn classify_stop_assistant_text(text: &str) -> RecoveryDecision {
    let decision = classify_error(text);
    if decision.label == "Silent turn completion" {
        return RecoveryDecision::none();
    }
    decision
}

fn classify_stop_from_log(
    input: &HookInput,
    base_decision: &RecoveryDecision,
    collect_latest_feedback: bool,
) -> Option<StopClassification> {
    match codex::latest_recovery_log_for_hook(
        input.session_id.as_deref(),
        input.turn_id.as_deref(),
        stop_log_lookback_seconds(input),
    ) {
        Ok(Some(event)) => {
            let fallback_decision = classify_error(&event.body);
            if fallback_decision.kind != RecoveryKind::None {
                return Some(stop_classification_with_feedback(
                    input,
                    fallback_decision,
                    "codex_log_fallback",
                    &event.body,
                    collect_latest_feedback,
                ));
            }
        }
        Ok(None) => {}
        Err(err) => {
            let source = format!("Stop hook log fallback failed: {err:#}");
            return Some(stop_classification_with_feedback(
                input,
                base_decision.clone(),
                "log_lookup_error",
                &source,
                collect_latest_feedback,
            ));
        }
    }
    None
}

fn should_prefer_log_fallback(input: &HookInput) -> bool {
    input
        .turn_id
        .as_deref()
        .is_some_and(|turn_id| !turn_id.trim().is_empty())
}

#[cfg(test)]
fn stop_classification(
    input: &HookInput,
    decision: RecoveryDecision,
    source: &str,
    body: &str,
) -> StopClassification {
    stop_classification_with_feedback(input, decision, source, body, true)
}

fn stop_classification_with_feedback(
    input: &HookInput,
    decision: RecoveryDecision,
    source: &str,
    body: &str,
    collect_latest_feedback: bool,
) -> StopClassification {
    let latest_feedback = if collect_latest_feedback
        && (decision.kind != RecoveryKind::None || !body.trim().is_empty())
    {
        latest_feedback_snapshot(input)
    } else {
        None
    };
    StopClassification {
        decision,
        source: source.to_string(),
        body: body.to_string(),
        latest_feedback,
    }
}

fn should_collect_latest_feedback(cfg: &AppConfig) -> bool {
    cfg.observability.latest_feedback_enabled
        || cfg.observability.completion_notifications_enabled
        || cfg.observability.record_normal_completion_events
}

fn latest_transcript_message(input: &HookInput) -> Option<String> {
    let path = input.transcript_path.as_deref()?.trim();
    if path.is_empty() {
        return None;
    }
    match codex::latest_assistant_text_from_rollout_path(Path::new(path)) {
        Ok(Some((_, text))) => Some(text),
        Ok(None) => None,
        Err(err) => {
            tracing::debug!("failed to read transcript fallback {path}: {err:#}");
            None
        }
    }
}

fn latest_feedback_snapshot(input: &HookInput) -> Option<HookFeedbackSnapshot> {
    let feedback = codex::latest_thread_feedback(input.session_id.as_deref()?).ok()?;
    Some(HookFeedbackSnapshot {
        thread_id: feedback.thread_id,
        title: feedback.title,
        timestamp: feedback.timestamp,
        text: truncate(&feedback.text, 1200),
    })
}

fn stop_log_lookback_seconds(input: &HookInput) -> i64 {
    if input.turn_id.is_some() {
        STOP_LOG_LOOKBACK_WITH_TURN_SECONDS
    } else {
        STOP_LOG_LOOKBACK_SECONDS
    }
}

fn stop_hook_response(
    input: &HookInput,
    cfg: &AppConfig,
    decision: &RecoveryDecision,
    allow_sleep: bool,
    duplicate: bool,
) -> Result<Value> {
    if input.stop_hook_active.unwrap_or(false) {
        return Ok(json!({
            "continue": false,
            "stopReason": "Codex Sentinel 已处理过本次 Stop 事件，已阻止重复续跑循环。"
        }));
    }

    if decision.kind == RecoveryKind::None {
        return Ok(json!({ "continue": true }));
    }

    if duplicate {
        return Ok(json!({
            "continue": false,
            "systemMessage": format!("Codex Sentinel 已在最近 {}s 内处理过同一个 Stop 事件，本次交给 watcher、Telegram 或桌面面板继续观察。", STOP_HOOK_DEDUP_WINDOW_SECONDS)
        }));
    }

    if !cfg.recovery.auto_recover || !decision.auto_allowed {
        return Ok(json!({
            "continue": false,
            "systemMessage": format!("Codex Sentinel 检测到「{}」，但自动恢复已关闭或此类问题需要人工处理：{}", decision.label, decision.reason)
        }));
    }

    if decision.delay_seconds > 0 {
        if decision.delay_seconds > STOP_HOOK_MAX_INLINE_DELAY_SECONDS {
            return Ok(json!({
                "continue": false,
                "systemMessage": format!("Codex Sentinel 检测到「{}」，但退避为 {}s；Stop hook 不在行内长时间等待，已交给 watcher、Telegram 或桌面面板稍后恢复。", decision.label, decision.delay_seconds)
            }));
        }
        if allow_sleep {
            std::thread::sleep(Duration::from_secs(decision.delay_seconds));
        }
    }

    Ok(json!({
        "decision": "block",
        "reason": continuation_prompt(&cfg, &decision)
    }))
}

fn read_hook_input() -> Result<HookInput> {
    let raw = read_stdin_string()?;
    serde_json::from_str(&raw).with_context(|| "failed to parse Codex hook JSON from stdin")
}

fn read_stdin_string() -> Result<String> {
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    Ok(raw)
}

fn continuation_prompt(cfg: &AppConfig, decision: &RecoveryDecision) -> String {
    let prompt = match decision.kind {
        RecoveryKind::ToolRetryWithDifferentPath => &cfg.recovery.tool_failure_prompt,
        RecoveryKind::SafetyRephrase => &cfg.recovery.safety_rephrase_prompt,
        _ => &cfg.recovery.continue_prompt,
    };
    let combined = format!(
        "{}\n\nCodex Sentinel 自动恢复原因：{}。{}",
        prompt, decision.label, decision.reason
    );
    if decision.kind == RecoveryKind::SafetyRephrase {
        sanitized_recovery_text(&combined)
    } else {
        combined
    }
}

fn should_sleep_in_hook() -> bool {
    std::env::var_os("CODEX_SENTINEL_SKIP_HOOK_SLEEP").is_none()
}

fn read_codex_hooks_feature(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&raw)?;
    let features = value.get("features").and_then(toml::Value::as_table);
    Ok(features
        .and_then(|table| {
            table
                .get("hooks")
                .or_else(|| table.get("codex_hooks"))
                .and_then(toml::Value::as_bool)
        })
        .unwrap_or(false))
}

fn enable_codex_hooks_feature(path: &Path, backup_files: &mut Vec<String>) -> Result<bool> {
    let mut value = if path.exists() {
        let raw = fs::read_to_string(path)?;
        toml::from_str::<toml::Value>(&raw)?
    } else {
        toml::Value::Table(Default::default())
    };

    let table = value
        .as_table_mut()
        .ok_or_else(|| anyhow!("{} is not a TOML table", path.display()))?;
    let features = table
        .entry("features".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let features_table = features
        .as_table_mut()
        .ok_or_else(|| anyhow!("[features] in {} is not a table", path.display()))?;

    let hooks_enabled = features_table
        .get("hooks")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let legacy_enabled = features_table
        .get("codex_hooks")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);

    if hooks_enabled && !features_table.contains_key("codex_hooks") {
        return Ok(false);
    }

    features_table.insert("hooks".to_string(), toml::Value::Boolean(true));
    if legacy_enabled {
        features_table.remove("codex_hooks");
    }
    let next = toml::to_string_pretty(&value)?;
    write_if_changed(path, &next, backup_files)
}

fn ensure_hooks_shape(value: &mut Value) -> Result<()> {
    if !value.is_object() {
        return Err(anyhow!("hooks.json root must be an object"));
    }
    if value.get("hooks").is_none() {
        value["hooks"] = json!({});
    }
    if !value["hooks"].is_object() {
        return Err(anyhow!("hooks.json field `hooks` must be an object"));
    }
    Ok(())
}

fn add_stop_hook(value: &mut Value) -> Result<()> {
    add_hook_group(
        value,
        "Stop",
        None,
        command_for("hook-stop")?,
        "Codex Sentinel 正在判断是否需要继续",
        STOP_TIMEOUT_SECONDS,
    )
}

fn add_hook_group(
    value: &mut Value,
    event: &str,
    matcher: Option<&str>,
    command: String,
    status_message: &str,
    timeout: u64,
) -> Result<()> {
    let hooks = value
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("hooks.json field `hooks` must be an object"))?;
    let event_groups = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    let event_groups = event_groups
        .as_array_mut()
        .ok_or_else(|| anyhow!("hooks.{event} must be an array"))?;

    let mut group = json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
            "statusMessage": status_message
        }]
    });
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    event_groups.push(group);
    Ok(())
}

fn command_for(subcommand: &str) -> Result<String> {
    let exe = std::env::current_exe()?.display().to_string();
    Ok(format!("\"{}\" {subcommand}", exe.replace('"', "\\\"")))
}

fn remove_existing_sentinel_hooks(value: &mut Value) {
    let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(items) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            items.retain(|hook| !hook_command(hook).contains(SENTINEL_MARKER));
        }
        groups.retain(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_some_and(|groups| !groups.is_empty()));
}

fn collect_sentinel_commands(value: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    let Some(hooks) = value.get("hooks").and_then(Value::as_object) else {
        return commands;
    };
    for groups in hooks.values() {
        let Some(groups) = groups.as_array() else {
            continue;
        };
        for group in groups {
            let Some(items) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in items {
                let command = hook_command(hook);
                if command.contains(SENTINEL_MARKER) {
                    commands.push(command);
                }
            }
        }
    }
    commands.sort();
    commands.dedup();
    commands
}

fn event_has_sentinel_command(value: &Value, event: &str) -> bool {
    value
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|hook| hook_command(hook).contains(SENTINEL_MARKER))
                    })
            })
        })
}

fn hook_command(hook: &Value) -> String {
    hook.get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn write_if_changed(path: &Path, next: &str, backup_files: &mut Vec<String>) -> Result<bool> {
    if path.exists() {
        let current = fs::read_to_string(path)?;
        if current == next {
            return Ok(false);
        }
        let backup = backup_path(path);
        fs::copy(path, &backup)?;
        backup_files.push(backup.display().to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, next)?;
    Ok(true)
}

fn backup_path(path: &Path) -> PathBuf {
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("codex-sentinel-backup");
    path.with_file_name(format!("{name}.bak-{ts}"))
}

fn stop_hook_action(
    input: &HookInput,
    cfg: &AppConfig,
    classification: &StopClassification,
    duplicate: bool,
) -> &'static str {
    let decision = &classification.decision;
    if input.stop_hook_active.unwrap_or(false) {
        "loop_prevented"
    } else if decision.kind == RecoveryKind::None {
        if is_normal_completion(classification) {
            "completed"
        } else {
            "none"
        }
    } else if duplicate {
        "deduped"
    } else if !cfg.recovery.auto_recover || !decision.auto_allowed {
        match decision.kind {
            RecoveryKind::Reauth => "manual_reauth",
            _ => "manual",
        }
    } else if decision.delay_seconds > STOP_HOOK_MAX_INLINE_DELAY_SECONDS {
        "deferred"
    } else {
        "continue"
    }
}

fn is_normal_completion(classification: &StopClassification) -> bool {
    classification.decision.kind == RecoveryKind::None
        && classification.latest_feedback.is_some()
        && !classification.body.trim().is_empty()
        && matches!(
            classification.source.as_str(),
            "last_assistant_message" | "transcript_path"
        )
        && !is_machine_control_payload(&classification.body)
}

fn is_machine_control_payload(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return false;
    }

    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };

    object
        .keys()
        .all(|key| matches!(key.as_str(), "suggestions" | "exclude"))
        && object
            .keys()
            .any(|key| matches!(key.as_str(), "suggestions" | "exclude"))
}

fn should_log_stop_event(
    cfg: &AppConfig,
    classification: &StopClassification,
    action: &str,
) -> bool {
    if action == "completed" {
        return cfg.observability.record_normal_completion_events;
    }
    classification.decision.kind != RecoveryKind::None
        || action != "none"
        || classification.source == "log_lookup_error"
}

fn log_stop_hook_event(
    cfg: &AppConfig,
    input: &HookInput,
    classification: &StopClassification,
    action: &str,
    body_hash: &str,
    event_key: &str,
) -> Result<()> {
    let line = json!({
        "ts": Utc::now(),
        "event": &input.hook_event_name,
        "action": action,
        "event_key": event_key,
        "source": &classification.source,
        "session_id": &input.session_id,
        "turn_id": &input.turn_id,
        "cwd": &input.cwd,
        "model": &input.model,
        "transcript_path": &input.transcript_path,
        "stop_hook_active": input.stop_hook_active.unwrap_or(false),
        "delay_seconds": classification.decision.delay_seconds,
        "decision": &classification.decision,
        "body_hash": body_hash,
        "body": truncate(&classification.body, 1600),
        "latest_feedback": &classification.latest_feedback
    });
    append_hook_event(&line, cfg.observability.hook_event_max_lines)
}

fn should_write_cooldown_key(classification: &StopClassification, action: &str) -> bool {
    action != "none"
        || classification.decision.kind != RecoveryKind::None
        || classification.source == "log_lookup_error"
}

fn stop_event_key(
    input: &HookInput,
    classification: &StopClassification,
    body_hash: &str,
) -> String {
    format!(
        "{}:{}:{:?}:{}",
        event_key_part(input.session_id.as_deref()),
        event_key_part(input.turn_id.as_deref()),
        classification.decision.kind,
        body_hash
    )
}

fn event_key_part(value: Option<&str>) -> &str {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
}

fn stable_hash_hex(parts: &[&str]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for part in parts {
        for byte in part.as_bytes().iter().copied().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    format!("{hash:016x}")
}

fn append_hook_event(line: &Value, max_lines: usize) -> Result<()> {
    let dir = config::config_dir();
    fs::create_dir_all(&dir)?;
    let path = hook_events_path();
    let mut raw = serde_json::to_string(&line)?;
    raw.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(raw.as_bytes())?;
    maintenance::trim_jsonl_file(&path, max_lines)?;
    Ok(())
}

fn hook_events_path() -> PathBuf {
    config::config_dir().join("hook-events.jsonl")
}

fn hook_cooldowns_path() -> PathBuf {
    config::config_dir().join("hook-cooldowns.jsonl")
}

fn append_hook_cooldown_key(cfg: &AppConfig, event_key: &str) -> Result<()> {
    let dir = config::config_dir();
    fs::create_dir_all(&dir)?;
    let path = hook_cooldowns_path();
    let line = json!({
        "ts": Utc::now(),
        "event_key": event_key
    });
    let mut raw = serde_json::to_string(&line)?;
    raw.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(raw.as_bytes())?;
    maintenance::trim_jsonl_file(&path, cfg.observability.hook_cooldown_max_lines)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletionNotification {
    event: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    source: String,
    body: String,
    latest_feedback: Option<HookFeedbackSnapshot>,
}

fn spawn_completion_notification(
    input: &HookInput,
    classification: &StopClassification,
) -> Result<()> {
    let payload = CompletionNotification {
        event: input.hook_event_name.clone(),
        session_id: input.session_id.clone(),
        turn_id: input.turn_id.clone(),
        cwd: input.cwd.clone(),
        model: input.model.clone(),
        source: classification.source.clone(),
        body: truncate(&classification.body, 1600),
        latest_feedback: classification.latest_feedback.clone(),
    };
    let raw = serde_json::to_vec(&payload)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("notify-completion")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn completion notification helper")?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(anyhow!(
            "completion notification helper stdin is unavailable"
        ));
    };
    stdin.write_all(&raw)?;
    drop(stdin);
    Ok(())
}

fn read_completion_notification() -> Result<CompletionNotification> {
    let raw = read_stdin_string()?;
    serde_json::from_str(&raw).context("failed to parse completion notification JSON from stdin")
}

fn should_notify_completion(cfg: &AppConfig, notification: &CompletionNotification) -> bool {
    cfg.observability.completion_notifications_enabled && notification.latest_feedback.is_some()
}

fn format_completion_notification(notification: &CompletionNotification) -> String {
    let title = notification
        .latest_feedback
        .as_ref()
        .map(|feedback| feedback.title.as_str())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("未命名线程");
    let thread_id = notification
        .latest_feedback
        .as_ref()
        .map(|feedback| feedback.thread_id.as_str())
        .or(notification.session_id.as_deref())
        .unwrap_or("-");
    let feedback_text = notification
        .latest_feedback
        .as_ref()
        .map(|feedback| feedback.text.as_str())
        .unwrap_or(notification.body.as_str());

    format!(
        "线程正常完成\n{}\n{}\n\n最后反馈：\n{}",
        title,
        thread_id,
        truncate(feedback_text, 2600)
    )
}

fn completion_notification_keyboard(notification: &CompletionNotification) -> Option<Value> {
    let thread_id = completion_notification_thread_id(notification)?;
    Some(json!({
        "inline_keyboard": [[{
            "text": "追加指令",
            "callback_data": format!("ask:{thread_id}")
        }]]
    }))
}

fn completion_notification_thread_id(notification: &CompletionNotification) -> Option<&str> {
    notification
        .latest_feedback
        .as_ref()
        .map(|feedback| feedback.thread_id.as_str())
        .or(notification.session_id.as_deref())
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty() && *thread_id != "-")
}

pub fn recent_hook_events(limit: usize) -> Result<Vec<HookEventSummary>> {
    let path = hook_events_path();
    recent_hook_events_from_path(&path, limit)
}

fn recent_hook_events_from_path(path: &Path, limit: usize) -> Result<Vec<HookEventSummary>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    for line in read_tail_lines(path, HOOK_EVENTS_TAIL_SCAN_BYTES)? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<HookEventLine>(trimmed) {
            if is_stale_completion_event_without_feedback(&event) {
                continue;
            }
            events.push(event);
        }
    }
    events.sort_by_key(|event| event.ts);
    events.reverse();
    Ok(events
        .into_iter()
        .take(limit)
        .map(HookEventSummary::from)
        .collect())
}

fn is_stale_completion_event_without_feedback(event: &HookEventLine) -> bool {
    event.action == "completed" && event.latest_feedback.is_none()
}

fn recent_key_seen(key: &str, max_age_seconds: i64) -> Result<bool> {
    let now = Utc::now();
    if recent_cooldown_key_seen(key, max_age_seconds, now)? {
        return Ok(true);
    }
    Ok(
        recent_event_with_key_from_events(recent_hook_events(64)?, key, max_age_seconds, now)?
            .is_some(),
    )
}

fn recent_cooldown_key_seen(key: &str, max_age_seconds: i64, now: DateTime<Utc>) -> Result<bool> {
    let path = hook_cooldowns_path();
    recent_cooldown_key_seen_from_path(&path, key, max_age_seconds, now)
}

fn recent_cooldown_key_seen_from_path(
    path: &Path,
    key: &str,
    max_age_seconds: i64,
    now: DateTime<Utc>,
) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    for line in read_tail_lines(path, HOOK_EVENTS_TAIL_SCAN_BYTES)?
        .into_iter()
        .rev()
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<HookCooldownLine>(trimmed) else {
            continue;
        };
        let age = now.signed_duration_since(event.ts).num_seconds();
        if age > max_age_seconds {
            break;
        }
        if event.event_key == key {
            return Ok(true);
        }
    }
    Ok(false)
}

fn recent_event_with_key_from_events(
    events: Vec<HookEventSummary>,
    key: &str,
    max_age_seconds: i64,
    now: DateTime<Utc>,
) -> Result<Option<HookEventSummary>> {
    Ok(events.into_iter().find(|event| {
        event.event_key == key
            && now.signed_duration_since(event.ts).num_seconds() <= max_age_seconds
    }))
}

impl From<HookEventLine> for HookEventSummary {
    fn from(event: HookEventLine) -> Self {
        let decision_label = event
            .decision
            .as_ref()
            .map(|decision| decision.label.clone())
            .unwrap_or_else(|| "-".to_string());
        let decision_kind = event
            .decision
            .as_ref()
            .map(|decision| format!("{:?}", decision.kind))
            .unwrap_or_else(|| "-".to_string());
        Self {
            ts: event.ts,
            event: event.event,
            action: event.action,
            event_key: event.event_key,
            source: event.source.unwrap_or_else(|| "-".to_string()),
            session_id: event.session_id,
            turn_id: event.turn_id,
            delay_seconds: event.delay_seconds.unwrap_or(0),
            decision_label,
            decision_kind,
            body: truncate(&event.body.unwrap_or_default(), 280),
        }
    }
}

fn installed_app_command_present(commands: &[String]) -> bool {
    commands
        .iter()
        .any(|command| command.contains(INSTALLED_APP_EXECUTABLE))
}

fn read_tail_lines(path: &Path, max_bytes: u64) -> Result<Vec<String>> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;

    let mut bytes = Vec::with_capacity((len - start).min(max_bytes) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok(Vec::new());
        };
        bytes.drain(..=newline);
    }

    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(str::to_string).collect())
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn test_feedback_snapshot(thread_id: &str) -> HookFeedbackSnapshot {
        HookFeedbackSnapshot {
            thread_id: thread_id.to_string(),
            title: "测试线程".to_string(),
            timestamp: None,
            text: "最终结果在这里。".to_string(),
        }
    }

    #[test]
    fn stop_hook_does_not_fallback_after_normal_assistant_message() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some("任务已经完成，验证通过。".to_string()),
        };
        let classification = classify_stop_input(&input);
        assert_eq!(classification.decision.kind, RecoveryKind::None);
        assert_eq!(classification.body, "任务已经完成，验证通过。");
        assert_eq!(classification.source, "last_assistant_message");
        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "none"
        );
    }

    #[test]
    fn normal_completion_requires_latest_feedback_snapshot() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some("任务已经完成，验证通过。".to_string()),
        };
        let mut classification = classify_stop_input(&input);

        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "none"
        );

        classification.latest_feedback = Some(test_feedback_snapshot("thread"));
        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "completed"
        );
    }

    #[test]
    fn stop_hook_ignores_machine_control_json_payloads() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some(
                r#"{"suggestions":[{"appId":"","description":"x","prompt":"y","title":"z"}]}"#
                    .to_string(),
            ),
        };
        let classification = classify_stop_input(&input);
        assert_eq!(classification.decision.kind, RecoveryKind::None);
        assert_eq!(classification.source, "last_assistant_message");
        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "none"
        );

        let exclude_input = HookInput {
            last_assistant_message: Some(r#"{"exclude":[]}"#.to_string()),
            ..input
        };
        let exclude_classification = classify_stop_input(&exclude_input);
        assert_eq!(
            stop_hook_action(
                &exclude_input,
                &AppConfig::default(),
                &exclude_classification,
                false
            ),
            "none"
        );
    }

    #[test]
    fn normal_completion_notification_uses_latest_feedback_snapshot() {
        let notification = CompletionNotification {
            event: Some("Stop".to_string()),
            session_id: Some("thread-a".to_string()),
            turn_id: Some("turn-a".to_string()),
            cwd: Some("/Users/gosu/Documents".to_string()),
            model: Some("gpt".to_string()),
            source: "last_assistant_message".to_string(),
            body: "任务已经完成，验证通过。".to_string(),
            latest_feedback: Some(test_feedback_snapshot("thread-a")),
        };

        assert!(should_notify_completion(
            &AppConfig::default(),
            &notification
        ));
        let text = format_completion_notification(&notification);
        assert!(text.contains("线程正常完成"));
        assert!(text.contains("测试线程"));
        assert!(text.contains("thread-a"));
        assert!(text.contains("最终结果在这里。"));

        let keyboard = completion_notification_keyboard(&notification).unwrap();
        assert_eq!(
            keyboard["inline_keyboard"][0][0]["text"].as_str(),
            Some("追加指令")
        );
        assert_eq!(
            keyboard["inline_keyboard"][0][0]["callback_data"].as_str(),
            Some("ask:thread-a")
        );
    }

    #[test]
    fn completion_notification_without_thread_feedback_is_suppressed() {
        let notification = CompletionNotification {
            event: Some("Stop".to_string()),
            session_id: Some("thread-a".to_string()),
            turn_id: Some("turn-a".to_string()),
            cwd: Some("/Users/gosu/.codex/memories".to_string()),
            model: Some("gpt".to_string()),
            source: "last_assistant_message".to_string(),
            body: "Updated [MEMORY.md](/Users/gosu/.codex/memories/MEMORY.md).".to_string(),
            latest_feedback: None,
        };

        assert!(!should_notify_completion(
            &AppConfig::default(),
            &notification
        ));
    }

    #[test]
    fn completion_notification_can_be_disabled_by_config() {
        let mut cfg = AppConfig::default();
        cfg.observability.completion_notifications_enabled = false;
        let notification = CompletionNotification {
            event: Some("Stop".to_string()),
            session_id: Some("thread-a".to_string()),
            turn_id: Some("turn-a".to_string()),
            cwd: Some("/Users/gosu/Documents".to_string()),
            model: Some("gpt".to_string()),
            source: "last_assistant_message".to_string(),
            body: "任务已经完成，验证通过。".to_string(),
            latest_feedback: Some(test_feedback_snapshot("thread-a")),
        };

        assert!(!should_notify_completion(&cfg, &notification));
    }

    #[test]
    fn latest_feedback_collection_can_be_disabled_for_stop_classification() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some("任务已经完成，验证通过。".to_string()),
        };

        let classification = classify_stop_input_with_feedback(&input, false);

        assert_eq!(classification.decision.kind, RecoveryKind::None);
        assert!(classification.latest_feedback.is_none());
        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "none"
        );
    }

    #[test]
    fn transcript_fallback_without_thread_feedback_is_not_notified() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-transcript-{}.jsonl",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let raw = serde_json::to_string(&json!({
            "timestamp": "2026-05-12T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "从 transcript 读到的完成反馈"}
                ]
            }
        }))
        .unwrap();
        fs::write(&path, raw).unwrap();

        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: None,
            transcript_path: Some(path.display().to_string()),
            cwd: None,
            model: None,
            turn_id: None,
            stop_hook_active: Some(false),
            last_assistant_message: None,
        };
        let classification = classify_stop_input(&input);

        assert_eq!(classification.decision.kind, RecoveryKind::None);
        assert_eq!(classification.source, "transcript_path");
        assert_eq!(
            stop_hook_action(&input, &AppConfig::default(), &classification, false),
            "none"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn stop_hook_ignores_silent_completion_phrase_in_final_assistant_message() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some(
                "根因是 app-server completed/error:null 压掉了 completed without a final assistant message 这个异常信号，已修复并验证。".to_string(),
            ),
        };
        let classification = classify_stop_input(&input);

        assert_eq!(classification.decision.kind, RecoveryKind::None);
        assert_eq!(classification.source, "last_assistant_message");
    }

    #[test]
    fn safety_stop_hook_prompt_does_not_echo_trigger_terms() {
        let mut cfg = AppConfig::default();
        cfg.recovery.safety_rephrase_prompt = "继续干。possible cybersecurity risk / try rephrasing your request / Trusted Access for Cyber / https://chatgpt.com/cyber".to_string();
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some(
                "This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request. To get authorized for security work, join the Trusted Access for Cyber program: https://chatgpt.com/cyber".to_string(),
            ),
        };
        let decision = classify_error(input.last_assistant_message.as_deref().unwrap());
        let response = stop_hook_response(&input, &cfg, &decision, false, false).unwrap();
        let reason = response
            .get("reason")
            .and_then(Value::as_str)
            .expect("hook response contains continuation reason");
        let lower = reason.to_ascii_lowercase();

        assert_eq!(
            response.get("decision").and_then(Value::as_str),
            Some("block")
        );
        assert!(!lower.contains("possible cybersecurity risk"));
        assert!(!lower.contains("try rephrasing"));
        assert!(!lower.contains("trusted access for cyber"));
        assert!(!lower.contains("chatgpt.com/cyber"));
        assert!(reason.contains("平台内容安全规则"));
    }

    #[test]
    fn stop_hook_prefers_log_fallback_when_turn_id_is_present() {
        let with_turn = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: None,
        };
        let without_turn = HookInput {
            turn_id: None,
            ..with_turn.clone()
        };

        assert!(should_prefer_log_fallback(&with_turn));
        assert!(!should_prefer_log_fallback(&without_turn));
    }

    #[test]
    fn parses_realistic_codex_stop_payload_variants() {
        let payload = include_str!("../tests/fixtures/hook_stop_full_payload.json");
        let input: HookInput = serde_json::from_str(payload).unwrap();
        let classification = classify_stop_input(&input);

        assert_eq!(input.hook_event_name.as_deref(), Some("Stop"));
        assert_eq!(input.cwd.as_deref(), Some("/Users/gosu/Documents"));
        assert_eq!(classification.decision.kind, RecoveryKind::RetrySoon);
        assert_eq!(classification.source, "last_assistant_message");

        let minimal_payload = include_str!("../tests/fixtures/hook_stop_minimal_payload.json");
        let minimal: HookInput = serde_json::from_str(minimal_payload).unwrap();
        let minimal_classification = classify_stop_input(&minimal);

        assert_eq!(minimal.turn_id, None);
        assert_eq!(minimal_classification.decision.kind, RecoveryKind::None);
    }

    #[test]
    fn duplicate_stop_hook_is_suppressed() {
        let cfg = AppConfig::default();
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some("Turn error: 503 Service Unavailable".to_string()),
        };
        let decision = classify_error(input.last_assistant_message.as_deref().unwrap());
        let response = stop_hook_response(&input, &cfg, &decision, false, true).unwrap();

        assert_eq!(
            response.get("continue").and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            response
                .get("systemMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("同一个 Stop 事件")
        );
        let classification = stop_classification(
            &input,
            decision,
            "last_assistant_message",
            "Turn error: 503 Service Unavailable",
        );
        assert_eq!(
            stop_hook_action(&input, &cfg, &classification, true),
            "deduped"
        );
    }

    #[test]
    fn normal_completion_event_logging_is_configurable() {
        let mut cfg = AppConfig::default();
        let classification = StopClassification {
            decision: RecoveryDecision::none(),
            source: "last_assistant_message".to_string(),
            body: "任务完成".to_string(),
            latest_feedback: Some(test_feedback_snapshot("thread")),
        };

        assert!(!should_log_stop_event(&cfg, &classification, "completed"));
        cfg.observability.record_normal_completion_events = true;
        assert!(should_log_stop_event(&cfg, &classification, "completed"));
        assert!(should_write_cooldown_key(&classification, "completed"));
    }

    #[test]
    fn long_delay_is_deferred_outside_stop_hook() {
        let cfg = AppConfig::default();
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: None,
        };
        let decision = RecoveryDecision {
            kind: RecoveryKind::RetryLater,
            auto_allowed: true,
            delay_seconds: STOP_HOOK_MAX_INLINE_DELAY_SECONDS + 1,
            label: "Long backoff".to_string(),
            reason: "wait longer outside the Stop hook".to_string(),
        };
        let response = stop_hook_response(&input, &cfg, &decision, false, false).unwrap();

        assert_eq!(
            response.get("continue").and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            response
                .get("systemMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("不在行内长时间等待")
        );
        let classification =
            stop_classification(&input, decision, "codex_log_fallback", "Long backoff");
        assert_eq!(
            stop_hook_action(&input, &cfg, &classification, false),
            "deferred"
        );
    }

    #[test]
    fn recent_hook_events_parse_and_dedupe_by_age() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-hook-events-{}.jsonl",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let old_ts = Utc.with_ymd_and_hms(2026, 5, 12, 0, 0, 0).unwrap();
        let new_ts = Utc.with_ymd_and_hms(2026, 5, 12, 0, 4, 0).unwrap();
        let decision = RecoveryDecision {
            kind: RecoveryKind::RetrySoon,
            auto_allowed: true,
            delay_seconds: 3,
            label: "Temporary upstream failure".to_string(),
            reason: "transient".to_string(),
        };
        let older = json!({
            "ts": old_ts,
            "event": "Stop",
            "action": "continue",
            "event_key": "same-key",
            "source": "last_assistant_message",
            "session_id": "thread",
            "turn_id": "turn",
            "delay_seconds": 3,
            "decision": decision,
            "body": "old event"
        });
        let newer = json!({
            "ts": new_ts,
            "event": "Stop",
            "action": "deduped",
            "event_key": "same-key",
            "source": "last_assistant_message",
            "session_id": "thread",
            "turn_id": "turn",
            "delay_seconds": 3,
            "decision": decision,
            "body": "new event"
        });
        fs::write(
            &path,
            format!(
                "{}\nnot-json\n{}\n",
                serde_json::to_string(&older).unwrap(),
                serde_json::to_string(&newer).unwrap()
            ),
        )
        .unwrap();

        let events = recent_hook_events_from_path(&path, 8).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, "deduped");
        assert_eq!(events[0].decision_kind, "RetrySoon");
        assert!(events[0].body.contains("new event"));

        let duplicate = recent_event_with_key_from_events(
            events.clone(),
            "same-key",
            300,
            Utc.with_ymd_and_hms(2026, 5, 12, 0, 4, 30).unwrap(),
        )
        .unwrap();
        assert!(duplicate.is_some());

        let stale = recent_event_with_key_from_events(
            events,
            "same-key",
            10,
            Utc.with_ymd_and_hms(2026, 5, 12, 0, 4, 30).unwrap(),
        )
        .unwrap();
        assert!(stale.is_none());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn recent_cooldown_key_respects_age() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-hook-cooldowns-{}.jsonl",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let key = "thread:turn:None:hash";
        let now = Utc::now();
        let recent = json!({
            "ts": now - chrono::Duration::seconds(30),
            "event_key": key
        });
        let old = json!({
            "ts": now - chrono::Duration::seconds(400),
            "event_key": "old-key"
        });
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&old).unwrap(),
                serde_json::to_string(&recent).unwrap()
            ),
        )
        .unwrap();

        assert!(recent_cooldown_key_seen_from_path(&path, key, 300, now).unwrap());
        assert!(!recent_cooldown_key_seen_from_path(&path, "old-key", 300, now).unwrap());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn recent_hook_events_hide_stale_completion_without_feedback() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-hook-events-{}.jsonl",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let ts = Utc.with_ymd_and_hms(2026, 5, 12, 1, 0, 0).unwrap();
        let decision = RecoveryDecision::none();
        let stale = json!({
            "ts": ts,
            "event": "Stop",
            "action": "completed",
            "event_key": "memory-thread:turn:None:hash",
            "source": "last_assistant_message",
            "session_id": "memory-thread",
            "turn_id": "turn",
            "decision": decision,
            "body": "Updated [MEMORY.md](/Users/gosu/.codex/memories/MEMORY.md)."
        });
        let visible = json!({
            "ts": ts + chrono::Duration::seconds(1),
            "event": "Stop",
            "action": "completed",
            "event_key": "visible-thread:turn:None:hash",
            "source": "last_assistant_message",
            "session_id": "visible-thread",
            "turn_id": "turn",
            "decision": decision,
            "body": "任务完成",
            "latest_feedback": {
                "thread_id": "visible-thread",
                "title": "可见线程",
                "timestamp": null,
                "text": "任务完成"
            }
        });
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&stale).unwrap(),
                serde_json::to_string(&visible).unwrap()
            ),
        )
        .unwrap();

        let events = recent_hook_events_from_path(&path, 8).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id.as_deref(), Some("visible-thread"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn installed_hook_command_must_point_to_app_bundle() {
        assert!(installed_app_command_present(&[format!(
            "\"{INSTALLED_APP_EXECUTABLE}\" hook-stop"
        )]));
        assert!(!installed_app_command_present(&[
            "\"/Users/gosu/Documents/codex-sentinel-work/target/debug/codex-sentinel\" hook-stop"
                .to_string()
        ]));
    }

    #[test]
    fn hook_event_keys_are_stable_and_dedupable() {
        let input = HookInput {
            hook_event_name: Some("Stop".to_string()),
            session_id: Some("thread-a".to_string()),
            transcript_path: None,
            cwd: None,
            model: None,
            turn_id: Some("turn-a".to_string()),
            stop_hook_active: Some(false),
            last_assistant_message: Some("Turn error: 503 Service Unavailable".to_string()),
        };
        let classification = stop_classification(
            &input,
            classify_error("Turn error: 503 Service Unavailable"),
            "last_assistant_message",
            "Turn error: 503 Service Unavailable",
        );
        let body_hash = stable_hash_hex(&[&classification.body]);
        let key = stop_event_key(&input, &classification, &body_hash);

        assert_eq!(body_hash, stable_hash_hex(&[&classification.body]));
        assert_eq!(key, stop_event_key(&input, &classification, &body_hash));
        assert!(key.contains("thread-a:turn-a:RetrySoon:"));
    }
}
