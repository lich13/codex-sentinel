use std::fs::OpenOptions;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sysinfo::System;
use tauri::menu::{CheckMenuItem, MenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{Manager, WindowEvent};

use crate::{codex, config, control_queue, desktop_control, hooks};

const TRAY_ID: &str = "main";
const TRAY_MENU_SHOW: &str = "tray-show";
const TRAY_MENU_AUTO_RECOVER: &str = "tray-auto-recover";
const TRAY_MENU_QUIT: &str = "tray-quit";
const TELEGRAM_PANEL_HTTP_TIMEOUT_SECONDS: u64 = 20;

struct TrayMenuState {
    auto_recover: CheckMenuItem<tauri::Wry>,
}

#[derive(Debug, Serialize)]
struct DashboardPayload {
    status: codex::SentinelStatus,
    config: ConfigSummary,
    hooks: hooks::HookStatus,
    telegram: TelegramSettings,
    desktop_control: desktop_control::DesktopControlStatus,
    recoverable_threads: Vec<codex::ThreadRecovery>,
    active_feedback: Option<codex::ThreadFeedback>,
}

#[derive(Debug, Serialize)]
struct ConfigSummary {
    config_path: String,
    config_dir: String,
    telegram_enabled: bool,
    telegram_token_configured: bool,
    allowed_user_count: usize,
    allowed_chat_count: usize,
    watch_enabled: bool,
    poll_interval_seconds: u64,
    auto_recover: bool,
    max_recoveries_per_thread: u32,
    cooldown_seconds: u64,
    continue_prompt: String,
    tool_failure_prompt: String,
    safety_rephrase_prompt: String,
}

#[derive(Debug, Serialize)]
struct ContinueResult {
    thread_id: String,
    turn_id: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeSettingsInput {
    watch_enabled: bool,
    poll_interval_seconds: u64,
    auto_recover: bool,
    max_recoveries_per_thread: u32,
    cooldown_seconds: u64,
    continue_prompt: String,
    tool_failure_prompt: String,
    safety_rephrase_prompt: String,
}

#[derive(Debug, Serialize)]
struct TelegramSettings {
    enabled: bool,
    bot_token_masked: String,
    token_configured: bool,
    allowed_user_ids: String,
    allowed_chat_ids: String,
    pairing_enabled: bool,
    pairing_code: String,
    daemon_running: bool,
    config_path: String,
    daemon_log_path: String,
}

#[derive(Debug, Deserialize)]
struct TelegramSettingsInput {
    enabled: bool,
    bot_token: String,
    allowed_user_ids: String,
    allowed_chat_ids: String,
    pairing_enabled: bool,
    pairing_code: String,
}

#[derive(Debug, Serialize)]
struct TelegramBotCheck {
    id: i64,
    username: String,
    first_name: String,
}

#[derive(Debug, Serialize)]
struct TelegramPairResult {
    user_id: Option<i64>,
    chat_id: i64,
    chat_type: String,
    chat_label: String,
    user_label: String,
    update_id: i64,
    dashboard: DashboardPayload,
}

#[derive(Debug, Serialize)]
struct DaemonStartResult {
    already_running: bool,
    pid: Option<u32>,
    log_path: String,
}

#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramMe {
    id: i64,
    username: Option<String>,
    first_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramIncomingMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramIncomingMessage {
    chat: TelegramChat,
    from: Option<TelegramUser>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
}

pub fn run_gui() -> Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            install_tray_menu(app)?;
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_MENU_SHOW => show_main_window(app),
            TRAY_MENU_AUTO_RECOVER => {
                if let Err(err) = toggle_auto_recover_from_tray(app) {
                    tracing::warn!("failed to toggle auto recover from tray: {err:#}");
                }
            }
            TRAY_MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .on_tray_icon_event(|app, event| {
            if tray_event_should_show_window(&event) {
                show_main_window(app);
            }
        })
        .invoke_handler(tauri::generate_handler![
            dashboard,
            install_hooks,
            continue_current_thread,
            submit_thread_instruction,
            start_new_thread,
            archive_thread,
            clear_archived_threads,
            save_runtime_settings,
            set_auto_recover,
            save_telegram_settings,
            test_telegram_bot,
            pair_telegram_bot,
            send_telegram_test_message,
            start_telegram_daemon,
            open_desktop_permissions,
            reveal_config_dir
        ])
        .run(tauri::generate_context!())
        .map_err(|err| anyhow!("failed to run Tauri app: {err}"))?;
    Ok(())
}

fn install_tray_menu(app: &tauri::App) -> Result<()> {
    let auto_recover = config::load_or_create()
        .map(|cfg| cfg.recovery.auto_recover)
        .unwrap_or(true);
    let auto_item = CheckMenuItem::with_id(
        app,
        TRAY_MENU_AUTO_RECOVER,
        "可见自动恢复",
        true,
        auto_recover,
        None::<&str>,
    )?;
    let menu = MenuBuilder::new(app)
        .text(TRAY_MENU_SHOW, "打开控制台")
        .separator()
        .item(&auto_item)
        .separator()
        .text(TRAY_MENU_QUIT, "退出 Codex Sentinel")
        .build()?;
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| anyhow!("tray icon `{TRAY_ID}` was not created from tauri.conf.json"))?;
    tray.set_menu(Some(menu))?;
    tray.set_tooltip(Some(tray_tooltip(auto_recover)))?;
    app.manage(TrayMenuState {
        auto_recover: auto_item,
    });
    Ok(())
}

fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn tray_event_should_show_window(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        }
    )
}

fn toggle_auto_recover_from_tray(app: &tauri::AppHandle) -> Result<()> {
    let current = config::load_or_create()
        .map(|cfg| cfg.recovery.auto_recover)
        .unwrap_or(false);
    let enabled = !current;
    config::set_auto_recover(enabled)?;
    sync_tray_auto_recover(app, enabled);
    Ok(())
}

fn sync_tray_auto_recover(app: &tauri::AppHandle, enabled: bool) {
    if let Some(state) = app.try_state::<TrayMenuState>() {
        if let Err(err) = state.auto_recover.set_checked(enabled) {
            tracing::warn!("failed to update tray auto recover checkmark: {err:#}");
        }
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Err(err) = tray.set_tooltip(Some(tray_tooltip(enabled))) {
            tracing::warn!("failed to update tray tooltip: {err:#}");
        }
    }
}

fn tray_tooltip(auto_recover: bool) -> String {
    format!(
        "Codex Sentinel · 可见自动恢复{}",
        if auto_recover {
            "已开启"
        } else {
            "已关闭"
        }
    )
}

#[tauri::command]
fn dashboard() -> std::result::Result<DashboardPayload, String> {
    load_dashboard().map_err(format_error)
}

#[tauri::command]
fn install_hooks() -> std::result::Result<hooks::HookInstallResult, String> {
    hooks::install_hooks().map_err(format_error)
}

#[tauri::command]
fn continue_current_thread(
    thread_id: Option<String>,
) -> std::result::Result<ContinueResult, String> {
    let cfg = config::load_or_create().map_err(format_error)?;
    let thread_id = thread_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| {
            codex::read_recent_threads(1)
                .ok()?
                .into_iter()
                .next()
                .map(|thread| thread.id)
        })
        .ok_or_else(|| "没有找到最近的 Codex 线程".to_string())?;
    let response = control_queue::submit_and_wait(control_queue::ControlAction::Continue {
        thread_id: thread_id.clone(),
        prompt: cfg.recovery.continue_prompt,
    })
    .map_err(format_error)?;
    let turn_id = response.turn_id.unwrap_or_default();
    Ok(ContinueResult { thread_id, turn_id })
}

#[tauri::command]
fn submit_thread_instruction(
    thread_id: String,
    prompt: String,
) -> std::result::Result<control_queue::ControlResponse, String> {
    let thread_id = thread_id.trim();
    let prompt = prompt.trim();
    if thread_id.is_empty() {
        return Err("线程 ID 为空".to_string());
    }
    if prompt.is_empty() {
        return Err("追加指令为空".to_string());
    }
    control_queue::submit_and_wait(control_queue::ControlAction::Continue {
        thread_id: thread_id.to_string(),
        prompt: prompt.to_string(),
    })
    .map_err(format_error)
}

