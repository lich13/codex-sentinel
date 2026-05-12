use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::config::AppConfig;
use crate::recovery::{RecoveryDecision, RecoveryKind, sanitized_recovery_text};
use crate::{codex, config, control_queue, desktop_control};

const TELEGRAM_GET_UPDATES_TIMEOUT_SECONDS: u64 = 0;
const TELEGRAM_EMPTY_POLL_SLEEP_SECONDS: u64 = 2;
const TELEGRAM_HTTP_TIMEOUT_SECONDS: u64 = 12;

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
    callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    id: String,
    from: User,
    message: Option<Message>,
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
    from: Option<User>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct User {
    id: i64,
}

#[derive(Debug)]
struct BotReply {
    text: String,
    keyboard: Option<Value>,
}

#[derive(Debug)]
enum PendingAction {
    ThreadInstruction { thread_id: String },
    NewThread,
}

pub async fn run_bot(cfg: AppConfig) -> Result<()> {
    let mut cfg = cfg;
    let client = telegram_client()?;
    if !cfg.telegram.enabled || cfg.telegram.bot_token.trim().is_empty() {
        tracing::info!("telegram disabled; running local watcher only");
        watch_loop(cfg, client).await;
        return Ok(());
    }

    let watcher_cfg = cfg.clone();
    let watcher_client = client.clone();
    tokio::spawn(async move {
        watch_loop(watcher_cfg, watcher_client).await;
    });

    let mut offset = 0_i64;
    let mut recoveries: HashMap<String, RecoveryCounter> = HashMap::new();
    let mut pending: HashMap<i64, PendingAction> = HashMap::new();

    loop {
        if let Ok(fresh) = config::load_or_create() {
            if fresh.telegram.bot_token == cfg.telegram.bot_token {
                cfg = fresh;
            }
        }
        let resp = match get_updates(&client, &cfg.telegram.bot_token, offset).await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::warn!(
                    error = %redact_telegram_token(&format!("{err:#}")),
                    "telegram getUpdates failed"
                );
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if !resp.ok {
            let description = resp
                .description
                .unwrap_or_else(|| "telegram getUpdates returned ok=false".to_string());
            tracing::warn!(
                error = %redact_telegram_token(&description),
                "telegram getUpdates returned ok=false"
            );
            sleep(Duration::from_secs(5)).await;
            continue;
        }

        let updates = resp.result.unwrap_or_default();
        if updates.is_empty() {
            sleep(Duration::from_secs(TELEGRAM_EMPTY_POLL_SLEEP_SECONDS)).await;
            continue;
        }

        for update in updates {
            offset = update.update_id + 1;

            if let Some(callback) = update.callback_query {
                if let Err(err) =
                    answer_callback_query(&client, &cfg.telegram.bot_token, &callback.id).await
                {
                    tracing::warn!(
                        error = %redact_telegram_token(&format!("{err:#}")),
                        "telegram answerCallbackQuery failed"
                    );
                }
                let Some(message) = callback.message else {
                    continue;
                };
                if !allowed_chat_user(&cfg, message.chat.id, Some(callback.from.id)) {
                    continue;
                }
                let data = callback.data.unwrap_or_else(|| "menu".to_string());
                let reply =
                    handle_callback(&cfg, message.chat.id, &data, &mut recoveries, &mut pending)
                        .await;
                if let Err(err) =
                    send_handled_reply(&client, &cfg.telegram.bot_token, message.chat.id, reply)
                        .await
                {
                    tracing::warn!(
                        chat_id = message.chat.id,
                        error = %redact_telegram_token(&format!("{err:#}")),
                        "telegram reply failed"
                    );
                }
                continue;
            }

            let Some(message) = update.message else {
                continue;
            };
            let Some(text) = message.text.as_deref() else {
                continue;
            };
            if is_pair_command(text) {
                let reply = handle_pair_message(&mut cfg, &message, text);
                if let Err(err) =
                    send_handled_reply(&client, &cfg.telegram.bot_token, message.chat.id, reply)
                        .await
                {
                    tracing::warn!(
                        chat_id = message.chat.id,
                        error = %redact_telegram_token(&format!("{err:#}")),
                        "telegram pair reply failed"
                    );
                }
                continue;
            }
            if !allowed(&cfg, &message) {
                continue;
            }
            let reply =
                handle_message(&cfg, message.chat.id, text, &mut recoveries, &mut pending).await;
            if let Err(err) =
                send_handled_reply(&client, &cfg.telegram.bot_token, message.chat.id, reply).await
            {
                tracing::warn!(
                    chat_id = message.chat.id,
                    error = %redact_telegram_token(&format!("{err:#}")),
                    "telegram reply failed"
                );
            }
        }
    }
}

