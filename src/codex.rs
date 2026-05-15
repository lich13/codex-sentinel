use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::System;

use crate::app_server_probe::ThreadProbe;
use crate::recovery::{RecoveryDecision, RecoveryKind, classify_error};
use crate::{app_server_probe, desktop_control};

const THREAD_RECOVERY_LOOKBACK_SECONDS: i64 = 600;
const THREAD_RECOVERY_MAX_LOOKBACK_SECONDS: i64 = 2 * 60 * 60;
const THREAD_TERMINAL_QUIET_SECONDS: i64 = 30;
const THREAD_EVENT_UPDATED_AT_GRACE_SECONDS: i64 = 60;
const THREAD_RUNNING_LOG_WINDOW_SECONDS: i64 = 120;
const STATUS_LOG_LOOKBACK_SECONDS: i64 = 24 * 60 * 60;
const ROLLOUT_RECOVERY_SCAN_BYTES: u64 = 2 * 1024 * 1024;
const ROLLOUT_FEEDBACK_SCAN_BYTES: u64 = 1024 * 1024;
const ROLLOUT_MAX_PARSE_LINE_BYTES: usize = 512 * 1024;
const VISIBLE_SUBMIT_ATTEMPTS: usize = 4;
const VISIBLE_SUBMIT_WAIT: Duration = Duration::from_secs(5);
const NEW_THREAD_SUBMIT_WAIT: Duration = Duration::from_secs(15);
const VISIBLE_SUBMIT_FAST_PROBE_WAIT: Duration = Duration::from_millis(1_600);
const VISIBLE_SUBMIT_DB_WAIT: Duration = Duration::from_millis(2_400);
const VISIBLE_SUBMIT_PROBE_GRACE_SECONDS: i64 = 2;
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