#[tauri::command]
fn start_new_thread(
    prompt: String,
    path: Option<String>,
) -> std::result::Result<control_queue::ControlResponse, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("新线程指令为空".to_string());
    }
    control_queue::submit_and_wait(control_queue::ControlAction::NewThread {
        prompt: prompt.to_string(),
        path: path.and_then(|value| {
            let trimmed = value.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        }),
    })
    .map_err(format_error)
}

#[tauri::command]
fn archive_thread(thread_id: String) -> std::result::Result<DashboardPayload, String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("线程 ID 为空".to_string());
    }
    control_queue::submit_and_wait(control_queue::ControlAction::ArchiveThread {
        thread_id: thread_id.to_string(),
    })
    .map_err(format_error)?;
    load_dashboard().map_err(format_error)
}

#[tauri::command]
fn clear_archived_threads() -> std::result::Result<DashboardPayload, String> {
    control_queue::submit_and_wait(control_queue::ControlAction::ClearArchived)
        .map_err(format_error)?;
    load_dashboard().map_err(format_error)
}

#[tauri::command]
fn save_runtime_settings(
    input: RuntimeSettingsInput,
) -> std::result::Result<DashboardPayload, String> {
    if input.poll_interval_seconds < 5 {
        return Err("轮询间隔不能小于 5 秒".to_string());
    }
    if input.max_recoveries_per_thread == 0 {
        return Err("恢复上限不能为 0".to_string());
    }
    if input.continue_prompt.trim().is_empty() {
        return Err("默认续跑指令不能为空".to_string());
    }
    let mut cfg = config::load_or_create().map_err(format_error)?;
    cfg.watch.enabled = input.watch_enabled;
    cfg.watch.poll_interval_seconds = input.poll_interval_seconds;
    cfg.watch.max_recoveries_per_thread = input.max_recoveries_per_thread;
    cfg.watch.cooldown_seconds = input.cooldown_seconds;
    cfg.recovery.auto_recover = input.auto_recover;
    cfg.recovery.continue_prompt = input.continue_prompt.trim().to_string();
    cfg.recovery.tool_failure_prompt = input.tool_failure_prompt.trim().to_string();
    cfg.recovery.safety_rephrase_prompt = input.safety_rephrase_prompt.trim().to_string();
    config::save(&cfg).map_err(format_error)?;
    load_dashboard().map_err(format_error)
}

#[tauri::command]
fn set_auto_recover(
    app: tauri::AppHandle,
    enabled: bool,
) -> std::result::Result<DashboardPayload, String> {
    config::set_auto_recover(enabled).map_err(format_error)?;
    sync_tray_auto_recover(&app, enabled);
    load_dashboard().map_err(format_error)
}

#[tauri::command]
fn save_telegram_settings(
    input: TelegramSettingsInput,
) -> std::result::Result<DashboardPayload, String> {
    let mut cfg = config::load_or_create().map_err(format_error)?;
    cfg.telegram.enabled = input.enabled;
    if !input.bot_token.trim().is_empty() {
        cfg.telegram.bot_token = input.bot_token.trim().to_string();
    }
    cfg.telegram.allowed_user_ids = parse_id_list(&input.allowed_user_ids).map_err(format_error)?;
    cfg.telegram.allowed_chat_ids = parse_id_list(&input.allowed_chat_ids).map_err(format_error)?;
    cfg.telegram.pairing_enabled = input.pairing_enabled;
    cfg.telegram.pairing_code =
        normalize_optional_pair_code(&input.pairing_code).map_err(format_error)?;
    config::save(&cfg).map_err(format_error)?;
    load_dashboard().map_err(format_error)
}

#[tauri::command]
async fn test_telegram_bot(
    input: TelegramSettingsInput,
) -> std::result::Result<TelegramBotCheck, String> {
    let token = token_from_input_or_config(&input).map_err(format_error)?;
    let url = telegram_api_url(&token, "getMe");
    let resp: TelegramApiResponse<TelegramMe> = telegram_client()
        .map_err(format_error)?
        .get(url)
        .send()
        .await
        .map_err(format_error)?
        .json()
        .await
        .map_err(format_error)?;
    if !resp.ok {
        return Err(resp
            .description
            .unwrap_or_else(|| "Telegram getMe returned ok=false".to_string()));
    }
    let me = resp
        .result
        .ok_or_else(|| "Telegram getMe did not return bot info".to_string())?;
    Ok(TelegramBotCheck {
        id: me.id,
        username: me.username.unwrap_or_default(),
        first_name: me.first_name.unwrap_or_default(),
    })
}