pub async fn notify_configured_text_with_keyboard(
    cfg: &AppConfig,
    text: &str,
    keyboard: Option<Value>,
) -> Result<()> {
    if !cfg.telegram.enabled {
        tracing::debug!("telegram notification skipped because telegram is disabled");
        return Ok(());
    }
    let client = telegram_client()?;
    notify_configured_chats(
        &client,
        &cfg.telegram.bot_token,
        &cfg.telegram.allowed_chat_ids,
        text,
        keyboard,
    )
    .await
}

async fn get_updates(
    client: &Client,
    token: &str,
    offset: i64,
) -> Result<TelegramResponse<Vec<Update>>> {
    client
        .post(api_url(token, "getUpdates")?)
        .json(&json!({
            "timeout": TELEGRAM_GET_UPDATES_TIMEOUT_SECONDS,
            "offset": offset,
            "allowed_updates": ["message", "callback_query"]
        }))
        .send()
        .await
        .context("telegram getUpdates request failed")?
        .json()
        .await
        .context("telegram getUpdates response decode failed")
}

fn telegram_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(TELEGRAM_HTTP_TIMEOUT_SECONDS))
        .pool_max_idle_per_host(0)
        .build()
        .context("failed to build telegram HTTP client")
}

async fn watch_loop(cfg: AppConfig, client: Client) {
    let mut recoveries: HashMap<String, RecoveryCounter> = HashMap::new();
    let mut alerted_keys: HashSet<String> = HashSet::new();

    loop {
        let active_cfg = config::load_or_create().unwrap_or_else(|_| cfg.clone());
        if let Err(err) = watch_once(&active_cfg, &client, &mut recoveries, &mut alerted_keys).await
        {
            tracing::warn!(
                error = %redact_telegram_token(&format!("{err:#}")),
                "watch loop failed"
            );
        }
        sleep(Duration::from_secs(
            active_cfg.watch.poll_interval_seconds.max(5),
        ))
        .await;
    }
}

async fn watch_once(
    cfg: &AppConfig,
    client: &Client,
    recoveries: &mut HashMap<String, RecoveryCounter>,
    alerted_keys: &mut HashSet<String>,
) -> Result<()> {
    if !cfg.watch.enabled {
        return Ok(());
    }

    for candidate in codex::recoverable_threads(10)? {
        let key = format!(
            "{}:{}:{}",
            candidate.thread.id, candidate.event.ts, candidate.decision.label
        );
        if auto_enabled_for(cfg, &candidate.decision) {
            tracing::info!(
                thread_id = %candidate.thread.id,
                label = %candidate.decision.label,
                kind = ?candidate.decision.kind,
                "watch detected recoverable thread"
            );
        } else {
            tracing::debug!(
                thread_id = %candidate.thread.id,
                label = %candidate.decision.label,
                kind = ?candidate.decision.kind,
                "watch observed manual recoverable thread"
            );
        }
        if alerted_keys.insert(key.clone()) {
            notify_configured_chats(
                client,
                &cfg.telegram.bot_token,
                &cfg.telegram.allowed_chat_ids,
                &format_watch_alert(&candidate),
                None,
            )
            .await?;
        }

        if !auto_enabled_for(cfg, &candidate.decision) {
            continue;
        }
        if !desktop_control::visible_input_ready() {
            tracing::info!(
                thread_id = %candidate.thread.id,
                label = %candidate.decision.label,
                "automatic visible recovery skipped because Accessibility is not granted"
            );
            continue;
        }
        if !desktop_control::visible_state_ready() {
            tracing::info!(
                thread_id = %candidate.thread.id,
                label = %candidate.decision.label,
                "automatic visible recovery skipped because Screen Recording is not granted"
            );
            continue;
        }

        let result = recover_candidate(cfg, recoveries, true, &candidate, Some(&key)).await?;
        if !result.is_empty() {
            notify_configured_chats(
                client,
                &cfg.telegram.bot_token,
                &cfg.telegram.allowed_chat_ids,
                &format!("自动恢复结果\n{result}"),
                None,
            )
            .await?;
        }
    }

    Ok(())
}

fn allowed(cfg: &AppConfig, message: &Message) -> bool {
    allowed_chat_user(
        cfg,
        message.chat.id,
        message.from.as_ref().map(|user| user.id),
    )
}

fn allowed_chat_user(cfg: &AppConfig, chat_id: i64, user_id: Option<i64>) -> bool {
    if cfg.telegram.allowed_chat_ids.is_empty()
        && cfg.telegram.allowed_user_ids.is_empty()
        && cfg.telegram.pairing_enabled
        && !cfg.telegram.pairing_code.trim().is_empty()
    {
        return false;
    }
    if !cfg.telegram.allowed_chat_ids.is_empty()
        && !cfg.telegram.allowed_chat_ids.contains(&chat_id)
    {
        return false;
    }
    if let Some(user_id) = user_id {
        if !cfg.telegram.allowed_user_ids.is_empty()
            && !cfg.telegram.allowed_user_ids.contains(&user_id)
        {
            return false;
        }
    }
    true
}