#[derive(Debug, Clone)]
struct NewThreadCandidate {
    id: String,
    updated_at: i64,
    first_user_message: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningThread {
    pub thread: ThreadSummary,
    pub thread_status: Option<String>,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
    pub started_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMutationResult {
    pub thread_id: String,
    pub title: String,
    pub archived: bool,
    pub rollout_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearArchivedResult {
    pub cleared_threads: usize,
    pub moved_rollouts: usize,
    pub cleanup_dir: String,
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

pub fn codex_app_running() -> bool {
    codex_process_status().codex_running
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
         FROM threads \
         WHERE coalesce(archived, 0) = 0 \
         ORDER BY updated_at DESC LIMIT ?1",
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

fn read_thread_ids(limit: usize) -> Result<HashSet<String>> {
    let db_path = state_db_path();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let mut stmt = conn.prepare(
        "SELECT id \
         FROM threads \
         ORDER BY updated_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(Into::into)
}

fn read_thread(conn: &Connection, thread_id: &str) -> Result<ThreadSummary> {
    conn.query_row(
        "SELECT id, title, cwd, updated_at, rollout_path \
         FROM threads WHERE id = ?1",
        [thread_id],
        |row| {
            Ok(ThreadSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                cwd: row.get(2)?,
                updated_at: row.get(3)?,
                rollout_path: row.get(4)?,
            })
        },
    )
    .with_context(|| format!("thread not found: {thread_id}"))
}

pub fn archive_thread(thread_id: &str) -> Result<ThreadMutationResult> {
    let db_path = state_db_path();
    let mut conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let thread = read_thread(&conn, thread_id)?;
    let now = Utc::now().timestamp();
    let archived_rollout = archive_rollout_file(&thread.rollout_path)
        .unwrap_or_else(|_| PathBuf::from(&thread.rollout_path));
    let archived_rollout_text = archived_rollout.display().to_string();
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE threads \
         SET archived = 1, archived_at = ?1, rollout_path = ?2 \
         WHERE id = ?3",
        (&now, &archived_rollout_text, thread_id),
    )?;
    tx.commit()?;
    Ok(ThreadMutationResult {
        thread_id: thread.id,
        title: thread.title,
        archived: true,
        rollout_path: archived_rollout_text,
    })
}

pub fn clear_archived_threads() -> Result<ClearArchivedResult> {
    let db_path = state_db_path();
    let mut conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let archived = {
        let mut stmt = conn.prepare(
            "SELECT id, rollout_path FROM threads \
             WHERE coalesce(archived, 0) != 0 OR archived_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let trash_dir = user_trash_dir()?;

    let mut moved_rollouts = 0;
    for (_, rollout_path) in &archived {
        if move_file_to_dir_if_exists(Path::new(rollout_path), &trash_dir).is_ok() {
            moved_rollouts += 1;
        }
    }

    let tx = conn.transaction()?;
    for (thread_id, _) in &archived {
        tx.execute("DELETE FROM threads WHERE id = ?1", [thread_id])?;
    }
    tx.commit()?;

    Ok(ClearArchivedResult {
        cleared_threads: archived.len(),
        moved_rollouts,
        cleanup_dir: trash_dir.display().to_string(),
    })
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
    let recent_threads = read_recent_threads(limit)?;
    let thread_ids = recent_threads
        .iter()
        .map(|thread| thread.id.clone())
        .collect::<Vec<_>>();
    let app_server_probes =
        app_server_probe::read_thread_probes(&thread_ids).unwrap_or_else(|err| {
            tracing::debug!("Codex app-server probe skipped for recoverable scan: {err:#}");
            Default::default()
        });

    for thread in recent_threads {
        if thread.updated_at < now_ts.saturating_sub(THREAD_RECOVERY_MAX_LOOKBACK_SECONDS) {
            continue;
        }
        let app_server_probe = app_server_probes.get(&thread.id);
        let Some(event) = latest_recovery_event_for_thread(&thread, app_server_probe)? else {
            continue;
        };
        let decision = classify_error(&event.body);
        if decision.kind != RecoveryKind::None {
            if !recovery_event_is_terminal(&thread, &event, now_ts, app_server_probe)? {
                continue;
            }
            candidates.push(ThreadRecovery {
                thread,
                event,
                decision,
            });
        }
    }
    Ok(candidates)
}

pub fn running_threads(limit: usize) -> Result<Vec<RunningThread>> {
    let now_ts = Utc::now().timestamp();
    let running_activity_since = now_ts.saturating_sub(THREAD_RUNNING_LOG_WINDOW_SECONDS);
    let recent_threads = read_recent_threads(limit)?;
    let thread_ids = recent_threads
        .iter()
        .map(|thread| thread.id.clone())
        .collect::<Vec<_>>();
    let app_server_probes =
        app_server_probe::read_thread_probes(&thread_ids).unwrap_or_else(|err| {
            tracing::debug!("Codex app-server probe skipped for running thread scan: {err:#}");
            Default::default()
        });

    let mut running = Vec::new();
    for thread in recent_threads {
        let latest_running_activity_ts =
            match latest_running_log_activity_for_thread(&thread.id, running_activity_since) {
                Ok(activity) => activity,
                Err(err) => {
                    tracing::debug!(
                        thread_id = %thread.id,
                        "running thread log fallback skipped: {err:#}"
                    );
                    None
                }
            };
        if let Some(probe) = app_server_probes.get(&thread.id) {
            if let Some(item) = running_thread_from_probe_with_activity(
                thread,
                probe,
                latest_running_activity_ts,
                now_ts,
            ) {
                running.push(item);
            }
        } else if let Some(item) =
            running_thread_from_log_activity(thread, latest_running_activity_ts, now_ts)
        {
            running.push(item);
        }
    }
    Ok(running)
}

#[cfg(test)]
fn running_thread_from_probe(thread: ThreadSummary, probe: &ThreadProbe) -> Option<RunningThread> {
    running_thread_from_probe_with_activity(thread, probe, None, Utc::now().timestamp())
}

fn running_thread_from_probe_with_activity(
    thread: ThreadSummary,
    probe: &ThreadProbe,
    latest_running_activity_ts: Option<i64>,
    now_ts: i64,
) -> Option<RunningThread> {
    if probe.is_known_running() {
        return Some(RunningThread {
            thread,
            thread_status: probe.thread_status.clone(),
            turn_id: probe.latest_turn_id.clone(),
            turn_status: probe.latest_turn_status.clone(),
            started_at: probe.latest_turn_started_at,
        });
    }

    let activity_ts = latest_running_activity_ts?;
    if !fresh_running_log_activity(activity_ts, now_ts) {
        return None;
    }
    if !probe_has_unfinished_nonfailed_turn(probe) {
        return None;
    }
    if probe.latest_turn_started_at.is_some_and(|started_at| {
        activity_ts < started_at.saturating_sub(THREAD_EVENT_UPDATED_AT_GRACE_SECONDS)
    }) {
        return None;
    }

    Some(RunningThread {
        thread,
        thread_status: probe
            .thread_status
            .as_deref()
            .map(|status| format!("{status}+logActive"))
            .or_else(|| Some("logActive".to_string())),
        turn_id: probe.latest_turn_id.clone(),
        turn_status: probe
            .latest_turn_status
            .as_deref()
            .map(|status| format!("{status}+logActive"))
            .or_else(|| Some("logActive".to_string())),
        started_at: probe.latest_turn_started_at.or(Some(activity_ts)),
    })
}

fn running_thread_from_log_activity(
    thread: ThreadSummary,
    latest_running_activity_ts: Option<i64>,
    now_ts: i64,
) -> Option<RunningThread> {
    let activity_ts = latest_running_activity_ts?;
    if !fresh_running_log_activity(activity_ts, now_ts) {
        return None;
    }
    Some(RunningThread {
        thread,
        thread_status: Some("logActive".to_string()),
        turn_id: None,
        turn_status: Some("logActive".to_string()),
        started_at: Some(activity_ts),
    })
}

fn fresh_running_log_activity(activity_ts: i64, now_ts: i64) -> bool {
    now_ts.saturating_sub(activity_ts) <= THREAD_RUNNING_LOG_WINDOW_SECONDS
}

fn probe_has_unfinished_nonfailed_turn(probe: &ThreadProbe) -> bool {
    probe.latest_turn_id.is_some()
        && probe.latest_turn_completed_at.is_none()
        && !probe.has_terminal_failure()
}

fn latest_recovery_event_for_thread(
    thread: &ThreadSummary,
    app_server_probe: Option<&ThreadProbe>,
) -> Result<Option<LogEvent>> {
    let since_ts = thread_recovery_since(thread);
    let log_event = latest_recovery_log_for_thread(&thread.id, since_ts)?;
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

    Ok(select_latest_recovery_event(
        since_ts,
        log_event,
        rollout_scan,
        app_server_probe,
    ))
}

fn select_latest_recovery_event(
    since_ts: i64,
    mut log_event: Option<LogEvent>,
    rollout_scan: RolloutRecoveryScan,
    app_server_probe: Option<&ThreadProbe>,
) -> Option<LogEvent> {
    if let (Some(progress_ts), Some(log)) = (rollout_scan.normal_progress_ts, log_event.as_ref()) {
        if log.ts <= progress_ts {
            log_event = None;
        }
    }

    if let Some(probe) = app_server_probe {
        if probe.is_known_running()
            && app_server_probe_is_newer_than_event(probe, log_event.as_ref())
        {
            log_event = None;
        }
        if let Some(event) = app_server_probe_event(probe) {
            if event.ts >= since_ts {
                return Some(match log_event {
                    Some(log) if log.ts > event.ts => log,
                    _ => event,
                });
            }
        }
    }

    match (log_event, rollout_scan.recovery) {
        (Some(log), Some(rollout)) => Some(if rollout.ts >= log.ts { rollout } else { log }),
        (Some(log), None) => Some(log),
        (None, Some(rollout)) => Some(rollout),
        (None, None) => None,
    }
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

fn recovery_event_is_terminal(
    thread: &ThreadSummary,
    event: &LogEvent,
    now_ts: i64,
    app_server_probe: Option<&ThreadProbe>,
) -> Result<bool> {
    if let Some(probe) = app_server_probe {
        if probe.has_terminal_failure() {
            return Ok(true);
        }
        if probe.is_known_running() && app_server_probe_is_newer_than_event(probe, Some(event)) {
            return Ok(false);
        }
    }
    let latest_activity_ts =
        latest_log_activity_for_thread(&thread.id, event.ts.saturating_add(1))?;
    Ok(recovery_event_is_terminal_at(
        thread,
        event,
        latest_activity_ts,
        now_ts,
    ))
}

fn app_server_probe_is_newer_than_event(probe: &ThreadProbe, event: Option<&LogEvent>) -> bool {
    let Some(event) = event else {
        return true;
    };
    probe.latest_turn_ts().is_some_and(|ts| {
        ts >= event
            .ts
            .saturating_sub(THREAD_EVENT_UPDATED_AT_GRACE_SECONDS)
    })
}

fn app_server_probe_event(probe: &ThreadProbe) -> Option<LogEvent> {
    if !probe.has_terminal_failure() {
        return None;
    }
    let status = probe
        .latest_turn_status
        .as_deref()
        .or(probe.thread_status.as_deref())
        .unwrap_or("failed");
    let body = probe.latest_turn_error.clone().unwrap_or_else(|| {
        if probe.thread_status.as_deref() == Some("systemError") {
            "Codex app-server reported terminal thread status: systemError".to_string()
        } else {
            format!("Codex app-server reported terminal turn status: {status}")
        }
    });
    Some(LogEvent {
        ts: probe
            .latest_turn_ts()
            .unwrap_or_else(|| Utc::now().timestamp()),
        level: "ERROR".to_string(),
        target: probe.source.clone(),
        thread_id: Some(probe.thread_id.clone()),
        body,
    })
}

fn recovery_event_is_terminal_at(
    thread: &ThreadSummary,
    event: &LogEvent,
    latest_activity_ts: Option<i64>,
    now_ts: i64,
) -> bool {
    if event.ts > now_ts {
        return false;
    }
    if now_ts.saturating_sub(event.ts) < THREAD_TERMINAL_QUIET_SECONDS {
        return false;
    }
    if thread.updated_at
        > event
            .ts
            .saturating_add(THREAD_EVENT_UPDATED_AT_GRACE_SECONDS)
    {
        return false;
    }

    let Some(activity_ts) = latest_activity_ts else {
        return true;
    };
    if now_ts.saturating_sub(activity_ts) < THREAD_TERMINAL_QUIET_SECONDS {
        return false;
    }
    activity_ts
        <= event
            .ts
            .saturating_add(THREAD_EVENT_UPDATED_AT_GRACE_SECONDS)
}

fn latest_log_activity_for_thread(thread_id: &str, since_ts: i64) -> Result<Option<i64>> {
    latest_log_activity_for_thread_from_db(&logs_db_path(), thread_id, since_ts)
}

fn latest_running_log_activity_for_thread(thread_id: &str, since_ts: i64) -> Result<Option<i64>> {
    latest_running_log_activity_for_thread_from_db(&logs_db_path(), thread_id, since_ts)
}

fn latest_log_activity_for_thread_from_db(
    db_path: &Path,
    thread_id: &str,
    since_ts: i64,
) -> Result<Option<i64>> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let patterns = thread_match_patterns(thread_id);
    let sql = format!(
        "SELECT ts \
         FROM logs \
         WHERE ts >= ?1 \
           AND (thread_id = ?2 \
                OR ((thread_id IS NULL OR thread_id = '') \
                    AND (feedback_log_body LIKE ?3 \
                         OR feedback_log_body LIKE ?4 \
                         OR feedback_log_body LIKE ?5) \
                    AND {RECOVERY_LOG_NOISE_FILTER})) \
         ORDER BY ts DESC, ts_nanos DESC, id DESC \
         LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query((
        &since_ts,
        thread_id,
        &patterns[0],
        &patterns[1],
        &patterns[2],
    ))?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

fn latest_running_log_activity_for_thread_from_db(
    db_path: &Path,
    thread_id: &str,
    since_ts: i64,
) -> Result<Option<i64>> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let patterns = thread_match_patterns(thread_id);
    let sql = "SELECT ts \
         FROM logs \
         WHERE ts >= ?1 \
           AND (thread_id = ?2 \
                OR ((thread_id IS NULL OR thread_id = '') \
                    AND (feedback_log_body LIKE ?3 \
                         OR feedback_log_body LIKE ?4 \
                         OR feedback_log_body LIKE ?5))) \
           AND (feedback_log_body LIKE '%response.in_progress%' \
                OR feedback_log_body LIKE '%response.created%' \
                OR feedback_log_body LIKE '%response.output_item.added%' \
                OR feedback_log_body LIKE '%response.output_text.delta%' \
                OR feedback_log_body LIKE '%run_sampling_request%' \
                OR feedback_log_body LIKE '%codex.tool_call%' \
                OR feedback_log_body LIKE '%event.name=\"codex.sse_event\"%event.kind=response.output%') \
         ORDER BY ts DESC, ts_nanos DESC, id DESC \
         LIMIT 1";
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query((
        &since_ts,
        thread_id,
        &patterns[0],
        &patterns[1],
        &patterns[2],
    ))?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
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
        if let Some(text) =
            task_complete_feedback_text(&value).or_else(|| assistant_message_text(&value))
        {
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

fn latest_user_message_from_rollout_path(
    path: impl AsRef<Path>,
    since_ts: i64,
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
        let ts = rollout_event_ts(&value).unwrap_or(0);
        if ts < since_ts {
            continue;
        }
        if let Some(text) = user_message_text(&value) {
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

        if is_user_message(&value)
            || is_agent_or_assistant_message(&value)
            || is_successful_task_complete(&value)
        {
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

fn is_user_message(value: &Value) -> bool {
    let Some(payload) = value.get("payload") else {
        return false;
    };
    if value.get("type").and_then(Value::as_str) == Some("response_item")
        && payload.get("type").and_then(Value::as_str) == Some("message")
        && payload.get("role").and_then(Value::as_str) == Some("user")
    {
        return true;
    }

    value.get("type").and_then(Value::as_str) == Some("event_msg")
        && payload.get("type").and_then(Value::as_str) == Some("user_message")
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

fn user_message_text(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if value.get("type").and_then(Value::as_str) == Some("response_item")
        && payload.get("type").and_then(Value::as_str) == Some("message")
        && payload.get("role").and_then(Value::as_str) == Some("user")
    {
        let parts = payload
            .get("content")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            return Some(parts.join("\n\n"));
        }
    }

    if value.get("type").and_then(Value::as_str) == Some("event_msg")
        && payload.get("type").and_then(Value::as_str) == Some("user_message")
    {
        return payload
            .get("message")
            .or_else(|| payload.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string);
    }

    None
}

fn task_complete_feedback_text(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if value.get("type").and_then(Value::as_str) != Some("event_msg")
        || payload.get("type").and_then(Value::as_str) != Some("task_complete")
    {
        return None;
    }

    if let Some(text) = payload
        .get("last_agent_message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }

    if payload
        .get("last_agent_message")
        .is_some_and(Value::is_null)
    {
        let turn_id = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Some(format!(
            "最近一次 turn 已完成，但没有产生最终 assistant 反馈。\n\nturn_id: {turn_id}\n这通常表示 Codex 在安全拦截、异常中断或静默完成时终止；请查看恢复事件或继续一次确认实际状态。"
        ));
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
    let active_app_server_probe = active_thread
        .and_then(|thread| app_server_probe::read_thread_probe(&thread.id).ok())
        .flatten();
    let active_recovery_source = active_thread
        .and_then(|thread| {
            latest_recovery_event_for_thread(thread, active_app_server_probe.as_ref()).ok()
        })
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

fn archive_rollout_file(path: &str) -> Result<PathBuf> {
    let source = Path::new(path);
    if !source.exists() {
        return Ok(source.to_path_buf());
    }
    if source
        .components()
        .any(|component| component.as_os_str() == "archived_sessions")
    {
        return Ok(source.to_path_buf());
    }
    let archive_dir = codex_home().join("archived_sessions");
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("failed to create {}", archive_dir.display()))?;
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("rollout path has no file name: {}", source.display()))?;
    let dest = unique_dest(&archive_dir.join(file_name));
    fs::rename(source, &dest)
        .or_else(|_| {
            fs::copy(source, &dest)?;
            fs::remove_file(source)
        })
        .with_context(|| format!("failed to move rollout {} to archive", source.display()))?;
    Ok(dest)
}

fn user_trash_dir() -> Result<PathBuf> {
    let trash_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("failed to resolve user home directory"))?
        .join(".Trash");
    fs::create_dir_all(&trash_dir)
        .with_context(|| format!("failed to create {}", trash_dir.display()))?;
    Ok(trash_dir)
}

fn move_file_to_dir_if_exists(source: &Path, dest_dir: &Path) -> Result<()> {
    if !source.exists() {
        return Err(anyhow!("{} does not exist", source.display()));
    }
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("path has no file name: {}", source.display()))?;
    let dest = unique_dest(&dest_dir.join(file_name));
    fs::rename(source, &dest)
        .or_else(|_| {
            fs::copy(source, &dest)?;
            fs::remove_file(source)
        })
        .with_context(|| format!("failed to move {} to {}", source.display(), dest.display()))?;
    Ok(())
}

fn unique_dest(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let ext = path.extension().and_then(|value| value.to_str());
    for index in 1.. {
        let name = match ext {
            Some(ext) => format!("{stem}-{index}.{ext}"),
            None => format!("{stem}-{index}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

pub fn continue_thread(thread_id: &str, prompt: &str) -> Result<String> {
    let baseline = read_thread_submit_marker(thread_id)?;
    desktop_control::prepare_existing_thread_visible(thread_id)?;
    let submit_baseline = read_thread_submit_marker(thread_id).unwrap_or(baseline.clone());
    let submit_baseline_probe = app_server_probe::read_thread_probe(thread_id)
        .ok()
        .flatten();

    for attempt in 0..VISIBLE_SUBMIT_ATTEMPTS {
        let attempt_started_at = Utc::now().timestamp();
        let started = Instant::now();
        desktop_control::submit_prompt_to_visible_window(prompt, attempt)?;
        if wait_for_thread_submission(
            thread_id,
            submit_baseline.updated_at_ms,
            submit_baseline_probe.as_ref(),
            attempt_started_at,
            prompt,
            VISIBLE_SUBMIT_WAIT,
        )? {
            tracing::info!(
                thread_id,
                attempt,
                elapsed_ms = started.elapsed().as_millis(),
                "visible continue submission confirmed"
            );
            return Ok(desktop_control::visible_turn_id());
        }
        tracing::warn!(
            thread_id,
            attempt,
            elapsed_ms = started.elapsed().as_millis(),
            "visible continue submission attempt was not confirmed"
        );
    }

    Err(anyhow!(
        "未能确认 Codex APP 已将追加指令写入线程 {thread_id}。请回到 Codex 窗口确认输入框是否有残留内容后重试。"
    ))
}

fn wait_for_thread_submission(
    thread_id: &str,
    baseline_updated_at_ms: i64,
    baseline_probe: Option<&ThreadProbe>,
    attempt_started_at: i64,
    prompt: &str,
    timeout: Duration,
) -> Result<bool> {
    let fast_deadline = Instant::now() + VISIBLE_SUBMIT_FAST_PROBE_WAIT.min(timeout);
    while Instant::now() < fast_deadline {
        let probe = app_server_probe::read_thread_probe(thread_id)
            .ok()
            .flatten();
        if probe.as_ref().is_some_and(|probe| {
            thread_probe_confirms_submission(probe, baseline_probe, attempt_started_at)
        }) && thread_latest_user_prompt_matches(thread_id, prompt, attempt_started_at)?
        {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(180));
    }

    let db_deadline = Instant::now() + VISIBLE_SUBMIT_DB_WAIT.min(timeout);
    while Instant::now() < db_deadline {
        let current = read_thread_submit_marker(thread_id)?;
        if current.updated_at_ms > baseline_updated_at_ms
            && submit_marker_latest_user_prompt_matches(&current, prompt, attempt_started_at)
        {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(240));
    }

    let probe = app_server_probe::read_thread_probe(thread_id)
        .ok()
        .flatten();
    Ok(probe.as_ref().is_some_and(|probe| {
        thread_probe_confirms_submission(probe, baseline_probe, attempt_started_at)
    }) && thread_latest_user_prompt_matches(thread_id, prompt, attempt_started_at)?)
}

pub fn start_new_thread(
    prompt: &str,
    path: Option<&str>,
) -> Result<desktop_control::VisibleNewThreadResult> {
    let before = read_thread_ids(250).unwrap_or_default();
    let started_at = Utc::now().timestamp();
    desktop_control::prepare_new_thread_visible(path)?;
    let mut result = desktop_control::VisibleNewThreadResult {
        thread_id: None,
        turn_id: desktop_control::visible_turn_id(),
        transport: "visible_desktop".to_string(),
    };

    for attempt in 0..VISIBLE_SUBMIT_ATTEMPTS {
        desktop_control::submit_new_thread_prompt_to_visible_window(prompt, attempt)?;
        if let Some(thread) =
            wait_for_new_thread_match(&before, prompt, started_at, NEW_THREAD_SUBMIT_WAIT)?
        {
            result.thread_id = Some(thread.id);
            return Ok(result);
        }
    }

    Err(anyhow!(
        "未能确认 Codex APP 已创建新线程，本次新线程请求未返回成功。为避免把指令误发到旧线程，请切到 Codex APP 确认是否停留在新聊天页后重试。"
    ))
}

fn wait_for_new_thread_match(
    before: &HashSet<String>,
    prompt: &str,
    started_at: i64,
    timeout: Duration,
) -> Result<Option<NewThreadCandidate>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(thread) = read_new_thread_candidates(50)
            .unwrap_or_default()
            .into_iter()
            .find(|thread| {
                !before.contains(&thread.id)
                    && thread.updated_at >= started_at - 2
                    && prompts_match(&thread.first_user_message, prompt)
            })
        {
            return Ok(Some(thread));
        }
        std::thread::sleep(Duration::from_millis(280));
    }
    Ok(None)
}

fn thread_probe_confirms_submission(
    probe: &ThreadProbe,
    baseline: Option<&ThreadProbe>,
    attempt_started_at: i64,
) -> bool {
    let fresh_after_attempt =
        |ts: i64| ts >= attempt_started_at.saturating_sub(VISIBLE_SUBMIT_PROBE_GRACE_SECONDS);

    if let Some(baseline) = baseline {
        if probe.latest_turn_id.is_some() && probe.latest_turn_id != baseline.latest_turn_id {
            return true;
        }
        if let Some(ts) = probe.latest_turn_ts() {
            let baseline_ts = baseline.latest_turn_ts().unwrap_or(i64::MIN);
            if ts > baseline_ts && fresh_after_attempt(ts) {
                return true;
            }
            if probe.is_known_running() && !baseline.is_known_running() && fresh_after_attempt(ts) {
                return true;
            }
        }
        return false;
    }

    probe.latest_turn_ts().is_some_and(fresh_after_attempt)
}

#[derive(Debug, Clone)]
struct ThreadSubmitMarker {
    updated_at_ms: i64,
    rollout_path: String,
}

fn read_thread_submit_marker(thread_id: &str) -> Result<ThreadSubmitMarker> {
    let db_path = state_db_path();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    conn.query_row(
        "SELECT coalesce(updated_at_ms, updated_at * 1000) \
                , rollout_path \
         FROM threads WHERE id = ?1",
        [thread_id],
        |row| {
            Ok(ThreadSubmitMarker {
                updated_at_ms: row.get::<_, i64>(0)?,
                rollout_path: row.get(1)?,
            })
        },
    )
    .with_context(|| format!("thread not found: {thread_id}"))
}

fn read_new_thread_candidates(limit: usize) -> Result<Vec<NewThreadCandidate>> {
    let db_path = state_db_path();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    let mut stmt = conn.prepare(
        "SELECT id, updated_at, first_user_message \
         FROM threads \
         ORDER BY updated_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(NewThreadCandidate {
            id: row.get(0)?,
            updated_at: row.get(1)?,
            first_user_message: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn prompts_match(stored: &str, prompt: &str) -> bool {
    normalize_prompt(stored) == normalize_prompt(prompt)
}

fn thread_latest_user_prompt_matches(
    thread_id: &str,
    prompt: &str,
    attempt_started_at: i64,
) -> Result<bool> {
    let marker = read_thread_submit_marker(thread_id)?;
    Ok(submit_marker_latest_user_prompt_matches(
        &marker,
        prompt,
        attempt_started_at,
    ))
}

fn submit_marker_latest_user_prompt_matches(
    marker: &ThreadSubmitMarker,
    prompt: &str,
    attempt_started_at: i64,
) -> bool {
    let since_ts = attempt_started_at.saturating_sub(VISIBLE_SUBMIT_PROBE_GRACE_SECONDS);
    let latest = match latest_user_message_from_rollout_path(&marker.rollout_path, since_ts) {
        Ok(latest) => latest,
        Err(err) => {
            tracing::debug!(
                rollout_path = %marker.rollout_path,
                "visible continue prompt verification skipped while rollout is unreadable: {err:#}"
            );
            return false;
        }
    };
    let Some((_, actual)) = latest else {
        return false;
    };
    let matches = submitted_prompts_match(&actual, prompt);
    if !matches {
        tracing::warn!(
            actual_preview = %prompt_preview(&actual),
            expected_preview = %prompt_preview(prompt),
            "visible continue submission prompt mismatch"
        );
    }
    matches
}

fn submitted_prompts_match(actual: &str, prompt: &str) -> bool {
    strip_trailing_line_breaks(actual) == strip_trailing_line_breaks(prompt)
}

fn strip_trailing_line_breaks(value: &str) -> &str {
    value.trim_end_matches(['\n', '\r'])
}

fn prompt_preview(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(80).collect()
}

fn normalize_prompt(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Mutex, MutexGuard};
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

    fn probe_with_turn(turn_id: &str, status: &str, started_at: i64) -> ThreadProbe {
        ThreadProbe {
            thread_id: "thread-a".to_string(),
            thread_status: if status == "inProgress" {
                Some("active".to_string())
            } else {
                Some("notLoaded".to_string())
            },
            latest_turn_id: Some(turn_id.to_string()),
            latest_turn_status: Some(status.to_string()),
            latest_turn_error: None,
            latest_turn_started_at: Some(started_at),
            latest_turn_completed_at: None,
            source: "test".to_string(),
        }
    }

    fn thread_summary(id: &str) -> ThreadSummary {
        ThreadSummary {
            id: id.to_string(),
            title: "测试线程".to_string(),
            cwd: "/Users/gosu/Documents".to_string(),
            updated_at: 1778303650,
            rollout_path: "/tmp/thread.jsonl".to_string(),
        }
    }

    struct HomeEnvGuard {
        previous: Option<std::ffi::OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

    impl HomeEnvGuard {
        fn set(home: &Path) -> Self {
            let lock = HOME_ENV_LOCK.lock().expect("HOME env lock poisoned");
            let previous = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", home);
            }
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeEnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var("HOME", previous);
                } else {
                    std::env::remove_var("HOME");
                }
            }
        }
    }

    #[test]
    fn clear_archived_threads_moves_rollouts_to_user_trash() {
        let root = temp_db_path("clear-archived-trash-root");
        let _ = fs::remove_file(&root);
        fs::create_dir_all(&root).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".Trash")).unwrap();
        let _home = HomeEnvGuard::set(&home);

        let rollout = root.join("rollout.jsonl");
        fs::write(&rollout, "{}\n").unwrap();
        let db = home.join(".codex").join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                archived INTEGER,
                archived_at INTEGER
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, archived, archived_at)
             VALUES ('thread-a', ?1, 1, 1778303600)",
            [&rollout.display().to_string()],
        )
        .unwrap();
        drop(conn);

        let result = clear_archived_threads().unwrap();

        assert_eq!(result.cleared_threads, 1);
        assert_eq!(result.moved_rollouts, 1);
        assert_eq!(
            result.cleanup_dir,
            home.join(".Trash").display().to_string()
        );
        assert!(!rollout.exists());
        assert!(home.join(".Trash").join("rollout.jsonl").exists());
        assert!(
            !home
                .join(".codex-sentinel")
                .join("cleared-archived-rollouts")
                .exists()
        );

        let conn = Connection::open(&db).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn running_thread_from_probe_keeps_only_known_running_threads() {
        let thread = thread_summary("thread-a");
        let running_probe = ThreadProbe {
            thread_id: "thread-a".to_string(),
            thread_status: Some("active".to_string()),
            latest_turn_id: Some("turn-a".to_string()),
            latest_turn_status: Some("inProgress".to_string()),
            latest_turn_error: None,
            latest_turn_started_at: Some(1778303600),
            latest_turn_completed_at: None,
            source: "test".to_string(),
        };
        let running = running_thread_from_probe(thread.clone(), &running_probe)
            .expect("active app-server probe should be displayed as running");

        assert_eq!(running.thread.id, "thread-a");
        assert_eq!(running.turn_id.as_deref(), Some("turn-a"));
        assert_eq!(running.turn_status.as_deref(), Some("inProgress"));
        assert_eq!(running.started_at, Some(1778303600));

        let completed_probe = ThreadProbe {
            thread_id: "thread-a".to_string(),
            thread_status: Some("notLoaded".to_string()),
            latest_turn_id: Some("turn-b".to_string()),
            latest_turn_status: Some("completed".to_string()),
            latest_turn_error: None,
            latest_turn_started_at: Some(1778303500),
            latest_turn_completed_at: Some(1778303600),
            source: "test".to_string(),
        };
        assert!(running_thread_from_probe(thread, &completed_probe).is_none());
    }

    #[test]
    fn interrupted_unfinished_turn_with_fresh_running_log_is_displayed_as_running() {
        let thread = thread_summary("thread-a");
        let probe = ThreadProbe {
            thread_id: "thread-a".to_string(),
            thread_status: Some("notLoaded".to_string()),
            latest_turn_id: Some("turn-a".to_string()),
            latest_turn_status: Some("interrupted".to_string()),
            latest_turn_error: None,
            latest_turn_started_at: Some(1778303600),
            latest_turn_completed_at: None,
            source: "test".to_string(),
        };

        let running = running_thread_from_probe_with_activity(
            thread.clone(),
            &probe,
            Some(1778303655),
            1778303660,
        )
        .expect("fresh running log activity should override stale interrupted probe state");

        assert_eq!(running.thread.id, "thread-a");
        assert_eq!(running.turn_id.as_deref(), Some("turn-a"));
        assert_eq!(
            running.turn_status.as_deref(),
            Some("interrupted+logActive")
        );

        assert!(
            running_thread_from_probe_with_activity(
                thread.clone(),
                &probe,
                Some(1778303400),
                1778303660
            )
            .is_none(),
            "stale activity must not be shown as running"
        );

        let completed_probe = ThreadProbe {
            latest_turn_completed_at: Some(1778303660),
            ..probe
        };
        assert!(
            running_thread_from_probe_with_activity(
                thread,
                &completed_probe,
                Some(1778303655),
                1778303660
            )
            .is_none(),
            "completed turns must not be shown as running from log activity"
        );
    }

    #[test]
    fn visible_submit_confirmation_uses_short_fast_probe_window() {
        assert!(VISIBLE_SUBMIT_FAST_PROBE_WAIT <= Duration::from_secs(2));
        assert!(VISIBLE_SUBMIT_DB_WAIT <= Duration::from_secs(3));
        assert!(VISIBLE_SUBMIT_WAIT <= Duration::from_secs(5));
    }

    #[test]
    fn new_thread_confirmation_wait_covers_real_desktop_write_latency() {
        assert!(NEW_THREAD_SUBMIT_WAIT >= Duration::from_secs(12));
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
    fn latest_feedback_prefers_silent_task_completion_over_stale_message() {
        let path = temp_db_path("feedback-silent-task").with_extension("jsonl");
        let raw = [
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:26:08.291Z",
                "type": "event_msg",
                "payload": {
                    "type": "agent_message",
                    "message": "语法检查通过，继续验证。",
                    "phase": "commentary"
                }
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "timestamp": "2026-05-09T17:27:04.514Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_complete",
                    "turn_id": "turn-silent",
                    "last_agent_message": null,
                    "completed_at": 1778606824
                }
            }))
            .unwrap(),
        ]
        .join("\n");
        fs::write(&path, raw).expect("write rollout");

        let (_, text) = latest_assistant_text_from_rollout_path(&path)
            .expect("feedback lookup succeeds")
            .expect("feedback found");
        assert!(text.contains("没有产生最终 assistant 反馈"));
        assert!(text.contains("turn-silent"));
        assert!(!text.contains("语法检查通过"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn latest_feedback_reads_task_complete_final_message() {
        let value = json!({
            "timestamp": "2026-05-09T17:27:04.514Z",
            "type": "event_msg",
            "payload": {
                "type": "task_complete",
                "turn_id": "turn-ok",
                "last_agent_message": "最终反馈。",
                "completed_at": 1778606824
            }
        });

        assert_eq!(
            task_complete_feedback_text(&value).as_deref(),
            Some("最终反馈。")
        );
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
    fn completed_app_server_probe_does_not_suppress_silent_rollout_completion() {
        let probe = ThreadProbe {
            thread_id: "thread-a".to_string(),
            thread_status: Some("notLoaded".to_string()),
            latest_turn_id: Some("turn-silent".to_string()),
            latest_turn_status: Some("completed".to_string()),
            latest_turn_error: None,
            latest_turn_started_at: Some(1_778_567_000),
            latest_turn_completed_at: Some(1_778_567_161),
            source: "codex_app_server_thread_read".to_string(),
        };
        let rollout_event = LogEvent {
            ts: 1_778_567_161,
            level: "WARN".to_string(),
            target: "codex_sentinel::rollout".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Silent turn completion: thread thread-a turn turn-silent completed without a final assistant message".to_string(),
        };

        let selected = select_latest_recovery_event(
            1_778_567_000,
            None,
            RolloutRecoveryScan {
                recovery: Some(rollout_event),
                normal_progress_ts: None,
            },
            Some(&probe),
        )
        .expect("silent completion remains recoverable");

        assert_eq!(selected.target, "codex_sentinel::rollout");
        assert_eq!(classify_error(&selected.body).kind, RecoveryKind::RetrySoon);
    }

    #[test]
    fn submit_confirmation_accepts_new_app_server_turn() {
        let baseline = probe_with_turn("turn-old", "completed", 1_778_560_000);
        let current = probe_with_turn("turn-new", "inProgress", 1_778_560_050);

        assert!(thread_probe_confirms_submission(
            &current,
            Some(&baseline),
            1_778_560_049
        ));
    }

    #[test]
    fn submit_confirmation_rejects_stale_app_server_turn() {
        let baseline = probe_with_turn("turn-old", "completed", 1_778_560_000);
        let current = probe_with_turn("turn-old", "completed", 1_778_560_000);

        assert!(!thread_probe_confirms_submission(
            &current,
            Some(&baseline),
            1_778_560_049
        ));
    }

    #[test]
    fn visible_continue_rejects_thread_update_when_latest_user_prompt_is_polluted() {
        let root = temp_db_path("visible-continue-polluted-root");
        let _ = fs::remove_file(&root);
        fs::create_dir_all(&root).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let _home = HomeEnvGuard::set(&home);

        let rollout = root.join("rollout.jsonl");
        let raw = serde_json::to_string(&json!({
            "timestamp": "2026-05-14T18:08:56.960Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "式\n"}
                ]
            }
        }))
        .unwrap();
        fs::write(&rollout, raw).unwrap();

        let db = home.join(".codex").join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                updated_at INTEGER NOT NULL,
                updated_at_ms INTEGER,
                rollout_path TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, updated_at, updated_at_ms, rollout_path)
             VALUES ('thread-a', 1778782136, 1778782136960, ?1)",
            [&rollout.display().to_string()],
        )
        .unwrap();
        drop(conn);

        let confirmed = wait_for_thread_submission(
            "thread-a",
            1778782076000,
            None,
            1778782134,
            "把5.14 5.15两天的活动都落盘到活动库里，给我补推5.15的日报，以后日报标题格式改为5月15日玩卡薅羊毛活动这种格式",
            Duration::from_millis(10),
        )
        .unwrap();

        assert!(!confirmed);
        let _ = fs::remove_file(rollout);
        let _ = fs::remove_file(db);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn visible_continue_accepts_thread_update_when_latest_user_prompt_matches() {
        let root = temp_db_path("visible-continue-match-root");
        let _ = fs::remove_file(&root);
        fs::create_dir_all(&root).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let _home = HomeEnvGuard::set(&home);
        let prompt = "把5.14 5.15两天的活动都落盘到活动库里，给我补推5.15的日报，以后日报标题格式改为5月15日玩卡薅羊毛活动这种格式";

        let rollout = root.join("rollout.jsonl");
        let raw = serde_json::to_string(&json!({
            "timestamp": "2026-05-14T18:08:56.960Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": format!("{prompt}\n")
            }
        }))
        .unwrap();
        fs::write(&rollout, raw).unwrap();

        let db = home.join(".codex").join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                updated_at INTEGER NOT NULL,
                updated_at_ms INTEGER,
                rollout_path TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, updated_at, updated_at_ms, rollout_path)
             VALUES ('thread-a', 1778782136, 1778782136960, ?1)",
            [&rollout.display().to_string()],
        )
        .unwrap();
        drop(conn);

        let confirmed = wait_for_thread_submission(
            "thread-a",
            1778782076000,
            None,
            1778782134,
            prompt,
            Duration::from_millis(120),
        )
        .unwrap();

        assert!(confirmed);
        let _ = fs::remove_file(rollout);
        let _ = fs::remove_file(db);
        let _ = fs::remove_dir_all(root);
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
    fn terminal_gate_waits_for_quiet_period() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_303_650,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };
        let event = LogEvent {
            ts: 1_778_303_650,
            level: "INFO".to_string(),
            target: "codex_core::session::turn".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Turn error: exceeded retry limit, last status: 429 Too Many Requests"
                .to_string(),
        };

        assert!(!recovery_event_is_terminal_at(
            &thread,
            &event,
            None,
            1_778_303_670
        ));
        assert!(recovery_event_is_terminal_at(
            &thread,
            &event,
            None,
            1_778_303_681
        ));
    }

    #[test]
    fn terminal_gate_ignores_errors_followed_by_thread_progress() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_303_800,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };
        let event = LogEvent {
            ts: 1_778_303_650,
            level: "INFO".to_string(),
            target: "codex_core::session::turn".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Turn error: exceeded retry limit, last status: 429 Too Many Requests"
                .to_string(),
        };