#[tauri::command]
async fn pair_telegram_bot(
    input: TelegramSettingsInput,
    code: String,
) -> std::result::Result<TelegramPairResult, String> {
    let token = token_from_input_or_config(&input).map_err(format_error)?;
    let code = normalize_pair_code(&code).map_err(format_error)?;
    let client = telegram_client().map_err(format_error)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut offset: Option<i64> = None;

    while Instant::now() < deadline {
        let mut body = json!({
            "timeout": 8,
            "allowed_updates": ["message"]
        });
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }

        let resp: TelegramApiResponse<Vec<TelegramUpdate>> = client
            .post(telegram_api_url(&token, "getUpdates"))
            .json(&body)
            .send()
            .await
            .map_err(format_error)?
            .json()
            .await
            .map_err(format_error)?;
        if !resp.ok {
            return Err(resp.description.unwrap_or_else(|| {
                "Telegram getUpdates returned ok=false; 如果机器人启用了 webhook，需要先关闭 webhook 才能轮询配对。".to_string()
            }));
        }

        for update in resp.result.unwrap_or_default() {
            offset = Some(update.update_id + 1);
            let Some(message) = update.message else {
                continue;
            };
            let text = message.text.as_deref().unwrap_or_default();
            if !pair_text_matches(text, &code) {
                continue;
            }

            acknowledge_updates(&client, &token, update.update_id + 1)
                .await
                .map_err(format_error)?;

            let user_id = message.from.as_ref().map(|user| user.id);
            let mut cfg = config::load_or_create().map_err(format_error)?;
            cfg.telegram.enabled = true;
            if !input.bot_token.trim().is_empty() {
                cfg.telegram.bot_token = input.bot_token.trim().to_string();
            }
            cfg.telegram.pairing_enabled = input.pairing_enabled;
            cfg.telegram.pairing_code = code.clone();
            if let Some(user_id) = user_id {
                push_unique(&mut cfg.telegram.allowed_user_ids, user_id);
            }
            push_unique(&mut cfg.telegram.allowed_chat_ids, message.chat.id);
            config::save(&cfg).map_err(format_error)?;

            let chat_label = label_chat(&message.chat);
            let user_label = message
                .from
                .as_ref()
                .map(label_user)
                .unwrap_or_else(|| "未知用户".to_string());
            let _ = send_telegram_message(
                &client,
                &token,
                message.chat.id,
                "Codex Sentinel 配对成功。现在可以发送 /status、/threads、/recover、/continue。",
            )
            .await;

            return Ok(TelegramPairResult {
                user_id,
                chat_id: message.chat.id,
                chat_type: message.chat.kind,
                chat_label,
                user_label,
                update_id: update.update_id,
                dashboard: load_dashboard().map_err(format_error)?,
            });
        }
    }

    Err(format!(
        "60 秒内没有收到配对消息。请确认你已经向这个机器人发送：/pair {code}"
    ))
}

#[tauri::command]
async fn send_telegram_test_message(
    input: TelegramSettingsInput,
) -> std::result::Result<String, String> {
    let token = token_from_input_or_config(&input).map_err(format_error)?;
    let chat_ids = if input.allowed_chat_ids.trim().is_empty() {
        config::load_or_create()
            .map_err(format_error)?
            .telegram
            .allowed_chat_ids
    } else {
        parse_id_list(&input.allowed_chat_ids).map_err(format_error)?
    };
    if chat_ids.is_empty() {
        return Err(
            "allowed_chat_ids 为空，无法发送测试消息。先填你的 Telegram chat_id。".to_string(),
        );
    }

    let client = telegram_client().map_err(format_error)?;
    for chat_id in &chat_ids {
        send_telegram_message(
            &client,
            &token,
            *chat_id,
            "Codex Sentinel 测试消息：Telegram 接入正常。",
        )
        .await
        .map_err(|err| format!("sendMessage to {chat_id} failed: {err}"))?;
    }
    Ok(format!("测试消息已发送到 {} 个 chat。", chat_ids.len()))
}