async fn handle_message(
    cfg: &AppConfig,
    chat_id: i64,
    text: &str,
    recoveries: &mut HashMap<String, RecoveryCounter>,
    pending: &mut HashMap<i64, PendingAction>,
) -> Result<BotReply> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("/cancel") || trimmed == "取消" {
        pending.remove(&chat_id);
        return main_menu_reply();
    }

    if let Some(action) = pending.remove(&chat_id) {
        match action {
            PendingAction::ThreadInstruction { thread_id } => {
                if !trimmed.starts_with('/') && !trimmed.is_empty() {
                    let response =
                        control_queue::submit_and_wait(control_queue::ControlAction::Continue {
                            thread_id: thread_id.clone(),
                            prompt: trimmed.to_string(),
                        })?;
                    return Ok(BotReply {
                        text: format!(
                            "已在 Codex APP 可见窗口发送你的指令\n线程：{thread_id}\nturn：{turn}",
                            turn = response.turn_id.unwrap_or_default()
                        ),
                        keyboard: Some(thread_actions_keyboard(&thread_id)),
                    });
                }
            }
            PendingAction::NewThread => {
                if !trimmed.starts_with('/') && !trimmed.is_empty() {
                    let response =
                        control_queue::submit_and_wait(control_queue::ControlAction::NewThread {
                            prompt: trimmed.to_string(),
                            path: default_new_thread_path(),
                        })?;
                    return Ok(BotReply {
                        text: format!(
                            "已在 Codex APP 内创建新线程\n线程：{}\nturn：{}",
                            response.thread_id.unwrap_or_else(|| "-".to_string()),
                            response.turn_id.unwrap_or_default()
                        ),
                        keyboard: Some(main_keyboard()),
                    });
                }
            }
        }
    }

    let mut parts = trimmed.split_whitespace();
    let command = normalize_command_token(parts.next().unwrap_or_default());
    match command.as_str() {
        "/start" | "/help" | "/menu" | "menu" | "菜单" => main_menu_reply(),
        "/status" | "status" | "状态" => status_reply(cfg),
        "/threads" | "threads" | "线程" => thread_list_reply(),
        "/recoverable" | "recoverable" | "待恢复" => recoverable_reply(),
        "/new" | "new" | "新线程" => {
            let prompt = parts.collect::<Vec<_>>().join(" ");
            if prompt.trim().is_empty() {
                pending.insert(chat_id, PendingAction::NewThread);
                Ok(BotReply {
                    text: "下一条消息会作为新线程的首条指令发送到 Codex APP。".to_string(),
                    keyboard: Some(json!({
                        "inline_keyboard": [
                            [{"text": "取消", "callback_data": "cancel"}],
                            [{"text": "返回主菜单", "callback_data": "menu"}]
                        ]
                    })),
                })
            } else {
                start_new_thread_reply(&prompt).await
            }
        }
        "/push_test" | "push_test" | "测试推送" => Ok(BotReply {
            text: "Codex Sentinel 推送通道正常。".to_string(),
            keyboard: Some(main_keyboard()),
        }),
        "/auto" | "auto" | "自动" => {
            let value = parts.next().unwrap_or_default();
            auto_recover_reply(value)
        }
        "/recover" | "recover" | "恢复" => recover_current(cfg, recoveries, false)
            .await
            .map(reply_with_main_keyboard),
        "/continue" | "continue" | "继续" => {
            let thread_id = parts
                .next()
                .map(str::to_string)
                .or_else(|| {
                    codex::read_recent_threads(1)
                        .ok()?
                        .first()
                        .map(|t| t.id.clone())
                })
                .context("没有指定线程，也没有最近线程")?;
            let prompt = parts.collect::<Vec<_>>().join(" ");
            if prompt.trim().is_empty() {
                continue_thread_reply(cfg, &thread_id).await
            } else {
                continue_thread_with_prompt_reply(&thread_id, &prompt).await
            }
        }
        "/archive" | "archive" | "归档" | "/delete" | "delete" | "删除" => {
            let thread_id = parts
                .next()
                .map(str::to_string)
                .context("请提供线程 ID：/delete <thread_id>")?;
            archive_thread_reply(&thread_id)
        }
        "/clear_archived" | "clear_archived" | "清除归档" => clear_archived_reply(),
        _ => Ok(BotReply {
            text: "选择一个线程查看最后反馈；也可以点“一键继续”或“输入指令”。".to_string(),
            keyboard: Some(main_keyboard()),
        }),
    }
}

