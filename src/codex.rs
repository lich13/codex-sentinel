use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::System;

use crate::desktop_control;
use crate::recovery::{RecoveryDecision, RecoveryKind, classify_error};

const THREAD_RECOVERY_LOOKBACK_SECONDS: i64 = 600;
const THREAD_RECOVERY_MAX_LOOKBACK_SECONDS: i64 = 2 * 60 * 60;
const STATUS_LOG_LOOKBACK_SECONDS: i64 = 24 * 60 * 60;
const ROLLOUT_RECOVERY_SCAN_BYTES: u64 = 2 * 1024 * 1024;
const ROLLOUT_FEEDBACK_SCAN_BYTES: u64 = 1024 * 1024;
const ROLLOUT_MAX_PARSE_LINE_BYTES: usize = 512 * 1024;
const RECOVERY_LOG_TARGETS: &str = "(target IN ('codex_core::session::turn', 'codex_client::transport') \
          OR (target IN ('codex_otel.trace_safe', 'codex_otel.log_only') \
              AND feedback_log_body LIKE '%event.name=\"codex.sse_event\"%' \
              AND feedback_log_body LIKE '%error.message=%'))";
const RECOVERY_LOG_BODY: &str = "(feedback_log_body LIKE '%Turn error:%' \
          OR feedback_log_body LIKE '%Selected model is at capacity%' \
          OR feedback_log_body LIKE '%stream disconnected%' \
          OR feedback_log_body LIKE '%response_stream_disconnected%' \
          OR feedback_log_body LIKE '%error decoding response body%' \
          OR feedback_log_body LIKE '%possible cybersecurity risk%' \
          OR feedback_log_body LIKE '%Trusted Access for Cyber%')";
const RECOVERY_LOG_NOISE_FILTER: &str = "feedback_log_body NOT LIKE '%POST to http%' \
          AND feedback_log_body NOT LIKE '%\"instructions\"%' \
          AND feedback_log_body NOT LIKE '%\"input\"%' \
          AND feedback_log_body NOT LIKE '%event.name=\"codex.tool_result\"%' \
          AND feedback_log_body NOT LIKE '%tool_result%' \
          AND feedback_log_body NOT LIKE '%ToolCall:%' \
          AND feedback_log_body NOT LIKE '%response.function_call_arguments%' \
          AND feedback_log_body NOT LIKE '%response.output_item.done%' \
          AND feedback_log_body NOT LIKE '%feedback_log_body LIKE%'";
