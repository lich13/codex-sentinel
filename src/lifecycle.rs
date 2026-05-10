use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sysinfo::System;

use crate::config;

const LABEL: &str = "local.codex-sentinel";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleStatus {
    pub codex_running: bool,
    pub sentinel_gui_running: bool,
    pub daemon_running: bool,
    pub lifecycle_running: bool,
    pub gui_pids: Vec<u32>,
    pub daemon_pids: Vec<u32>,
    pub lifecycle_pids: Vec<u32>,
    pub launch_agent_path: String,
}

#[derive(Debug)]
struct ProcessSnapshot {
    codex_running: bool,
    gui_pids: Vec<u32>,
    daemon_pids: Vec<u32>,
    lifecycle_pids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentinelRole {
    Gui,
    Daemon,
    Lifecycle,
    Other,
}

pub fn run_lifecycle() -> Result<()> {
    fs::create_dir_all(config::config_dir())?;
    tracing::info!("Codex Sentinel lifecycle helper started");

    loop {
        let snapshot = inspect_processes()?;
        if snapshot.codex_running {
            ensure_gui_running(&snapshot)?;
            ensure_daemon_running(&snapshot)?;
        } else {
            stop_followed_processes(&snapshot)?;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn status() -> Result<LifecycleStatus> {
    let snapshot = inspect_processes()?;
    Ok(LifecycleStatus {
        codex_running: snapshot.codex_running,
        sentinel_gui_running: !snapshot.gui_pids.is_empty(),
        daemon_running: !snapshot.daemon_pids.is_empty(),
        lifecycle_running: !snapshot.lifecycle_pids.is_empty(),
        gui_pids: snapshot.gui_pids,
        daemon_pids: snapshot.daemon_pids,
        lifecycle_pids: snapshot.lifecycle_pids,
        launch_agent_path: launch_agent_path().display().to_string(),
    })
}

pub fn install_launch_agent() -> Result<PathBuf> {
    let exe = current_exe()?;
    let path = launch_agent_path();
    let stdout = config::config_dir().join("lifecycle.out.log");
    let stderr = config::config_dir().join("lifecycle.err.log");
    fs::create_dir_all(config::config_dir())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&path, launch_agent_plist(&exe, &stdout, &stderr).as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    reload_launch_agent(&path)?;
    Ok(path)
}

fn ensure_gui_running(snapshot: &ProcessSnapshot) -> Result<()> {
    if !snapshot.gui_pids.is_empty() {
        return Ok(());
    }
    let app = app_bundle_path()?;
    tracing::info!(app = %app.display(), "starting Codex Sentinel GUI because Codex is running");
    let status = Command::new("open")
        .arg(&app)
        .status()
        .with_context(|| format!("failed to open {}", app.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("open {} failed with {status}", app.display()))
    }
}

fn ensure_daemon_running(snapshot: &ProcessSnapshot) -> Result<()> {
    if !snapshot.daemon_pids.is_empty() {
        return Ok(());
    }
    let exe = current_exe()?;
    let log_path = config::config_dir().join("telegram-daemon.out.log");
    let err_path = config::config_dir().join("telegram-daemon.err.log");
    fs::create_dir_all(config::config_dir())?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&err_path)?;
    let child = Command::new(&exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("failed to start daemon from {}", exe.display()))?;
    tracing::info!(
        pid = child.id(),
        "started Codex Sentinel daemon because Codex is running"
    );
    Ok(())
}

fn stop_followed_processes(snapshot: &ProcessSnapshot) -> Result<()> {
    for pid in snapshot
        .gui_pids
        .iter()
        .chain(snapshot.daemon_pids.iter())
        .copied()
    {
        tracing::info!(
            pid,
            "stopping Codex Sentinel process because Codex is not running"
        );
        terminate_pid(pid)?;
    }
    Ok(())
}

fn terminate_pid(pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .with_context(|| format!("failed to terminate pid {pid}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("kill -TERM {pid} failed with {status}"))
    }
}

fn inspect_processes() -> Result<ProcessSnapshot> {
    let own_exe = current_exe()?;
    let current_pid = std::process::id();
    let mut sys = System::new_all();
    sys.refresh_all();

    let mut snapshot = ProcessSnapshot {
        codex_running: false,
        gui_pids: Vec::new(),
        daemon_pids: Vec::new(),
        lifecycle_pids: Vec::new(),
    };

    for process in sys.processes().values() {
        let cmd = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let cmd_text = cmd.join(" ");
        let name = process.name().to_string_lossy();
        let pid = process.pid().as_u32();

        if is_codex_app_process(&name, &cmd_text) {
            snapshot.codex_running = true;
            continue;
        }

        if pid == current_pid || !is_current_binary(process.exe(), &cmd, &own_exe) {
            continue;
        }

        match sentinel_role(&cmd) {
            SentinelRole::Gui => snapshot.gui_pids.push(pid),
            SentinelRole::Daemon => snapshot.daemon_pids.push(pid),
            SentinelRole::Lifecycle => snapshot.lifecycle_pids.push(pid),
            SentinelRole::Other => {}
        }
    }

    snapshot.gui_pids.sort_unstable();
    snapshot.daemon_pids.sort_unstable();
    snapshot.lifecycle_pids.sort_unstable();
    Ok(snapshot)
}

fn is_codex_app_process(name: &str, cmd: &str) -> bool {
    name == "Codex" && cmd.contains("Codex.app/Contents/MacOS/Codex")
}

fn is_current_binary(exe: Option<&Path>, cmd: &[String], own_exe: &Path) -> bool {
    if exe.is_some_and(|path| same_path(path, own_exe)) {
        return true;
    }
    cmd.first()
        .map(Path::new)
        .is_some_and(|path| same_path(path, own_exe))
}

fn sentinel_role(cmd: &[String]) -> SentinelRole {
    if has_mode_arg(cmd, "daemon") || has_mode_arg(cmd, "--daemon") {
        return SentinelRole::Daemon;
    }
    if has_mode_arg(cmd, "lifecycle") || has_mode_arg(cmd, "--lifecycle") {
        return SentinelRole::Lifecycle;
    }
    if cmd.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "status"
                | "--status"
                | "recoverable"
                | "--recoverable"
                | "continue"
                | "--continue"
                | "desktop-control-status"
                | "--desktop-control-status"
                | "open-desktop-permissions"
                | "--open-desktop-permissions"
                | "hook-status"
                | "--hook-status"
                | "install-hooks"
                | "--install-hooks"
                | "hook-stop"
                | "--hook-stop"
                | "config"
                | "--config"
                | "install-launch-agent"
                | "--install-launch-agent"
                | "lifecycle-status"
                | "--lifecycle-status"
                | "help"
                | "--help"
        )
    }) {
        return SentinelRole::Other;
    }
    SentinelRole::Gui
}

fn has_mode_arg(cmd: &[String], mode: &str) -> bool {
    cmd.iter().skip(1).any(|arg| arg == mode)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("failed to resolve current executable")
}

fn app_bundle_path() -> Result<PathBuf> {
    let exe = current_exe()?;
    let macos = exe
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve executable parent"))?;
    let contents = macos
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve app Contents directory"))?;
    let app = contents
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve app bundle directory"))?;
    if app.extension().and_then(|ext| ext.to_str()) == Some("app") {
        Ok(app.to_path_buf())
    } else {
        Ok(PathBuf::from("/Applications/Codex Sentinel.app"))
    }
}

fn launch_agent_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn launch_agent_plist(exe: &Path, stdout: &Path, stderr: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>lifecycle</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>
"#,
        label = LABEL,
        exe = xml_escape(&exe.display().to_string()),
        stdout = xml_escape(&stdout.display().to_string()),
        stderr = xml_escape(&stderr.display().to_string()),
    )
}

