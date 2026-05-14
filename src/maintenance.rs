use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::config::{self, AppConfig};

const LOG_TRIM_KEEP_RATIO_DIVISOR: u64 = 2;

pub fn trim_jsonl_file(path: &Path, max_lines: usize) -> Result<()> {
    if max_lines == 0 || !path.exists() {
        return Ok(());
    }

    let file = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let lines = BufReader::new(file)
        .lines()
        .collect::<std::io::Result<Vec<_>>>()?;
    if lines.len() <= max_lines {
        return Ok(());
    }

    let keep_from = lines.len().saturating_sub(max_lines);
    let mut next = lines[keep_from..].join("\n");
    next.push('\n');
    fs::write(path, next).with_context(|| format!("failed to trim {}", path.display()))?;
    Ok(())
}

pub fn trim_log_file(path: &Path, max_bytes: u64) -> Result<()> {
    if max_bytes == 0 || !path.exists() {
        return Ok(());
    }

    let len = fs::metadata(path)?.len();
    if len <= max_bytes {
        return Ok(());
    }

    let keep_bytes = (max_bytes / LOG_TRIM_KEEP_RATIO_DIVISOR).max(1);
    let mut file =
        File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let start = len.saturating_sub(keep_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(keep_bytes as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    }

    let marker = format!(
        "[{}] Codex Sentinel trimmed this log to keep runtime files bounded.\n",
        Utc::now().to_rfc3339()
    );
    let mut next = marker.into_bytes();
    next.extend(bytes);
    fs::write(path, next).with_context(|| format!("failed to trim {}", path.display()))?;
    Ok(())
}

pub fn trim_runtime_files(cfg: &AppConfig) -> Result<()> {
    for path in managed_log_paths() {
        trim_log_file(&path, cfg.observability.log_max_bytes)?;
    }
    Ok(())
}

pub fn managed_log_paths() -> Vec<PathBuf> {
    let dir = config::config_dir();
    [
        "telegram-daemon.out.log",
        "telegram-daemon.err.log",
        "control-worker.out.log",
        "control-worker.err.log",
        "lifecycle.out.log",
        "lifecycle.err.log",
        "gui.out.log",
        "gui.err.log",
    ]
    .into_iter()
    .map(|name| dir.join(name))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_jsonl_file_keeps_latest_lines() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-trim-jsonl-{}.jsonl",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();

        trim_jsonl_file(&path, 2).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw, "three\nfour\n");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn trim_log_file_keeps_tail_and_marker() {
        let path = std::env::temp_dir().join(format!(
            "codex-sentinel-trim-log-{}.log",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::write(&path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        trim_log_file(&path, 24).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("Codex Sentinel trimmed this log"));
        assert!(raw.contains("line 5"));
        assert!(!raw.contains("line 1"));
        let _ = fs::remove_file(path);
    }
}