const THREAD_MATCH_FILTER_DYNAMIC: &str = "(thread_id = ? \
          OR feedback_log_body LIKE ? \
          OR feedback_log_body LIKE ? \
          OR feedback_log_body LIKE ?)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub updated_at: i64,
    pub rollout_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts: i64,
    pub level: String,
    pub target: String,
    pub thread_id: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelStatus {
    pub checked_at: DateTime<Utc>,
    pub codex_running: bool,
    pub recent_threads: Vec<ThreadSummary>,
    pub latest_turn_error: Option<LogEvent>,
    pub latest_model_error: Option<LogEvent>,
    pub latest_stream_retry: Option<LogEvent>,
    pub latest_tool_error: Option<LogEvent>,
    pub recovery: RecoveryDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRecovery {
    pub thread: ThreadSummary,
    pub event: LogEvent,
    pub decision: RecoveryDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadFeedback {
    pub thread_id: String,
    pub title: String,
    pub timestamp: Option<String>,
    pub text: String,
}

#[derive(Debug, Default)]
struct CodexProcessStatus {
    codex_running: bool,
}

#[derive(Debug, Default)]
struct RolloutRecoveryScan {
    recovery: Option<LogEvent>,
    normal_progress_ts: Option<i64>,
}

pub fn codex_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

pub fn state_db_path() -> PathBuf {
    codex_home().join("state_5.sqlite")
}

pub fn logs_db_path() -> PathBuf {
    codex_home().join("logs_2.sqlite")
}

fn codex_process_status() -> CodexProcessStatus {
    let mut sys = System::new_all();
    sys.refresh_all();
    let mut status = CodexProcessStatus::default();
    for process in sys.processes().values() {
        let cmd = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let name = process.name().to_string_lossy();
        if name == "Codex" || cmd.contains("/Applications/Codex.app") {
            status.codex_running = true;
        }
    }
    status
}

pub fn read_recent_threads(limit: usize) -> Result<Vec<ThreadSummary>> {
    let db_path = state_db_path();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let mut stmt = conn.prepare(
        "SELECT id, \
                title, cwd, updated_at, rollout_path \
         FROM threads ORDER BY updated_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(ThreadSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            cwd: row.get(2)?,
            updated_at: row.get(3)?,
            rollout_path: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn latest_log_like(where_clause: &str) -> Result<Option<LogEvent>> {
    let db_path = logs_db_path();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let sql = format!(
        "SELECT ts, level, target, thread_id, coalesce(feedback_log_body, '') \
         FROM logs WHERE ts >= ?1 AND ({where_clause}) \
         ORDER BY ts DESC, ts_nanos DESC, id DESC LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let since_ts = Utc::now()
        .timestamp()
        .saturating_sub(STATUS_LOG_LOOKBACK_SECONDS);
    let mut rows = stmt.query([since_ts])?;
    if let Some(row) = rows.next()? {
        Ok(Some(LogEvent {
            ts: row.get(0)?,
            level: row.get(1)?,
            target: row.get(2)?,
            thread_id: row.get(3)?,
            body: row.get::<_, String>(4)?.replace('\n', " "),
        }))
    } else {
        Ok(None)
    }
}

pub fn latest_recovery_log_for_hook(
    thread_id: Option<&str>,
    turn_id: Option<&str>,
    max_age_seconds: i64,
) -> Result<Option<LogEvent>> {
    latest_recovery_log_for_hook_from_db_with_fallback(
        &logs_db_path(),
        thread_id,
        turn_id,
        Utc::now().timestamp(),
        max_age_seconds,
    )
}

pub fn latest_recovery_log_for_thread(thread_id: &str, since_ts: i64) -> Result<Option<LogEvent>> {
    latest_recovery_log_for_thread_from_db(&logs_db_path(), thread_id, since_ts)
}

fn latest_recovery_log_for_thread_from_db(
    db_path: &Path,
    thread_id: &str,
    since_ts: i64,
) -> Result<Option<LogEvent>> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;

    let indexed_sql = format!(
        "SELECT ts, level, target, thread_id, coalesce(feedback_log_body, '') \
         FROM logs \
         WHERE {RECOVERY_LOG_TARGETS} \
           AND thread_id = ?1 \
           AND ts >= ?2 \
           AND {RECOVERY_LOG_BODY} \
           AND {RECOVERY_LOG_NOISE_FILTER} \
         ORDER BY ts DESC, ts_nanos DESC, id DESC \
         LIMIT 1"
    );
    if let Some(event) = query_latest_log_event(&conn, &indexed_sql, (thread_id, since_ts))? {
        return Ok(Some(event));
    }

    let body_sql = format!(
        "SELECT ts, level, target, thread_id, coalesce(feedback_log_body, '') \
         FROM logs \
         WHERE {RECOVERY_LOG_TARGETS} \
           AND (feedback_log_body LIKE ?1 \
                OR feedback_log_body LIKE ?2 \
                OR feedback_log_body LIKE ?3) \
           AND ts >= ?4 \
           AND {RECOVERY_LOG_BODY} \
           AND {RECOVERY_LOG_NOISE_FILTER} \
         ORDER BY ts DESC, ts_nanos DESC, id DESC \
         LIMIT 1"
    );
    let patterns = thread_match_patterns(thread_id);
    query_latest_log_event(
        &conn,
        &body_sql,
        (&patterns[0], &patterns[1], &patterns[2], since_ts),
    )
}

fn query_latest_log_event<P>(conn: &Connection, sql: &str, params: P) -> Result<Option<LogEvent>>
where
    P: rusqlite::Params,
{
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    if let Some(row) = rows.next()? {
        Ok(Some(LogEvent {
            ts: row.get(0)?,
            level: row.get(1)?,
            target: row.get(2)?,
            thread_id: row.get(3)?,
            body: row.get::<_, String>(4)?.replace('\n', " "),
        }))
    } else {
        Ok(None)
    }
}

pub fn recoverable_threads(limit: usize) -> Result<Vec<ThreadRecovery>> {
    let mut candidates = Vec::new();
    let now_ts = Utc::now().timestamp();
    for thread in read_recent_threads(limit)? {
        if thread.updated_at < now_ts.saturating_sub(THREAD_RECOVERY_MAX_LOOKBACK_SECONDS) {
            continue;
        }
        let Some(event) = latest_recovery_event_for_thread(&thread)? else {
            continue;
        };
        let decision = classify_error(&event.body);
        if decision.kind != RecoveryKind::None {
            candidates.push(ThreadRecovery {
                thread,
                event,
                decision,
            });
        }
    }
    Ok(candidates)
}

fn latest_recovery_event_for_thread(thread: &ThreadSummary) -> Result<Option<LogEvent>> {
    let since_ts = thread_recovery_since(thread);
    let mut log_event = latest_recovery_log_for_thread(&thread.id, since_ts)?;
    let rollout_scan = match scan_rollout_recovery(&thread.rollout_path, &thread.id, since_ts) {
        Ok(scan) => scan,
        Err(err) => {
            tracing::debug!(
                thread_id = %thread.id,
                rollout_path = %thread.rollout_path,
                "rollout recovery scan skipped: {err:#}"
            );
            RolloutRecoveryScan::default()
        }
    };

    if let (Some(progress_ts), Some(log)) = (rollout_scan.normal_progress_ts, log_event.as_ref()) {
        if log.ts <= progress_ts {
            log_event = None;
        }
    }

    Ok(match (log_event, rollout_scan.recovery) {
        (Some(log), Some(rollout)) => Some(if rollout.ts >= log.ts { rollout } else { log }),
        (Some(log), None) => Some(log),
        (None, Some(rollout)) => Some(rollout),
        (None, None) => None,
    })
}

fn thread_recovery_since(thread: &ThreadSummary) -> i64 {
    thread_recovery_since_at(thread, Utc::now().timestamp())
}

fn thread_recovery_since_at(thread: &ThreadSummary, now_ts: i64) -> i64 {
    thread
        .updated_at
        .saturating_sub(THREAD_RECOVERY_LOOKBACK_SECONDS)
        .max(now_ts.saturating_sub(THREAD_RECOVERY_MAX_LOOKBACK_SECONDS))
}

pub fn latest_thread_feedback(thread_id: &str) -> Result<ThreadFeedback> {
    let thread = read_recent_threads(50)?
        .into_iter()
        .find(|thread| thread.id == thread_id)
        .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
    latest_thread_feedback_for(&thread)
}

pub fn latest_thread_feedback_for(thread: &ThreadSummary) -> Result<ThreadFeedback> {
    if let Some((timestamp, text)) = latest_assistant_text_from_rollout_path(&thread.rollout_path)?
    {
        return Ok(ThreadFeedback {
            thread_id: thread.id.clone(),
            title: thread.title.clone(),
            timestamp,
            text,
        });
    }

    Ok(ThreadFeedback {
        thread_id: thread.id.clone(),
        title: thread.title.clone(),
        timestamp: None,
        text: "没有读取到 assistant 最后反馈。".to_string(),
    })
}

pub fn latest_assistant_text_from_rollout_path(
    path: impl AsRef<Path>,
) -> Result<Option<(Option<String>, String)>> {
    let path = path.as_ref();
    let lines = read_tail_lines(path, ROLLOUT_FEEDBACK_SCAN_BYTES)?;
    for line in lines.iter().rev() {
        if line.len() > ROLLOUT_MAX_PARSE_LINE_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(text) = assistant_message_text(&value) {
            return Ok(Some((
                value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                text,
            )));
        }
    }
    Ok(None)
}

#[cfg(test)]
fn latest_recovery_event_from_rollout_path(
    path: impl AsRef<Path>,
    thread_id: &str,
    since_ts: i64,
) -> Result<Option<LogEvent>> {
    Ok(scan_rollout_recovery(path, thread_id, since_ts)?.recovery)
}

fn scan_rollout_recovery(
    path: impl AsRef<Path>,
    thread_id: &str,
    since_ts: i64,
) -> Result<RolloutRecoveryScan> {
    let path = path.as_ref();
    let lines = read_tail_lines(path, ROLLOUT_RECOVERY_SCAN_BYTES)?;

    for line in lines.iter().rev() {
        if line.len() > ROLLOUT_MAX_PARSE_LINE_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = rollout_event_ts(&value).unwrap_or(0);
        if ts < since_ts {
            return Ok(RolloutRecoveryScan::default());
        }

        if let Some(body) = rollout_recovery_text(thread_id, &value) {
            let decision = classify_error(&body);
            if decision.kind != RecoveryKind::None {
                return Ok(RolloutRecoveryScan {
                    recovery: Some(LogEvent {
                        ts,
                        level: "WARN".to_string(),
                        target: "codex_sentinel::rollout".to_string(),
                        thread_id: Some(thread_id.to_string()),
                        body,
                    }),
                    normal_progress_ts: None,
                });
            }
        }

        if is_agent_or_assistant_message(&value) || is_successful_task_complete(&value) {
            return Ok(RolloutRecoveryScan {
                recovery: None,
                normal_progress_ts: Some(ts),
            });
        }

        if is_task_complete(&value) {
            return Ok(RolloutRecoveryScan::default());
        }
    }

    Ok(RolloutRecoveryScan::default())
}

#[cfg(test)]
fn latest_normal_progress_from_rollout(
    path: impl AsRef<Path>,
    since_ts: i64,
) -> Result<Option<i64>> {
    Ok(scan_rollout_recovery(path, "<thread>", since_ts)?.normal_progress_ts)
}

fn rollout_recovery_text(thread_id: &str, value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    let payload_type = payload.get("type").and_then(Value::as_str);

    if value.get("type").and_then(Value::as_str) == Some("event_msg")
        && payload_type == Some("task_complete")
    {
        let last_agent_message = payload.get("last_agent_message");
        if last_agent_message
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|text| !text.is_empty())
        {
            return None;
        }

        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        return Some(format!(
            "Silent turn completion: thread {thread_id} turn {turn_id} completed without a final assistant message"
        ));
    }

    if value.get("type").and_then(Value::as_str) == Some("event_msg") {
        match payload_type {
            Some("error") | Some("turn_error") => {
                return payload
                    .get("message")
                    .or_else(|| payload.get("error"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
            }
            _ => {}
        }
    }

    None
}

fn is_agent_or_assistant_message(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) == Some("response_item")
        && value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            == Some("message")
        && value
            .get("payload")
            .and_then(|payload| payload.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
    {
        return true;
    }

    value.get("type").and_then(Value::as_str) == Some("event_msg")
        && value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            == Some("agent_message")
}

fn is_task_complete(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("event_msg")
        && value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            == Some("task_complete")
}

fn is_successful_task_complete(value: &Value) -> bool {
    if !is_task_complete(value) {
        return false;
    }
    value
        .get("payload")
        .and_then(|payload| payload.get("last_agent_message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|text| !text.is_empty())
}

fn rollout_event_ts(value: &Value) -> Option<i64> {
    value
        .get("payload")
        .and_then(|payload| payload.get("completed_at"))
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|timestamp| {
                    DateTime::parse_from_rfc3339(timestamp)
                        .ok()
                        .map(|ts| ts.timestamp())
                })
        })
}

fn assistant_message_text(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if value.get("type").and_then(Value::as_str) == Some("response_item")
        && payload.get("type").and_then(Value::as_str) == Some("message")
        && payload.get("role").and_then(Value::as_str) == Some("assistant")
    {
        let parts = payload
            .get("content")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            return Some(parts.join("\n\n"));
        }
    }

    if value.get("type").and_then(Value::as_str) == Some("event_msg")
        && payload.get("type").and_then(Value::as_str) == Some("agent_message")
    {
        return payload
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
    }

    None
}

fn latest_recovery_log_for_hook_from_db(
    db_path: &Path,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
    now_ts: i64,
    max_age_seconds: i64,
) -> Result<Option<LogEvent>> {
    if thread_id.is_none() && turn_id.is_none() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;

    let mut clauses = vec![
        RECOVERY_LOG_TARGETS.to_string(),
        RECOVERY_LOG_BODY.to_string(),
        RECOVERY_LOG_NOISE_FILTER.to_string(),
        "ts >= ?".to_string(),
    ];
    let mut params = vec![now_ts.saturating_sub(max_age_seconds).to_string()];

    if let Some(thread_id) = thread_id {
        clauses.push(THREAD_MATCH_FILTER_DYNAMIC.to_string());
        params.push(thread_id.to_string());
        params.extend(thread_match_patterns(thread_id));
    }

    if let Some(turn_id) = turn_id {
        clauses.push("feedback_log_body LIKE ?".to_string());
        params.push(format!("%{turn_id}%"));
    }

    let sql = format!(
        "SELECT ts, level, target, thread_id, coalesce(feedback_log_body, '') \
         FROM logs WHERE {} ORDER BY ts DESC, ts_nanos DESC, id DESC LIMIT 1",
        clauses.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params.iter()))?;
    if let Some(row) = rows.next()? {
        Ok(Some(LogEvent {
            ts: row.get(0)?,
            level: row.get(1)?,
            target: row.get(2)?,
            thread_id: row.get(3)?,
            body: row.get::<_, String>(4)?.replace('\n', " "),
        }))
    } else {
        Ok(None)
    }
}

fn latest_recovery_log_for_hook_from_db_with_fallback(
    db_path: &Path,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
    now_ts: i64,
    max_age_seconds: i64,
) -> Result<Option<LogEvent>> {
    let strict =
        latest_recovery_log_for_hook_from_db(db_path, thread_id, turn_id, now_ts, max_age_seconds)?;
    if strict.is_some() || turn_id.is_none() || thread_id.is_none() {
        return Ok(strict);
    }

    latest_recovery_log_for_hook_from_db(db_path, thread_id, None, now_ts, max_age_seconds)
}

pub fn collect_status() -> Result<SentinelStatus> {
    let process_status = codex_process_status();
    let recent_threads = read_recent_threads(5).unwrap_or_default();
    let latest_turn_error = latest_log_like(&format!(
        "{RECOVERY_LOG_TARGETS} \
         AND (feedback_log_body LIKE '%Turn error:%' \
              OR feedback_log_body LIKE '%possible cybersecurity risk%' \
              OR feedback_log_body LIKE '%Trusted Access for Cyber%') \
         AND {RECOVERY_LOG_NOISE_FILTER}"
    ))?;
    let latest_model_error = latest_log_like(&format!(
        "{RECOVERY_LOG_TARGETS} \
         AND feedback_log_body LIKE '%Selected model is at capacity%' \
         AND {RECOVERY_LOG_NOISE_FILTER}"
    ))?;
    let latest_stream_retry = latest_log_like(&format!(
        "{RECOVERY_LOG_TARGETS} \
         AND (feedback_log_body LIKE '%stream disconnected%' \
              OR feedback_log_body LIKE '%response_stream_disconnected%' \
              OR feedback_log_body LIKE '%error decoding response body%') \
         AND {RECOVERY_LOG_NOISE_FILTER}"
    ))?;
    let latest_tool_error =
        latest_log_like("level='ERROR' AND target='codex_core::tools::router'")?;
    let active_thread = recent_threads.first();
    let active_recovery_source = active_thread
        .and_then(|thread| latest_recovery_event_for_thread(thread).ok())
        .flatten();
    let active_thread_id = active_thread.map(|thread| thread.id.as_str());
    let active_since = active_thread.map(thread_recovery_since).unwrap_or(0);
    let fallback_recovery_source = [
        latest_turn_error.as_ref(),
        latest_model_error.as_ref(),
        latest_tool_error.as_ref(),
        latest_stream_retry.as_ref(),
    ]
    .into_iter()
    .flatten()
    .filter(|event| event.thread_id.as_deref() == active_thread_id && event.ts >= active_since)
    .max_by_key(|event| event.ts);
    let recovery_source = active_recovery_source.as_ref().or(fallback_recovery_source);
    let recovery = recovery_source
        .map(|event| classify_error(&event.body))
        .unwrap_or_else(RecoveryDecision::none);

    Ok(SentinelStatus {
        checked_at: Utc::now(),
        codex_running: process_status.codex_running,
        recent_threads,
        latest_turn_error,
        latest_model_error,
        latest_stream_retry,
        latest_tool_error,
        recovery,
    })
}

fn thread_match_patterns(thread_id: &str) -> Vec<String> {
    vec![
        format!("%conversation.id={thread_id}%"),
        format!("%thread.id={thread_id}%"),
        format!("%thread_id={thread_id}%"),
    ]
}

fn read_tail_lines(path: &Path, max_bytes: u64) -> Result<Vec<String>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
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

pub fn continue_thread(thread_id: &str, prompt: &str) -> Result<String> {
    Ok(desktop_control::continue_thread_visible(thread_id, prompt)?.turn_id)
}

pub fn continue_thread_blocking(thread_id: &str, prompt: &str) -> Result<String> {
    continue_thread(thread_id, prompt)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;
    use serde_json::json;

    use super::*;

    fn temp_db_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("codex-sentinel-{name}-{nanos}.sqlite"))
    }

    fn create_logs_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
    }

    #[test]
    fn hook_log_lookup_finds_matching_turn_error() {
        let db = temp_db_path("hook-log-match");
        let conn = Connection::open(&db).expect("create temp db");
        create_logs_table(&conn);
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778293397, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["turn{turn.id=turn-a}:run_turn: Turn error: unexpected status 503 Service Unavailable: system memory overloaded"],
        )
        .expect("insert log");
        drop(conn);

        let event = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778293400,
            300,
        )
        .expect("lookup succeeds")
        .expect("event found");
        assert!(event.body.contains("503 Service Unavailable"));

        let missing_turn = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-b"),
            1778293400,
            300,
        )
        .expect("lookup succeeds");
        assert!(missing_turn.is_none());

        let stale = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778294000,
            300,
        )
        .expect("lookup succeeds");
        assert!(stale.is_none());

        let _ = fs::remove_file(db);
    }

    #[test]
    fn hook_log_lookup_falls_back_to_thread_when_turn_is_absent() {
        let db = temp_db_path("hook-log-thread-fallback");
        let conn = Connection::open(&db).expect("create temp db");
        create_logs_table(&conn);
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778293397, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["Turn error: unexpected status 503 Service Unavailable"],
        )
        .expect("insert log without turn id");
        drop(conn);

        let strict = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778293400,
            300,
        )
        .expect("strict lookup succeeds");
        assert!(strict.is_none());

        let fallback = latest_recovery_log_for_hook_from_db_with_fallback(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778293400,
            300,
        )
        .expect("fallback lookup succeeds")
        .expect("event found");
        assert!(fallback.body.contains("503 Service Unavailable"));

        let _ = fs::remove_file(db);
    }

    #[test]
    fn hook_log_lookup_falls_back_to_otel_body_thread_when_turn_is_absent() {
        let db = temp_db_path("hook-log-otel-body-thread-fallback");
        let conn = Connection::open(&db).expect("create temp db");
        create_logs_table(&conn);
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778293397, 1, 'INFO', 'codex_otel.trace_safe', NULL, ?1)",
            ["event.name=\"codex.sse_event\" event.kind=response.completed error.message=stream disconnected before completion: Transport error: network error: error decoding response body conversation.id=thread-a"],
        )
        .expect("insert otel log without thread column or turn id");
        drop(conn);

        let strict = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778293400,
            300,
        )
        .expect("strict lookup succeeds");
        assert!(strict.is_none());

        let fallback = latest_recovery_log_for_hook_from_db_with_fallback(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778293400,
            300,
        )
        .expect("fallback lookup succeeds")
        .expect("event found");
        assert!(fallback.body.contains("error decoding response body"));
        assert!(fallback.thread_id.is_none());

        let _ = fs::remove_file(db);
    }

    #[test]
    fn assistant_message_text_reads_latest_output_text() {
        let value = json!({
            "timestamp": "2026-05-09T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "最后反馈"}
                ]
            }
        });

        assert_eq!(assistant_message_text(&value).as_deref(), Some("最后反馈"));
    }

    #[test]
    fn rollout_lookup_finds_silent_task_completion() {
        let path = temp_db_path("rollout-silent").with_extension("jsonl");
        let raw = [
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:09:52.211Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-ok",
                    "last_agent_message": "已经完成。",
                    "completed_at": 1778346592
                }
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:25:10.023Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-silent",
                    "last_agent_message": null,
                    "completed_at": 1778347510
                }
            }))
            .unwrap(),
        ]
        .join("\n");
        fs::write(&path, raw).expect("write rollout");

        let event = latest_recovery_event_from_rollout_path(&path, "thread-a", 1778347000)
            .expect("lookup succeeds")
            .expect("event found");
        assert_eq!(event.target, "codex_sentinel::rollout");
        assert!(event.body.contains("Silent turn completion"));
        assert!(event.body.contains("turn-silent"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rollout_lookup_stops_after_latest_normal_agent_message() {
        let path = temp_db_path("rollout-normal-after-silent").with_extension("jsonl");
        let raw = [
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:25:10.023Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-silent",
                    "last_agent_message": null,
                    "completed_at": 1778347510
                }
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:26:10.023Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "我正在继续检查状态。",
                    "phase": "commentary"
                }
            }))
            .unwrap(),
        ]
        .join("\n");
        fs::write(&path, raw).expect("write rollout");

        let event = latest_recovery_event_from_rollout_path(&path, "thread-a", 1778347000)
            .expect("lookup succeeds");
        assert!(event.is_none());
        let progress = latest_normal_progress_from_rollout(&path, 1778347000)
            .expect("progress lookup succeeds");
        assert!(progress.is_some());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rollout_lookup_finds_agent_stream_disconnect() {
        let path = temp_db_path("rollout-stream").with_extension("jsonl");
        let raw = serde_json::to_string(&json!({
            "timestamp": "2026-05-09T17:25:10.023Z",
            "type": "event_msg",
            "payload": {
                "type": "error",
                "message": "stream disconnected before completion: Transport error: network error: error decoding response body",
                "phase": "commentary"
            }
        }))
        .unwrap();
        fs::write(&path, raw).expect("write rollout");

        let event = latest_recovery_event_from_rollout_path(&path, "thread-a", 1778347000)
            .expect("lookup succeeds")
            .expect("event found");
        assert!(event.body.contains("error decoding response body"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn thread_recovery_since_keeps_errors_before_thread_updated_at() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_328_734,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };

        assert_eq!(
            thread_recovery_since_at(&thread, 1_778_328_800),
            1_778_328_734 - THREAD_RECOVERY_LOOKBACK_SECONDS
        );
    }

    #[test]
    fn thread_recovery_log_lookup_finds_latest_recoverable_error() {
        let db = temp_db_path("thread-recovery-log-match");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778303650, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["turn{turn.id=turn-a}:run_turn: Turn error: unexpected status 503 Service Unavailable: system memory overloaded"],
        )
        .expect("insert old recoverable log");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (2, 1778303655, 2, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["turn{turn.id=turn-b}:run_turn: Turn error: exceeded retry limit, last status: 429 Too Many Requests"],
        )
        .expect("insert latest recoverable log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778303645)
            .expect("lookup succeeds")
            .expect("event found");
        assert!(event.body.contains("429 Too Many Requests"));

        let stale = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778303660)
            .expect("lookup succeeds");
        assert!(stale.is_none());

        let _ = fs::remove_file(db);
    }

    #[test]
    fn thread_recovery_log_lookup_finds_cyber_safety_rephrase() {
        let db = temp_db_path("thread-recovery-log-safety");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304000, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request."],
        )
        .expect("insert safety log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778303990)
            .expect("lookup succeeds")
            .expect("event found");
        assert!(event.body.contains("possible cybersecurity risk"));
        assert_eq!(
            classify_error(&event.body).kind,
            RecoveryKind::SafetyRephrase
        );

        let _ = fs::remove_file(db);
    }

    #[test]
    fn thread_recovery_log_lookup_finds_stream_body_decode_disconnect() {
        let db = temp_db_path("thread-recovery-log-stream-decode");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304100, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["stream disconnected before completion: Transport error: network error: error decoding response body"],
        )
        .expect("insert stream disconnect log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778304090)
            .expect("lookup succeeds")
            .expect("event found");
        assert!(event.body.contains("error decoding response body"));
        assert_eq!(classify_error(&event.body).kind, RecoveryKind::RetrySoon);

        let _ = fs::remove_file(db);
    }

    #[test]
    fn thread_recovery_log_lookup_matches_thread_id_embedded_in_body() {
        let db = temp_db_path("thread-recovery-log-body-thread");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304100, 1, 'INFO', 'codex_otel.log_only', '', ?1)",
            ["event.name=\"codex.sse_event\" event.kind=response.completed error.message=stream disconnected before completion: Transport error: network error: error decoding response body conversation.id=thread-a originator=Codex_Desktop"],
        )
        .expect("insert embedded thread log");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (2, 1778304105, 1, 'INFO', 'codex_otel.log_only', '', ?1)",
            ["event.name=\"codex.tool_result\" arguments={\"cmd\":\"sqlite3 logs WHERE conversation.id=thread-a AND feedback_log_body LIKE '%error decoding response body%'\"}"],
        )
        .expect("insert self-noise log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778304090)
            .expect("lookup succeeds")
            .expect("event found");
        assert_eq!(event.ts, 1778304100);
        assert!(event.body.contains("conversation.id=thread-a"));
        assert_eq!(classify_error(&event.body).kind, RecoveryKind::RetrySoon);

        let _ = fs::remove_file(db);
    }

    #[test]
    fn hook_log_lookup_falls_back_to_thread_id_embedded_in_body() {
        let db = temp_db_path("hook-log-body-thread");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304100, 1, 'INFO', 'codex_otel.trace_safe', '', ?1)",
            ["event.name=\"codex.sse_event\" event.kind=response.completed error.message=stream disconnected before completion: Transport error: network error: error decoding response body conversation.id=thread-a originator=Codex_Desktop"],
        )
        .expect("insert embedded thread hook log");
        drop(conn);

        let strict = latest_recovery_log_for_hook_from_db(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778304110,
            300,
        )
        .expect("strict lookup succeeds");
        assert!(strict.is_none());

        let fallback = latest_recovery_log_for_hook_from_db_with_fallback(
            &db,
            Some("thread-a"),
            Some("turn-a"),
            1778304110,
            300,
        )
        .expect("fallback lookup succeeds")
        .expect("event found");
        assert!(fallback.body.contains("conversation.id=thread-a"));
        assert_eq!(classify_error(&fallback.body).kind, RecoveryKind::RetrySoon);

        let _ = fs::remove_file(db);
    }

    #[test]
    fn thread_recovery_log_lookup_finds_otel_sse_body_thread_id() {
        let db = temp_db_path("thread-recovery-log-otel-sse-body-thread");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304100, 1, 'INFO', 'codex_otel.trace_safe', NULL, ?1)",
            ["event.name=\"codex.sse_event\" event.kind=response.completed error.message=stream disconnected before completion: Transport error: network error: error decoding response body conversation.id=thread-a app.version=0.130.0-alpha.5"],
        )
        .expect("insert otel stream disconnect log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778304090)
            .expect("lookup succeeds")
            .expect("event found");
        assert_eq!(event.target, "codex_otel.trace_safe");
        assert!(event.thread_id.is_none());
        assert!(event.body.contains("conversation.id=thread-a"));
        assert_eq!(classify_error(&event.body).kind, RecoveryKind::RetrySoon);

        let _ = fs::remove_file(db);
    }

    #[test]
    fn thread_recovery_log_lookup_ignores_sse_response_echo() {
        let db = temp_db_path("thread-recovery-log-sse-echo");
        let conn = Connection::open(&db).expect("create temp db");
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER,
                ts_nanos INTEGER,
                level TEXT,
                target TEXT,
                thread_id TEXT,
                feedback_log_body TEXT
            );",
        )
        .expect("create logs table");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778304100, 1, 'INFO', 'codex_api::sse::responses', NULL, ?1)",
            ["SSE event: {\"type\":\"response.completed\",\"response\":{\"instructions\":\"thread-a stream disconnected before completion: Transport error: network error: error decoding response body\"}}"],
        )
        .expect("insert sse response echo");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778304090)
            .expect("lookup succeeds");
        assert!(event.is_none());

        let _ = fs::remove_file(db);
    }
}
