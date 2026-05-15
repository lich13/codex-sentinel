use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sysinfo::{ProcessStatus, System};

use crate::{config, maintenance};

const LABEL: &str = "local.codex-sentinel";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleStatus {
    pub codex_running: bool,
    pub sentinel_gui_running: bool,
    pub daemon_running: bool,
    pub control_worker_running: bool,
    pub lifecycle_running: bool,
    pub gui_pids: Vec<u32>,
    pub daemon_pids: Vec<u32>,
    pub control_worker_pids: Vec<u32>,
    pub lifecycle_pids: Vec<u32>,
    pub launch_agent_path: String,
}

#[derive(Debug)]
struct ProcessSnapshot {
    codex_running: bool,
    gui_pids: Vec<u32>,
    daemon_pids: Vec<u32>,
    control_worker_pids: Vec<u32>,
    lifecycle_pids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentinelRole {
    Gui,
    Daemon,
    ControlWorker,
    Lifecycle,
    Other,
}

pub fn run_lifecycle() -> Result<()> {
    fs::create_dir_all(config::config_dir())?;
    tracing::info!("Codex Sentinel lifecycle helper started");

    loop {
        trim_runtime_files_once();
        let snapshot = inspect_processes()?;
        if snapshot.codex_running {
            ensure_gui_running(&snapshot)?;
            ensure_daemon_running(&snapshot)?;
            ensure_control_worker_running(&snapshot)?;
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
        control_worker_running: !snapshot.control_worker_pids.is_empty(),
        lifecycle_running: !snapshot.lifecycle_pids.is_empty(),
        gui_pids: snapshot.gui_pids,
        daemon_pids: snapshot.daemon_pids,
        control_worker_pids: snapshot.control_worker_pids,
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

pub fn shutdown_followed_processes() -> Result<()> {
    unload_launch_agent();
    let snapshot = inspect_processes()?;
    stop_followed_processes(&snapshot)
}

pub fn ensure_control_worker_running_for_queue() -> Result<()> {
    let snapshot = inspect_processes()?;
    if !snapshot.codex_running {
        return Err(anyhow!(
            "Codex APP 未运行，无法通过可见窗口处理控制队列。请先打开 Codex APP。"
        ));
    }
    ensure_control_worker_running(&snapshot)
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

fn ensure_control_worker_running(snapshot: &ProcessSnapshot) -> Result<()> {
    if !snapshot.control_worker_pids.is_empty() {
        return Ok(());
    }
    spawn_role_process(
        "control-worker",
        &config::config_dir().join("control-worker.out.log"),
        &config::config_dir().join("control-worker.err.log"),
        "started Codex Sentinel control worker because Codex is running",
    )
}

fn ensure_daemon_running(snapshot: &ProcessSnapshot) -> Result<()> {
    if !snapshot.daemon_pids.is_empty() {
        return Ok(());
    }
    spawn_role_process(
        "daemon",
        &config::config_dir().join("telegram-daemon.out.log"),
        &config::config_dir().join("telegram-daemon.err.log"),
        "started Codex Sentinel daemon because Codex is running",
    )
}

fn spawn_role_process(
    role: &str,
    stdout_path: &Path,
    stderr_path: &Path,
    message: &str,
) -> Result<()> {
    let exe = current_exe()?;
    fs::create_dir_all(config::config_dir())?;
    trim_runtime_files_once();
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_path)?;
    let child = Command::new(&exe)
        .arg(role)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("failed to start {role} from {}", exe.display()))?;
    tracing::info!(pid = child.id(), role, "{message}");
    Ok(())
}

fn trim_runtime_files_once() {
    let cfg = config::load_or_create().unwrap_or_default();
    if let Err(err) = maintenance::trim_runtime_files(&cfg) {
        tracing::debug!("failed to trim runtime files: {err:#}");
    }
}

fn stop_followed_processes(snapshot: &ProcessSnapshot) -> Result<()> {
    for pid in snapshot
        .gui_pids
        .iter()
        .chain(snapshot.daemon_pids.iter())
        .chain(snapshot.control_worker_pids.iter())
        .chain(snapshot.lifecycle_pids.iter())
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
        control_worker_pids: Vec::new(),
        lifecycle_pids: Vec::new(),
    };

    for process in sys.processes().values() {
        if process.status() == ProcessStatus::Zombie {
            continue;
        }
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
            SentinelRole::ControlWorker => snapshot.control_worker_pids.push(pid),
            SentinelRole::Lifecycle => snapshot.lifecycle_pids.push(pid),
            SentinelRole::Other => {}
        }
    }

    snapshot.gui_pids.sort_unstable();
    snapshot.daemon_pids.sort_unstable();
    snapshot.control_worker_pids.sort_unstable();
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
    if has_mode_arg(cmd, "control-worker") || has_mode_arg(cmd, "--control-worker") {
        return SentinelRole::ControlWorker;
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
                | "append"
                | "--append"
                | "new"
                | "--new"
                | "delete"
                | "--delete"
                | "clear-archived"
                | "--clear-archived"
                | "desktop-control-status"
                | "--desktop-control-status"
                | "debug-new-chat"
                | "--debug-new-chat"
                | "debug-new-direct"
                | "--debug-new-direct"
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

fn unload_launch_agent() {
    let Ok(uid) = current_uid() else {
        return;
    };
    let domain = format!("gui/{uid}");
    let path = launch_agent_path();
    let service = format!("{domain}/{LABEL}");
    let _ = Command::new("launchctl")
        .args(["bootout", &domain])
        .arg(&path)
        .status();
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .status();
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
            sentinel_role(&cmd(&["codex-sentinel", "control-worker"])),
            SentinelRole::ControlWorker
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