#[tauri::command]
fn start_telegram_daemon() -> std::result::Result<DaemonStartResult, String> {
    if telegram_daemon_running() {
        return Ok(DaemonStartResult {
            already_running: true,
            pid: None,
            log_path: daemon_log_path().display().to_string(),
        });
    }

    let log_path = daemon_log_path();
    let err_path = config::config_dir().join("telegram-daemon.err.log");
    std::fs::create_dir_all(config::config_dir()).map_err(format_error)?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(format_error)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(err_path)
        .map_err(format_error)?;

    let child = Command::new(std::env::current_exe().map_err(format_error)?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(format_error)?;

    Ok(DaemonStartResult {
        already_running: false,
        pid: Some(child.id()),
        log_path: log_path.display().to_string(),
    })
}

#[tauri::command]
fn reveal_config_dir() -> std::result::Result<(), String> {
    let dir = config::config_dir();
    std::fs::create_dir_all(&dir).map_err(format_error)?;
    let status = std::process::Command::new("open")
        .arg(&dir)
        .status()
        .map_err(format_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open {} failed with {status}", dir.display()))
    }
}

#[tauri::command]
fn open_desktop_permissions() -> std::result::Result<DashboardPayload, String> {
    desktop_control::open_permission_settings().map_err(format_error)?;
    load_dashboard().map_err(format_error)
}

fn load_dashboard() -> Result<DashboardPayload> {
    let cfg = config::load_or_create()?;
    let status = codex::collect_status()?;
    let active_feedback = status
        .recent_threads
        .first()
        .and_then(|thread| codex::latest_thread_feedback_for(thread).ok());
    Ok(DashboardPayload {
        config: summarize_config(&cfg),
        hooks: hooks::inspect_hooks()?,
        telegram: summarize_telegram(&cfg),
        desktop_control: desktop_control::inspect(),
        recoverable_threads: codex::recoverable_threads(5)?,
        active_feedback,
        status,
    })
}

fn summarize_config(cfg: &config::AppConfig) -> ConfigSummary {
    ConfigSummary {
        config_path: config::config_path().display().to_string(),
        config_dir: config::config_dir().display().to_string(),
        telegram_enabled: cfg.telegram.enabled,
        telegram_token_configured: !cfg.telegram.bot_token.trim().is_empty(),
        allowed_user_count: cfg.telegram.allowed_user_ids.len(),
        allowed_chat_count: cfg.telegram.allowed_chat_ids.len(),
        watch_enabled: cfg.watch.enabled,
        poll_interval_seconds: cfg.watch.poll_interval_seconds,
        auto_recover: cfg.recovery.auto_recover,
        max_recoveries_per_thread: cfg.watch.max_recoveries_per_thread,
        cooldown_seconds: cfg.watch.cooldown_seconds,
        continue_prompt: cfg.recovery.continue_prompt.clone(),
        tool_failure_prompt: cfg.recovery.tool_failure_prompt.clone(),
        safety_rephrase_prompt: cfg.recovery.safety_rephrase_prompt.clone(),
    }
}

fn summarize_telegram(cfg: &config::AppConfig) -> TelegramSettings {
    TelegramSettings {
        enabled: cfg.telegram.enabled,
        bot_token_masked: mask_token(&cfg.telegram.bot_token),
        token_configured: !cfg.telegram.bot_token.trim().is_empty(),
        allowed_user_ids: join_ids(&cfg.telegram.allowed_user_ids),
        allowed_chat_ids: join_ids(&cfg.telegram.allowed_chat_ids),
        pairing_enabled: cfg.telegram.pairing_enabled,
        pairing_code: cfg.telegram.pairing_code.clone(),
        daemon_running: telegram_daemon_running(),
        config_path: config::config_path().display().to_string(),
        daemon_log_path: daemon_log_path().display().to_string(),
    }
}

fn parse_id_list(text: &str) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for item in text
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let id = item
            .parse::<i64>()
            .with_context(|| format!("invalid Telegram id: {item}"))?;
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn mask_token(token: &str) -> String {
    let token = token.trim();
    if token.is_empty() {
        return String::new();
    }
    let suffix = token
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    match token.split_once(':') {
        Some((bot_id, _)) => format!("{bot_id}:••••{suffix}"),
        None => format!("••••{suffix}"),
    }
}

fn token_from_input_or_config(input: &TelegramSettingsInput) -> Result<String> {
    if !input.bot_token.trim().is_empty() {
        return Ok(input.bot_token.trim().to_string());
    }
    let cfg = config::load_or_create()?;
    let token = cfg.telegram.bot_token.trim();
    if token.is_empty() {
        Err(anyhow!("bot token 为空"))
    } else {
        Ok(token.to_string())
    }
}

fn normalize_pair_code(code: &str) -> Result<String> {
    let code = code.trim().to_ascii_uppercase();
    if code.len() < 4 {
        Err(anyhow!("配对码太短"))
    } else {
        Ok(code)
    }
}

fn normalize_optional_pair_code(code: &str) -> Result<String> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(String::new());
    }
    normalize_pair_code(code)
}