        assert!(!recovery_event_is_terminal_at(
            &thread,
            &event,
            None,
            1_778_303_900
        ));
    }

    #[test]
    fn terminal_gate_ignores_errors_while_logs_are_still_active() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_303_650,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };
        let event = LogEvent {
            ts: 1_778_303_650,
            level: "INFO".to_string(),
            target: "codex_core::session::turn".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Turn error: exceeded retry limit, last status: 429 Too Many Requests"
                .to_string(),
        };

        assert!(!recovery_event_is_terminal_at(
            &thread,
            &event,
            Some(1_778_303_890),
            1_778_303_900
        ));
    }

    #[test]
    fn terminal_gate_ignores_errors_with_late_thread_activity() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_303_650,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };
        let event = LogEvent {
            ts: 1_778_303_650,
            level: "INFO".to_string(),
            target: "codex_core::session::turn".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Turn error: exceeded retry limit, last status: 429 Too Many Requests"
                .to_string(),
        };

        assert!(!recovery_event_is_terminal_at(
            &thread,
            &event,
            Some(1_778_303_730),
            1_778_303_900
        ));
    }

    #[test]
    fn terminal_gate_allows_stale_error_without_later_activity() {
        let thread = ThreadSummary {
            id: "thread-a".to_string(),
            title: "thread".to_string(),
            cwd: "/tmp".to_string(),
            updated_at: 1_778_303_650,
            rollout_path: "/tmp/rollout.jsonl".to_string(),
        };
        let event = LogEvent {
            ts: 1_778_303_650,
            level: "INFO".to_string(),
            target: "codex_core::session::turn".to_string(),
            thread_id: Some("thread-a".to_string()),
            body: "Turn error: exceeded retry limit, last status: 429 Too Many Requests"
                .to_string(),
        };

        assert!(recovery_event_is_terminal_at(
            &thread,
            &event,
            Some(1_778_303_660),
            1_778_303_900
        ));
    }

    #[test]
    fn latest_log_activity_matches_thread_column_or_otel_body() {
        let db = temp_db_path("latest-log-activity");
        let conn = Connection::open(&db).expect("create temp db");
        create_logs_table(&conn);
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778303650, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["turn activity"],
        )
        .expect("insert thread activity");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (2, 1778303660, 1, 'INFO', 'codex_otel.trace_safe', NULL, ?1)",
            ["event.name=\"codex.sse_event\" conversation.id=thread-a event.kind=response.output_text.delta"],
        )
        .expect("insert embedded thread activity");
        drop(conn);

        let activity = latest_log_activity_for_thread_from_db(&db, "thread-a", 1778303640)
            .expect("lookup succeeds");
        assert_eq!(activity, Some(1778303660));

        let stale = latest_log_activity_for_thread_from_db(&db, "thread-a", 1778303661)
            .expect("lookup succeeds");
        assert!(stale.is_none());

        let _ = fs::remove_file(db);
    }

    #[test]
    fn running_log_activity_matches_only_active_stream_events() {
        let db = temp_db_path("running-log-activity");
        let conn = Connection::open(&db).expect("create temp db");
        create_logs_table(&conn);
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (1, 1778303650, 1, 'INFO', 'codex_core::session::turn', 'thread-a', ?1)",
            ["Turn error: temporary gateway error"],
        )
        .expect("insert non-running noise");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (2, 1778303660, 1, 'INFO', 'codex_otel.log_only', NULL, ?1)",
            ["event.name=\"codex.sse_event\" event.kind=response.in_progress conversation.id=thread-a"],
        )
        .expect("insert running event");
        conn.execute(
            "INSERT INTO logs (id, ts, ts_nanos, level, target, thread_id, feedback_log_body)
             VALUES (3, 1778303670, 1, 'INFO', 'codex_core::session::turn', 'thread-b', ?1)",
            ["run_sampling_request{turn_id=turn-b}"],
        )
        .expect("insert other thread running event");
        drop(conn);

        let activity = latest_running_log_activity_for_thread_from_db(&db, "thread-a", 1778303640)
            .expect("lookup succeeds");
        assert_eq!(activity, Some(1778303660));

        let stale = latest_running_log_activity_for_thread_from_db(&db, "thread-a", 1778303661)
            .expect("lookup succeeds");
        assert!(stale.is_none());

        let _ = fs::remove_file(db);
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
    fn thread_recovery_log_lookup_finds_cyber_safety_blocked() {
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
            RecoveryKind::SafetyBlocked
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
    fn thread_recovery_log_lookup_finds_stream_disconnect_with_request_id() {
        let db = temp_db_path("thread-recovery-log-stream-request-id");
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
            ["stream disconnected before completion: An error occurred while processing your request. You can retry your request, or contact us through our help center at help.openai.com if the error persists. Please include the request ID 00000000-0000-4000-8000-000000000000 in your message."],
        )
        .expect("insert stream disconnect request-id log");
        drop(conn);

        let event = latest_recovery_log_for_thread_from_db(&db, "thread-a", 1778304090)
            .expect("lookup succeeds")
            .expect("event found");
        assert!(event.body.contains("help.openai.com"));
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
