use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::codex;
use crate::config::{self, AppConfig};
use crate::recovery::{RecoveryDecision, RecoveryKind, classify_error, sanitized_recovery_text};

const SENTINEL_MARKER: &str = "codex-sentinel";
const STOP_TIMEOUT_SECONDS: u64 = 240;
const STOP_LOG_LOOKBACK_SECONDS: i64 = 300;
const STOP_LOG_LOOKBACK_WITH_TURN_SECONDS: i64 = 1800;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookStatus {
    pub feature_enabled: bool,
    pub config_path: String,
    pub hooks_path: String,
    pub hooks_file_exists: bool,
    pub stop_installed: bool,
    pub current_executable: String,
    pub installed_commands: Vec<String>,
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

#[derive(Debug, Clone)]
struct StopClassification {
    decision: RecoveryDecision,
    source: String,
    body: String,
    latest_feedback: Option<HookFeedbackSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
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

    if hooks_file_exists {
        let raw = fs::read_to_string(&hooks_path)
            .with_context(|| format!("failed to read {}", hooks_path.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", hooks_path.display()))?;
        installed_commands = collect_sentinel_commands(&value);
        stop_installed = event_has_sentinel_command(&value, "Stop");
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
    Ok(HookStatus {
        feature_enabled,
        config_path: config_path.display().to_string(),
        hooks_path: hooks_path.display().to_string(),
        hooks_file_exists,
        stop_installed,
        current_executable,
        installed_commands,
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
    let classification = classify_stop_input(&input);
    let response = stop_hook_response(
        &input,
        &cfg,
        &classification.decision,
        should_sleep_in_hook(),
    )?;
    let action = stop_hook_action(&input, &cfg, &classification.decision);

    if should_log_stop_event(&classification, action) {
        if let Err(err) = log_stop_hook_event(&input, &classification, action) {
            tracing::warn!("failed to write Stop hook event: {err:#}");
        }
    }
    print_json(&response)?;
    Ok(())
}

fn classify_stop_input(input: &HookInput) -> StopClassification {
    let message = input.last_assistant_message.as_deref().unwrap_or_default();
    let decision = classify_error(message);
    if decision.kind != RecoveryKind::None {
        return stop_classification(input, decision, "last_assistant_message", message);
    }
    if !message.trim().is_empty() {
        return stop_classification(input, decision, "last_assistant_message", message);
    }

    let transcript_message = latest_transcript_message(input);
    let prefer_log_fallback = should_prefer_log_fallback(input);
    if prefer_log_fallback {
        if let Some(classification) = classify_stop_from_log(input, &decision) {
            return classification;
        }
    }

    if let Some(transcript_message) = transcript_message.as_deref() {
        let transcript_decision = classify_error(transcript_message);
        if transcript_decision.kind != RecoveryKind::None {
            return stop_classification(
                input,
                transcript_decision,
                "transcript_path",
                transcript_message,
            );
        }
    }

    if !prefer_log_fallback {
        if let Some(classification) = classify_stop_from_log(input, &decision) {
            return classification;
        }
    }

    if let Some(transcript_message) = transcript_message {
        return stop_classification(input, decision, "transcript_path", &transcript_message);
    }

    stop_classification(input, decision, "empty", message)
}

fn classify_stop_from_log(
    input: &HookInput,
    base_decision: &RecoveryDecision,
) -> Option<StopClassification> {
    match codex::latest_recovery_log_for_hook(
        input.session_id.as_deref(),
        input.turn_id.as_deref(),
        stop_log_lookback_seconds(input),
    ) {
        Ok(Some(event)) => {
            let fallback_decision = classify_error(&event.body);
            if fallback_decision.kind != RecoveryKind::None {
                return Some(stop_classification(
                    input,
                    fallback_decision,
                    "codex_log_fallback",
                    &event.body,
                ));
            }
        }
        Ok(None) => {}
        Err(err) => {
            let source = format!("Stop hook log fallback failed: {err:#}");
            return Some(stop_classification(
                input,
                base_decision.clone(),
                "log_lookup_error",
                &source,
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

fn stop_classification(
    input: &HookInput,
    decision: RecoveryDecision,
    source: &str,
    body: &str,
) -> StopClassification {
    let latest_feedback = if decision.kind != RecoveryKind::None {
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

    if !cfg.recovery.auto_recover || !decision.auto_allowed {
        return Ok(json!({
            "continue": false,
            "systemMessage": format!("Codex Sentinel 检测到「{}」，但自动恢复已关闭或此类问题需要人工处理：{}", decision.label, decision.reason)
        }));
    }

    if decision.delay_seconds > 0 {
        if decision.delay_seconds > STOP_TIMEOUT_SECONDS.saturating_sub(20) {
            return Ok(json!({
                "continue": false,
                "systemMessage": format!("Codex Sentinel 检测到「{}」，但安全退避为 {}s，超过 Stop hook 超时预算。请稍后通过 Telegram 或桌面端恢复。", decision.label, decision.delay_seconds)
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
    decision: &RecoveryDecision,
) -> &'static str {
    if input.stop_hook_active.unwrap_or(false) {
        "loop_prevented"
    } else if decision.kind == RecoveryKind::None {
        "none"
    } else if !cfg.recovery.auto_recover || !decision.auto_allowed {
        match decision.kind {
            RecoveryKind::Reauth => "manual_reauth",
            _ => "manual",
        }
    } else {
        "continue"
    }
}

fn should_log_stop_event(classification: &StopClassification, action: &str) -> bool {
    classification.decision.kind != RecoveryKind::None
        || action != "none"
        || classification.source == "log_lookup_error"
}

fn log_stop_hook_event(
    input: &HookInput,
    classification: &StopClassification,
    action: &str,
) -> Result<()> {
    let body_hash = stable_hash_hex(&[&classification.body]);
    let line = json!({
        "ts": Utc::now(),
        "event": &input.hook_event_name,
        "action": action,
        "event_key": stop_event_key(input, classification, &body_hash),
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
    append_hook_event(&line)
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

fn append_hook_event(line: &Value) -> Result<()> {
    let dir = config::config_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("hook-events.jsonl");
    let mut raw = serde_json::to_string(&line)?;
    raw.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(raw.as_bytes())?;
    Ok(())
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
        let response = stop_hook_response(&input, &cfg, &decision, false).unwrap();
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
