use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const BUNDLED_CODEX_CLI: &str = "/Applications/Codex.app/Contents/Resources/codex";

#[derive(Debug, Clone, Default, Serialize)]
pub struct ThreadProbe {
    pub thread_id: String,
    pub thread_status: Option<String>,
    pub latest_turn_id: Option<String>,
    pub latest_turn_status: Option<String>,
    pub latest_turn_error: Option<String>,
    pub latest_turn_started_at: Option<i64>,
    pub latest_turn_completed_at: Option<i64>,
    pub source: String,
}

impl ThreadProbe {
    pub fn has_terminal_failure(&self) -> bool {
        self.thread_status.as_deref() == Some("systemError")
            || self.latest_turn_status.as_deref() == Some("failed")
            || self.latest_turn_error.is_some()
    }

    pub fn is_known_running(&self) -> bool {
        matches!(self.thread_status.as_deref(), Some("active"))
            || matches!(self.latest_turn_status.as_deref(), Some("inProgress"))
    }

    pub fn latest_turn_ts(&self) -> Option<i64> {
        self.latest_turn_completed_at
            .or(self.latest_turn_started_at)
    }
}

pub fn read_thread_probe(thread_id: &str) -> Result<Option<ThreadProbe>> {
    let probes = read_thread_probes(&[thread_id.to_string()])?;
    Ok(probes.get(thread_id).cloned())
}

pub fn read_thread_probes(thread_ids: &[String]) -> Result<HashMap<String, ThreadProbe>> {
    if thread_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut client = AppServerClient::start()?;
    client.initialize()?;

    let mut probes = HashMap::new();
    for thread_id in thread_ids {
        match client.read_thread(thread_id) {
            Ok(Some(probe)) => {
                probes.insert(thread_id.clone(), probe);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::debug!(
                    thread_id = %thread_id,
                    "codex app-server thread/read probe skipped: {err:#}"
                );
            }
        }
    }
    Ok(probes)
}

struct AppServerClient {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<String>,
    next_id: u64,
}

impl AppServerClient {
    fn start() -> Result<Self> {
        let mut child = Command::new(codex_cli_path())
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start Codex app-server probe")?;

        let stdout = child
            .stdout
            .take()
            .context("missing Codex app-server stdout")?;
        let stdin = child
            .stdin
            .take()
            .context("missing Codex app-server stdin")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            stdout_rx: rx,
            next_id: 1,
        })
    }

    fn initialize(&mut self) -> Result<()> {
        let id = self.next_request_id();
        let request = json!({
            "id": id,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "codex_sentinel",
                    "title": "Codex Sentinel",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": [
                        "item/agentMessage/delta",
                        "thread/tokenUsage/updated"
                    ]
                }
            }
        });
        self.write_json(&request)?;
        let response = self.wait_for_response(id, STARTUP_TIMEOUT)?;
        if let Some(error) = response.get("error") {
            return Err(anyhow!("Codex app-server initialize failed: {error}"));
        }
        self.write_json(&json!({"method": "initialized", "params": {}}))
    }

    fn read_thread(&mut self, thread_id: &str) -> Result<Option<ThreadProbe>> {
        let id = self.next_request_id();
        self.write_json(&json!({
            "id": id,
            "method": "thread/read",
            "params": {
                "threadId": thread_id,
                "includeTurns": true
            }
        }))?;
        let response = self.wait_for_response(id, REQUEST_TIMEOUT)?;
        if response.get("error").is_some() {
            return Ok(None);
        }
        let Some(thread) = response
            .get("result")
            .and_then(|result| result.get("thread"))
        else {
            return Ok(None);
        };
        Ok(Some(probe_from_thread_value(thread_id, thread)))
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn write_json(&mut self, value: &Value) -> Result<()> {
        writeln!(self.stdin, "{value}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn wait_for_response(&self, id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "Codex app-server timed out waiting for response {id}"
                ));
            }
            let line = self
                .stdout_rx
                .recv_timeout(remaining)
                .with_context(|| format!("Codex app-server closed before response {id}"))?;
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn codex_cli_path() -> PathBuf {
    let bundled = PathBuf::from(BUNDLED_CODEX_CLI);
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from("codex")
    }
}

fn probe_from_thread_value(thread_id: &str, thread: &Value) -> ThreadProbe {
    let latest_turn = thread
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.last());

    ThreadProbe {
        thread_id: thread
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(thread_id)
            .to_string(),
        thread_status: thread
            .get("status")
            .and_then(status_type)
            .map(str::to_string),
        latest_turn_id: latest_turn
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        latest_turn_status: latest_turn
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str)
            .map(str::to_string),
        latest_turn_error: latest_turn
            .and_then(|turn| turn.get("error"))
            .and_then(error_message),
        latest_turn_started_at: latest_turn
            .and_then(|turn| turn.get("startedAt"))
            .and_then(Value::as_i64),
        latest_turn_completed_at: latest_turn
            .and_then(|turn| turn.get("completedAt"))
            .and_then(Value::as_i64),
        source: "codex_app_server_thread_read".to_string(),
    }
}

fn status_type(status: &Value) -> Option<&str> {
    status
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| status.as_str())
}

fn error_message(error: &Value) -> Option<String> {
    if error.is_null() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        let message = message.trim();
        if !message.is_empty() {
            parts.push(message);
        }
    }
    if let Some(details) = error.get("additionalDetails").and_then(Value::as_str) {
        let details = details.trim();
        if !details.is_empty() {
            parts.push(details);
        }
    }
    if parts.is_empty() {
        Some(error.to_string())
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_failed_turn_probe() {
        let probe = probe_from_thread_value(
            "thread-a",
            &json!({
                "id": "thread-a",
                "status": {"type": "systemError"},
                "turns": [{
                    "id": "turn-a",
                    "status": "failed",
                    "error": {
                        "message": "Selected model is at capacity.",
                        "additionalDetails": "try again later"
                    },
                    "startedAt": 1778303650,
                    "completedAt": 1778303660
                }]
            }),
        );

        assert!(probe.has_terminal_failure());
        assert_eq!(probe.thread_status.as_deref(), Some("systemError"));
        assert_eq!(probe.latest_turn_status.as_deref(), Some("failed"));
        assert_eq!(probe.latest_turn_ts(), Some(1778303660));
        assert!(probe.latest_turn_error.unwrap().contains("capacity"));
    }

    #[test]
    fn completed_turn_is_not_treated_as_running_or_terminal() {
        let probe = probe_from_thread_value(
            "thread-a",
            &json!({
                "id": "thread-a",
                "status": {"type": "notLoaded"},
                "turns": [{
                    "id": "turn-a",
                    "status": "completed",
                    "error": null,
                    "startedAt": 1778303650,
                    "completedAt": 1778303660
                }]
            }),
        );

        assert!(!probe.has_terminal_failure());
        assert!(!probe.is_known_running());
    }

    #[test]
    fn active_turn_is_known_running() {
        let probe = probe_from_thread_value(
            "thread-a",
            &json!({
                "id": "thread-a",
                "status": {"type": "active", "activeFlags": []},
                "turns": [{
                    "id": "turn-a",
                    "status": "inProgress",
                    "error": null,
                    "startedAt": 1778303650,
                    "completedAt": null
                }]
            }),
        );

        assert!(!probe.has_terminal_failure());
        assert!(probe.is_known_running());
    }
}