fn handle_pair_message(cfg: &mut AppConfig, message: &Message, text: &str) -> Result<BotReply> {
    if !cfg.telegram.pairing_enabled {
        return Ok(BotReply {
            text: "远程配对未启用。请先在 Codex Sentinel 桌面面板启用配对。".to_string(),
            keyboard: None,
        });
    }
    let code = cfg.telegram.pairing_code.trim();
    if code.is_empty() {
        return Ok(BotReply {
            text: "远程配对码为空。请先在 Codex Sentinel 桌面面板保存配对码。".to_string(),
            keyboard: None,
        });
    }
    if !pair_text_matches(text, code) {
        return Ok(BotReply {
            text: "配对码不正确。".to_string(),
            keyboard: None,
        });
    }

    if let Some(user) = &message.from {
        push_unique(&mut cfg.telegram.allowed_user_ids, user.id);
    }
    push_unique(&mut cfg.telegram.allowed_chat_ids, message.chat.id);
    cfg.telegram.enabled = true;
    config::save(cfg)?;

    Ok(BotReply {
        text: format!(
            "配对成功。\nchat_id：{}\nuser_id：{}\n\n现在可以使用 /menu、/status、/threads、/new、/continue。在线程详情里可点一键继续或输入指令。",
            message.chat.id,
            message
                .from
                .as_ref()
                .map(|user| user.id.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        keyboard: Some(main_keyboard()),
    })
}

async fn handle_callback(
    cfg: &AppConfig,
    chat_id: i64,
    data: &str,
    recoveries: &mut HashMap<String, RecoveryCounter>,
    pending: &mut HashMap<i64, PendingAction>,
) -> Result<BotReply> {
    if data == "menu" {
        pending.remove(&chat_id);
        return main_menu_reply();
    }
    if data == "status" {
        pending.remove(&chat_id);
        return status_reply(cfg);
    }
    if data == "threads" {
        pending.remove(&chat_id);
        return thread_list_reply();
    }
    if data == "recoverable" {
        pending.remove(&chat_id);
        return recoverable_reply();
    }
    if data == "recover" {
        pending.remove(&chat_id);
        return recover_current(cfg, recoveries, false)
            .await
            .map(reply_with_main_keyboard);
    }
    if data == "new" {
        pending.insert(chat_id, PendingAction::NewThread);
        return Ok(BotReply {
            text: "下一条消息会作为新线程的首条指令发送到 Codex APP。".to_string(),
            keyboard: Some(json!({
                "inline_keyboard": [
                    [{"text": "取消", "callback_data": "cancel"}],
                    [{"text": "返回主菜单", "callback_data": "menu"}]
                ]
            })),
        });
    }
    if data == "cancel" {
        pending.remove(&chat_id);
        return main_menu_reply();
    }
    if let Some(thread_id) = data.strip_prefix("thread:") {
        pending.remove(&chat_id);
        return thread_detail_reply(thread_id);
    }
    if let Some(thread_id) = data.strip_prefix("cont:") {
        pending.remove(&chat_id);
        return continue_thread_reply(cfg, thread_id).await;
    }
    if data == "clear_archived" {
        pending.remove(&chat_id);
        return clear_archived_reply();
    }
    if let Some(thread_id) = data
        .strip_prefix("archive:")
        .or_else(|| data.strip_prefix("del:"))
    {
        pending.remove(&chat_id);
        return archive_thread_reply(thread_id);
    }
    if let Some(thread_id) = data.strip_prefix("ask:") {
        pending.insert(
            chat_id,
            PendingAction::ThreadInstruction {
                thread_id: thread_id.to_string(),
            },
        );
        return Ok(BotReply {
            text: format!(
                "下一条消息会作为后续指令发送到线程：{thread_id}\n\n直接输入你想让 Codex 接着做什么。"
            ),
            keyboard: Some(json!({
                "inline_keyboard": [
                    [{"text": "取消", "callback_data": "cancel"}],
                    [{"text": "返回线程", "callback_data": format!("thread:{thread_id}")}]
                ]
            })),
        });
    }

    main_menu_reply()
}

fn main_menu_reply() -> Result<BotReply> {
    let active = codex::read_recent_threads(1)?
        .first()
        .map(|thread| format!("当前线程：{}\n{}", thread.title, thread.id))
        .unwrap_or_else(|| "当前没有读取到 Codex 线程。".to_string());
    Ok(BotReply {
        text: format!("{active}\n\n所有动作都会回到 Codex APP 可见窗口执行。"),
        keyboard: Some(main_keyboard()),
    })
}

fn status_reply(cfg: &AppConfig) -> Result<BotReply> {
    let status = codex::collect_status()?;
    let recoverable = codex::recoverable_threads(5)?;
    let desktop = desktop_control::inspect();
    let active = status
        .recent_threads
        .first()
        .map(|thread| format!("{}\n{}", thread.title, thread.id))
        .unwrap_or_else(|| "未读取到最近线程".to_string());
    Ok(BotReply {
        text: format!(
            "Codex Sentinel 状态\n\nCodex APP：{}\n可见输入：{}\n自动恢复：{}\nWatcher：{} / {}s\n待恢复线程：{}\n\n当前线程：\n{}",
            if status.codex_running {
                "运行中"
            } else {
                "未发现"
            },
            if desktop.accessibility_granted {
                "已授权"
            } else {
                "未授权"
            },
            if cfg.recovery.auto_recover {
                "开启"
            } else {
                "关闭"
            },
            if cfg.watch.enabled {
                "开启"
            } else {
                "关闭"
            },
            cfg.watch.poll_interval_seconds,
            recoverable.len(),
            active,
        ),
        keyboard: Some(main_keyboard()),
    })
}

fn recoverable_reply() -> Result<BotReply> {
    let items = codex::recoverable_threads(8)?;
    if items.is_empty() {
        return Ok(BotReply {
            text: "当前没有待恢复线程。".to_string(),
            keyboard: Some(main_keyboard()),
        });
    }

    let rows = items
        .iter()
        .map(|item| {
            vec![
                json!({
                    "text": format!("查看 {}", truncate(&item.thread.title, 24)),
                    "callback_data": format!("thread:{}", item.thread.id)
                }),
                json!({
                    "text": "继续",
                    "callback_data": format!("cont:{}", item.thread.id)
                }),
            ]
        })
        .chain(std::iter::once(vec![json!({
            "text": "返回主菜单",
            "callback_data": "menu"
        })]))
        .collect::<Vec<_>>();

    let text = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            format!(
                "{}. {}\n{}\n原因：{}",
                idx + 1,
                item.thread.title,
                item.thread.id,
                item.decision.label
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(BotReply {
        text: format!("待恢复线程：\n\n{}", truncate(&text, 3000)),
        keyboard: Some(json!({ "inline_keyboard": rows })),
    })
}

fn auto_recover_reply(value: &str) -> Result<BotReply> {
    let cfg = config::load_or_create()?;
    let normalized = value.trim().to_ascii_lowercase();
    let cfg = match normalized.as_str() {
        "on" | "1" | "true" | "开" | "开启" => config::set_auto_recover(true)?,
        "off" | "0" | "false" | "关" | "关闭" => config::set_auto_recover(false)?,
        _ => {
            return Ok(BotReply {
                text: format!(
                    "当前自动恢复：{}\n使用 /auto on 或 /auto off 切换。",
                    if cfg.recovery.auto_recover {
                        "开启"
                    } else {
                        "关闭"
                    }
                ),
                keyboard: Some(main_keyboard()),
            });
        }
    };
    Ok(BotReply {
        text: format!(
            "自动恢复已{}。",
            if cfg.recovery.auto_recover {
                "开启"
            } else {
                "关闭"
            }
        ),
        keyboard: Some(main_keyboard()),
    })
}

fn main_keyboard() -> Value {
    let active = codex::read_recent_threads(1)
        .ok()
        .and_then(|threads| threads.first().map(|thread| thread.id.clone()));
    let mut rows = Vec::new();
    if let Some(thread_id) = active {
        rows.push(vec![
            json!({"text": "当前线程", "callback_data": format!("thread:{thread_id}")}),
            json!({"text": "继续当前", "callback_data": format!("cont:{thread_id}")}),
        ]);
    }
    rows.push(vec![
        json!({"text": "新线程", "callback_data": "new"}),
        json!({"text": "选择线程", "callback_data": "threads"}),
    ]);
    rows.push(vec![
        json!({"text": "待恢复", "callback_data": "recoverable"}),
        json!({"text": "状态", "callback_data": "status"}),
    ]);
    rows.push(vec![
        json!({"text": "清除归档", "callback_data": "clear_archived"}),
    ]);
    json!({ "inline_keyboard": rows })
}

fn thread_list_reply() -> Result<BotReply> {
    let threads = codex::read_recent_threads(8)?;
    if threads.is_empty() {
        return Ok(BotReply {
            text: "没有读取到最近线程。".to_string(),
            keyboard: Some(main_keyboard()),
        });
    }

    let rows = threads
        .iter()
        .enumerate()
        .map(|(idx, thread)| {
            vec![json!({
                "text": format!("{}. {}", idx + 1, truncate(&thread.title, 28)),
                "callback_data": format!("thread:{}", thread.id)
            })]
        })
        .chain(std::iter::once(vec![json!({
            "text": "返回主菜单",
            "callback_data": "menu"
        })]))
        .collect::<Vec<_>>();

    Ok(BotReply {
        text: "选择一个线程查看最后反馈：".to_string(),
        keyboard: Some(json!({ "inline_keyboard": rows })),
    })
}

fn thread_detail_reply(thread_id: &str) -> Result<BotReply> {
    let feedback = codex::latest_thread_feedback(thread_id)?;
    Ok(BotReply {
        text: format!(
            "线程：{}\n{}\n\n最后反馈：\n{}",
            feedback.title,
            feedback.thread_id,
            truncate(&feedback.text, 2500)
        ),
        keyboard: Some(thread_actions_keyboard(thread_id)),
    })
}

fn thread_actions_keyboard(thread_id: &str) -> Value {
    json!({
        "inline_keyboard": [
            [
                {"text": "继续", "callback_data": format!("cont:{thread_id}")},
                {"text": "输入指令", "callback_data": format!("ask:{thread_id}")}
            ],
            [
                {"text": "刷新反馈", "callback_data": format!("thread:{thread_id}")},
                {"text": "线程列表", "callback_data": "threads"}
            ],
            [
                {"text": "删除线程", "callback_data": format!("del:{thread_id}")},
                {"text": "主菜单", "callback_data": "menu"}
            ]
        ]
    })
}

async fn continue_thread_reply(cfg: &AppConfig, thread_id: &str) -> Result<BotReply> {
    continue_thread_with_prompt_reply(thread_id, &cfg.recovery.continue_prompt).await
}

async fn continue_thread_with_prompt_reply(thread_id: &str, prompt: &str) -> Result<BotReply> {
    let response = control_queue::submit_and_wait(control_queue::ControlAction::Continue {
        thread_id: thread_id.to_string(),
        prompt: prompt.to_string(),
    })?;
    Ok(BotReply {
        text: format!(
            "已打开 Codex APP，并在可见输入框发送继续指令\n线程：{thread_id}\nturn：{turn}",
            turn = response.turn_id.unwrap_or_default()
        ),
        keyboard: Some(thread_actions_keyboard(thread_id)),
    })
}

async fn start_new_thread_reply(prompt: &str) -> Result<BotReply> {
    let response = control_queue::submit_and_wait(control_queue::ControlAction::NewThread {
        prompt: prompt.to_string(),
        path: default_new_thread_path(),
    })?;
    Ok(BotReply {
        text: format!(
            "已在 Codex APP 内创建新线程\n线程：{}\nturn：{}",
            response.thread_id.unwrap_or_else(|| "-".to_string()),
            response.turn_id.unwrap_or_default()
        ),
        keyboard: Some(main_keyboard()),
    })
}

fn archive_thread_reply(thread_id: &str) -> Result<BotReply> {
    let response = control_queue::submit_and_wait(control_queue::ControlAction::ArchiveThread {
        thread_id: thread_id.to_string(),
    })?;
    Ok(BotReply {
        text: response.message,
        keyboard: Some(main_keyboard()),
    })
}

fn clear_archived_reply() -> Result<BotReply> {
    let response = control_queue::submit_and_wait(control_queue::ControlAction::ClearArchived)?;
    Ok(BotReply {
        text: response.message,
        keyboard: Some(main_keyboard()),
    })
}

fn default_new_thread_path() -> Option<String> {
    codex::read_recent_threads(1)
        .ok()
        .and_then(|threads| threads.first().map(|thread| thread.cwd.clone()))
}

fn reply_with_main_keyboard(text: String) -> BotReply {
    BotReply {
        text,
        keyboard: Some(main_keyboard()),
    }
}

async fn recover_current(
    cfg: &AppConfig,
    recoveries: &mut HashMap<String, RecoveryCounter>,
    automatic: bool,
) -> Result<String> {
    let candidates = codex::recoverable_threads(10)?;
    let Some(candidate) = candidates.first() else {
        return Ok("没有待恢复线程。".to_string());
    };
    recover_candidate(cfg, recoveries, automatic, candidate, None).await
}

async fn recover_candidate(
    cfg: &AppConfig,
    recoveries: &mut HashMap<String, RecoveryCounter>,
    automatic: bool,
    candidate: &codex::ThreadRecovery,
    event_key: Option<&str>,
) -> Result<String> {
    let thread = &candidate.thread;
    let decision = candidate.decision.clone();
    if decision.kind == RecoveryKind::None {
        return Ok(format!("无需恢复\n{}", decision.reason));
    }
    if automatic && !auto_enabled_for(cfg, &decision) {
        return Ok(format!("不自动恢复：{}", decision.label));
    }
    if decision.kind == RecoveryKind::Reauth {
        return Ok(format!(
            "需要重新授权：{}\n{}",
            decision.label, decision.reason
        ));
    }

    let max_attempts = max_recoveries_for(&decision).min(cfg.watch.max_recoveries_per_thread);
    let counter = recoveries.entry(thread.id.clone()).or_default();
    if automatic && counter.already_handled(event_key) {
        tracing::debug!(
            thread_id = %thread.id,
            label = %decision.label,
            "recovery already handled for this event"
        );
        return Ok(String::new());
    }
    if !counter.can_run(max_attempts, cfg.watch.cooldown_seconds) {
        tracing::info!(
            thread_id = %thread.id,
            label = %decision.label,
            max_attempts,
            cooldown_seconds = cfg.watch.cooldown_seconds,
            "recovery suppressed by counter"
        );
        return Ok(format!(
            "恢复已节流\n线程：{}\n上限：{} 次，冷却：{}s",
            thread.id, max_attempts, cfg.watch.cooldown_seconds
        ));
    }

    if automatic {
        match desktop_control::inspect_thread_failure_state(&thread.id)? {
            desktop_control::VisibleThreadFailureState::Failed => {
                tracing::info!(
                    thread_id = %thread.id,
                    label = %decision.label,
                    "visible Codex error marker confirmed before automatic recovery"
                );
            }
            desktop_control::VisibleThreadFailureState::StoppedMarker => {
                tracing::info!(
                    thread_id = %thread.id,
                    label = %decision.label,
                    "visible Codex stopped marker confirmed with terminal recovery event before automatic recovery"
                );
            }
            desktop_control::VisibleThreadFailureState::NotFailed => {
                tracing::info!(
                    thread_id = %thread.id,
                    label = %decision.label,
                    "automatic recovery skipped because Codex UI does not show a failure or stopped marker"
                );
                return Ok(String::new());
            }
        }
    }

    if decision.delay_seconds > 0 {
        sleep(Duration::from_secs(decision.delay_seconds.min(5))).await;
    }

    let prompt = recovery_prompt(cfg, &decision);
    tracing::info!(
        thread_id = %thread.id,
        label = %decision.label,
        "sending recovery continue prompt"
    );
    let response = control_queue::submit_and_wait(control_queue::ControlAction::Continue {
        thread_id: thread.id.clone(),
        prompt,
    })?;
    counter.record(event_key);
    let turn = response.turn_id.unwrap_or_default();
    tracing::info!(
        thread_id = %thread.id,
        turn_id = %turn,
        label = %decision.label,
        "recovery continue prompt accepted"
    );
    Ok(format!(
        "已通过 Codex APP 可见窗口恢复\n原因：{}\n线程：{}\nturn：{}",
        decision.label, thread.id, turn
    ))
}

#[derive(Debug, Default)]
struct RecoveryCounter {
    count: u32,
    last_run: Option<Instant>,
    last_event_key: Option<String>,
}

impl RecoveryCounter {
    fn already_handled(&self, event_key: Option<&str>) -> bool {
        event_key.is_some_and(|key| self.last_event_key.as_deref() == Some(key))
    }

    fn can_run(&self, max: u32, cooldown_seconds: u64) -> bool {
        if self.count >= max {
            return false;
        }
        match self.last_run {
            Some(last) => last.elapsed() >= Duration::from_secs(cooldown_seconds),
            None => true,
        }
    }

    fn record(&mut self, event_key: Option<&str>) {
        self.count += 1;
        self.last_run = Some(Instant::now());
        self.last_event_key = event_key.map(str::to_string);
    }
}

fn format_watch_alert(candidate: &codex::ThreadRecovery) -> String {
    let source = truncate(&candidate.event.body, 900);
    format!(
        "发现可恢复线程\n{}\n{}\n\n原因：{}\n{}\n\n来源：\n{}\n\n打开机器人菜单选择线程，可查看最后反馈、一键继续或输入指令。",
        candidate.thread.title,
        candidate.thread.id,
        candidate.decision.label,
        candidate.decision.reason,
        source
    )
}

fn auto_enabled_for(cfg: &AppConfig, decision: &RecoveryDecision) -> bool {
    cfg.recovery.auto_recover
        && decision.auto_allowed
        && !matches!(decision.kind, RecoveryKind::None | RecoveryKind::Reauth)
}

fn max_recoveries_for(decision: &RecoveryDecision) -> u32 {
    match decision.kind {
        RecoveryKind::RetryLater => 10,
        RecoveryKind::RetrySoon => 8,
        RecoveryKind::ToolRetryWithDifferentPath => 5,
        RecoveryKind::SafetyRephrase => 3,
        RecoveryKind::ManualOnly | RecoveryKind::SwitchModel => 3,
        RecoveryKind::None | RecoveryKind::Reauth => 0,
    }
}

fn recovery_prompt(cfg: &AppConfig, decision: &RecoveryDecision) -> String {
    let prompt = match decision.kind {
        RecoveryKind::ToolRetryWithDifferentPath => &cfg.recovery.tool_failure_prompt,
        RecoveryKind::SafetyRephrase => &cfg.recovery.safety_rephrase_prompt,
        _ => &cfg.recovery.continue_prompt,
    };
    if decision.kind == RecoveryKind::SafetyRephrase {
        sanitized_recovery_text(prompt)
    } else {
        prompt.to_string()
    }
}

async fn notify_configured_chats(
    client: &Client,
    token: &str,
    chat_ids: &[i64],
    text: &str,
    keyboard: Option<Value>,
) -> Result<()> {
    if token.trim().is_empty() || chat_ids.is_empty() {
        tracing::debug!("watch notification skipped because telegram is not fully configured");
        return Ok(());
    }

    for chat_id in chat_ids {
        send_message(client, token, *chat_id, text, keyboard.clone()).await?;
    }
    Ok(())
}

async fn send_handled_reply(
    client: &Client,
    token: &str,
    chat_id: i64,
    reply: Result<BotReply>,
) -> Result<()> {
    let reply = match reply {
        Ok(reply) => reply,
        Err(err) => BotReply {
            text: format!("执行失败：{err:#}"),
            keyboard: Some(main_keyboard()),
        },
    };
    send_message(client, token, chat_id, &reply.text, reply.keyboard).await
}

async fn send_message(
    client: &Client,
    token: &str,
    chat_id: i64,
    text: &str,
    keyboard: Option<Value>,
) -> Result<()> {
    let mut body = json!({"chat_id": chat_id, "text": truncate(text, 3800)});
    if let Some(keyboard) = keyboard {
        body["reply_markup"] = keyboard;
    }
    let resp: TelegramResponse<Value> = client
        .post(api_url(token, "sendMessage")?)
        .json(&body)
        .send()
        .await
        .context("telegram sendMessage request failed")?
        .json()
        .await
        .context("telegram sendMessage response decode failed")?;
    if resp.ok {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            resp.description
                .unwrap_or_else(|| "telegram sendMessage returned ok=false".to_string())
        ))
    }
}

async fn answer_callback_query(client: &Client, token: &str, callback_id: &str) -> Result<()> {
    let resp: TelegramResponse<Value> = client
        .post(api_url(token, "answerCallbackQuery")?)
        .json(&json!({"callback_query_id": callback_id}))
        .send()
        .await
        .context("telegram answerCallbackQuery request failed")?
        .json()
        .await
        .context("telegram answerCallbackQuery response decode failed")?;
    if resp.ok {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            resp.description
                .unwrap_or_else(|| "telegram answerCallbackQuery returned ok=false".to_string())
        ))
    }
}