fn reload_launch_agent(path: &Path) -> Result<()> {
    let domain = format!("gui/{}", current_uid()?);
    let _ = Command::new("launchctl")
        .args(["bootout", &domain])
        .arg(path)
        .status();

    let status = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(path)
        .status()
        .with_context(|| format!("failed to bootstrap {}", path.display()))?;
    if !status.success() {
        return Err(anyhow!(
            "launchctl bootstrap {} {} failed with {status}",
            domain,
            path.display()
        ));
    }

    let service = format!("{domain}/{LABEL}");
    let status = Command::new("launchctl")
        .args(["kickstart", "-k", &service])
        .status()
        .with_context(|| format!("failed to kickstart {service}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "launchctl kickstart {service} failed with {status}"
        ))
    }
}

fn current_uid() -> Result<String> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err(anyhow!("id -u failed with {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn classifies_sentinel_roles() {
        assert_eq!(sentinel_role(&cmd(&["codex-sentinel"])), SentinelRole::Gui);
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "daemon"])),
            SentinelRole::Daemon
        );
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "lifecycle"])),
            SentinelRole::Lifecycle
        );
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "hook-stop"])),
            SentinelRole::Other
        );
    }

    #[test]
    fn detects_codex_app_not_helpers() {
        assert!(is_codex_app_process(
            "Codex",
            "/Applications/Codex.app/Contents/MacOS/Codex"
        ));
        assert!(!is_codex_app_process(
            "Codex Helper",
            "/Applications/Codex.app/Contents/Frameworks/Codex Helper.app/Contents/MacOS/Codex Helper"
        ));
    }
}