fn pair_text_matches(text: &str, code: &str) -> bool {
    let text = text.trim().to_ascii_uppercase();
    if text == code {
        return true;
    }
    let mut parts = text.split_whitespace();
    parts
        .next()
        .is_some_and(|command| command == "/PAIR" || command.starts_with("/PAIR@"))
        && parts.any(|part| part.trim() == code)
}

async fn acknowledge_updates(client: &Client, token: &str, offset: i64) -> Result<()> {
    let _resp: TelegramApiResponse<Vec<TelegramUpdate>> = client
        .post(telegram_api_url(token, "getUpdates"))
        .json(&json!({"offset": offset, "timeout": 0, "allowed_updates": ["message"]}))
        .send()
        .await?
        .json()
        .await?;
    Ok(())
}

async fn send_telegram_message(
    client: &Client,
    token: &str,
    chat_id: i64,
    text: &str,
) -> Result<()> {
    let resp: TelegramApiResponse<Value> = client
        .post(telegram_api_url(token, "sendMessage"))
        .json(&json!({"chat_id": chat_id, "text": text}))
        .send()
        .await?
        .json()
        .await?;
    if resp.ok {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            resp.description
                .unwrap_or_else(|| "Telegram sendMessage returned ok=false".to_string())
        ))
    }
}

fn push_unique(ids: &mut Vec<i64>, id: i64) {
    if !ids.contains(&id) {
        ids.push(id);
        ids.sort_unstable();
    }
}

fn label_user(user: &TelegramUser) -> String {
    if let Some(username) = &user.username {
        return format!("@{username} ({})", user.id);
    }
    let name = [user.first_name.as_deref(), user.last_name.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if name.trim().is_empty() {
        user.id.to_string()
    } else {
        format!("{name} ({})", user.id)
    }
}

fn label_chat(chat: &TelegramChat) -> String {
    if let Some(title) = &chat.title {
        return format!("{title} ({})", chat.id);
    }
    if let Some(username) = &chat.username {
        return format!("@{username} ({})", chat.id);
    }
    let name = [chat.first_name.as_deref(), chat.last_name.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if name.trim().is_empty() {
        chat.id.to_string()
    } else {
        format!("{name} ({})", chat.id)
    }
}

fn telegram_api_url(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

fn telegram_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(TELEGRAM_PANEL_HTTP_TIMEOUT_SECONDS))
        .pool_max_idle_per_host(0)
        .build()
        .context("failed to build Telegram HTTP client")
}

fn telegram_daemon_running() -> bool {
    let mut sys = System::new_all();
    sys.refresh_all();
    sys.processes().values().any(|process| {
        let cmd = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        cmd.contains("codex-sentinel") && cmd.contains(" daemon")
    })
}

fn daemon_log_path() -> std::path::PathBuf {
    config::config_dir().join("telegram-daemon.out.log")
}

fn format_error(err: impl std::fmt::Display) -> String {
    err.to_string()
}