fn api_url(token: &str, method: &str) -> Result<Url> {
    Url::parse(&format!("https://api.telegram.org/bot{token}/{method}"))
        .context("failed to build telegram API URL")
}

fn redact_telegram_token(text: &str) -> String {
    let token = config::load_or_create()
        .map(|cfg| cfg.telegram.bot_token)
        .unwrap_or_default();
    if token.trim().is_empty() {
        return text.to_string();
    }
    text.replace(&format!("bot{}", token.trim()), "bot<redacted-token>")
}

fn is_pair_command(text: &str) -> bool {
    let mut parts = text.trim().split_whitespace();
    parts
        .next()
        .is_some_and(|command| normalize_command_token(command) == "/pair")
}

fn pair_text_matches(text: &str, code: &str) -> bool {
    let trimmed = text.trim();
    let code = code.trim();
    if trimmed.eq_ignore_ascii_case(code) {
        return true;
    }
    let mut parts = trimmed.split_whitespace();
    parts
        .next()
        .is_some_and(|command| normalize_command_token(command) == "/pair")
        && parts.any(|part| part.trim().eq_ignore_ascii_case(code))
}

fn normalize_command_token(command: &str) -> String {
    command
        .trim()
        .split_once('@')
        .map(|(base, _)| base)
        .unwrap_or(command.trim())
        .to_ascii_lowercase()
}

fn push_unique(ids: &mut Vec<i64>, id: i64) {
    if !ids.contains(&id) {
        ids.push(id);
        ids.sort_unstable();
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }

    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_group_commands_with_bot_suffix() {
        assert_eq!(normalize_command_token("/status@codexbot"), "/status");
        assert_eq!(normalize_command_token("/THREADS@CodexBot"), "/threads");
    }

    #[test]
    fn pair_command_matches_with_bot_suffix() {
        assert!(is_pair_command("/pair@codexbot 123456"));
        assert!(pair_text_matches("/pair@codexbot 123456", "123456"));
    }

    #[test]
    fn pair_text_matches_plain_code() {
        assert!(pair_text_matches("123456", "123456"));
    }
}
