use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{codex, config, lifecycle, maintenance};

const REQUESTS_FILE: &str = "control-requests.jsonl";
const RESPONSES_FILE: &str = "control-responses.jsonl";
const LOCK_FILE: &str = "control-queue.lock";
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(350);
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub id: String,
    pub created_at: i64,
    pub action: ControlAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlAction {
    Continue {
        thread_id: String,
        prompt: String,
    },
    NewThread {
        prompt: String,
        path: Option<String>,
    },
    ArchiveThread {
        thread_id: String,
    },
    ClearArchived,
}

impl ControlAction {
    fn is_immediate(&self) -> bool {
        matches!(
            self,
            ControlAction::ArchiveThread { .. } | ControlAction::ClearArchived
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    pub request_id: String,
    pub completed_at: i64,
    pub ok: bool,
    pub message: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub data: Option<Value>,
}

pub fn submit_and_wait(action: ControlAction) -> Result<ControlResponse> {
    if action.is_immediate() {
        return execute_action(&action);
    }
    lifecycle::ensure_control_worker_running_for_queue()?;
    let request = ControlRequest {
        id: request_id(),
        created_at: now_ts(),
        action,
    };
    append_json_line(&requests_path(), &request)?;
    wait_for_response(&request.id, DEFAULT_WAIT_TIMEOUT)
}

pub fn run_worker() -> Result<()> {
    run_worker_loop("process")
}

fn run_worker_loop(kind: &str) -> Result<()> {
    fs::create_dir_all(config::config_dir())?;
    tracing::info!(kind, "Codex Sentinel control worker started");
    loop {
        if !process_next_request()? {
            thread::sleep(WORKER_POLL_INTERVAL);
        }
    }
}

fn process_next_request() -> Result<bool> {
    fs::create_dir_all(config::config_dir())?;
    let lock_path = config::config_dir().join(LOCK_FILE);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock_file(&lock)?;
    let result = process_next_request_locked();
    unlock_file(&lock);
    result
}

fn process_next_request_locked() -> Result<bool> {
    let requests = read_json_lines::<ControlRequest>(&requests_path()).unwrap_or_default();
    let completed = read_json_lines::<ControlResponse>(&responses_path())
        .unwrap_or_default()
        .into_iter()
        .map(|response| response.request_id)
        .collect::<HashSet<_>>();
    let Some(request) = requests
        .into_iter()
        .find(|request| !completed.contains(&request.id))
    else {
        return Ok(false);
    };
    let response = execute_request(&request);
    append_json_line_locked(&responses_path(), &response)?;
    Ok(true)
}

fn execute_request(request: &ControlRequest) -> ControlResponse {
    match execute_action(&request.action) {
        Ok(mut response) => {
            response.request_id = request.id.clone();
            response
        }
        Err(err) => ControlResponse {
            request_id: request.id.clone(),
            completed_at: now_ts(),
            ok: false,
            message: format!("{err:#}"),
            thread_id: None,
            turn_id: None,
            data: None,
        },
    }
}

fn execute_action(action: &ControlAction) -> Result<ControlResponse> {
    match action {
        ControlAction::Continue { thread_id, prompt } => {
            let turn_id = codex::continue_thread(thread_id, prompt)?;
            Ok(ControlResponse {
                request_id: String::new(),
                completed_at: now_ts(),
                ok: true,
                message: "已在 Codex APP 内发送追加指令。".to_string(),
                thread_id: Some(thread_id.clone()),
                turn_id: Some(turn_id),
                data: None,
            })
        }
        ControlAction::NewThread { prompt, path } => {
            let result = codex::start_new_thread(prompt, path.as_deref())?;
            Ok(ControlResponse {
                request_id: String::new(),
                completed_at: now_ts(),
                ok: true,
                message: "已在 Codex APP 内创建新线程并发送指令。".to_string(),
                thread_id: result.thread_id,
                turn_id: Some(result.turn_id),
                data: Some(json!({ "transport": result.transport })),
            })
        }
        ControlAction::ArchiveThread { thread_id } => {
            let result = codex::archive_thread(thread_id)?;
            Ok(ControlResponse {
                request_id: String::new(),
                completed_at: now_ts(),
                ok: true,
                message: format!("已删除线程：{}", result.title),
                thread_id: Some(result.thread_id.clone()),
                turn_id: None,
                data: Some(serde_json::to_value(result)?),
            })
        }
        ControlAction::ClearArchived => {
            let result = codex::clear_archived_threads()?;
            Ok(ControlResponse {
                request_id: String::new(),
                completed_at: now_ts(),
                ok: true,
                message: format!("已清除 {} 条归档线程。", result.cleared_threads),
                thread_id: None,
                turn_id: None,
                data: Some(serde_json::to_value(result)?),
            })
        }
    }
}

fn wait_for_response(request_id: &str, timeout: Duration) -> Result<ControlResponse> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        for response in read_json_lines::<ControlResponse>(&responses_path())? {
            if response.request_id == request_id {
                if response.ok {
                    return Ok(response);
                }
                return Err(anyhow!(response.message));
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(anyhow!(
        "control-worker timed out waiting for request {request_id}"
    ))
}

fn read_json_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let mut parsed = Vec::new();
    let mut bad_lines = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(trimmed) {
            Ok(value) => parsed.push(value),
            Err(_) => bad_lines += 1,
        }
    }
    if bad_lines > 0 {
        tracing::debug!(
            path = %path.display(),
            bad_lines,
            "ignored malformed JSONL control queue lines"
        );
    }
    Ok(parsed)
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(config::config_dir())?;
    let lock_path = config::config_dir().join(LOCK_FILE);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock_file(&lock)?;
    let result = append_json_line_locked(path, value);
    unlock_file(&lock);
    result
}

fn append_json_line_locked<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(value)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    let max_lines = config::load_or_create()
        .map(|cfg| cfg.observability.control_queue_max_lines)
        .unwrap_or_else(|_| config::ObservabilityConfig::default().control_queue_max_lines);
    maintenance::trim_jsonl_file(path, max_lines)?;
    Ok(())
}

fn lock_file(file: &File) -> Result<()> {
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("failed to lock control queue")
    }
}

fn unlock_file(file: &File) {
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

fn request_id() -> String {
    let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("control-{millis}-{}-{seq}", std::process::id())
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn requests_path() -> PathBuf {
    config::config_dir().join(REQUESTS_FILE)
}

fn responses_path() -> PathBuf {
    config::config_dir().join(RESPONSES_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_tagged_actions() {
        let action = ControlAction::Continue {
            thread_id: "t".to_string(),
            prompt: "p".to_string(),
        };
        let raw = serde_json::to_string(&action).unwrap();
        assert!(raw.contains("\"kind\":\"continue\""));
    }
}
