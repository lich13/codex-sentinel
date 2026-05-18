#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
use self::macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use self::unsupported as platform;
#[cfg(target_os = "windows")]
use self::windows as platform;

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sysinfo::{ProcessStatus, System};

use crate::codex;
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
    let exe = helper_exe()?;
    let path = platform::install_launch_agent(&exe)?;
    #[cfg(target_os = "windows")]
    ensure_lifecycle_running_after_install()?;
    Ok(path)
}

pub fn helper_executable() -> Result<PathBuf> {
    helper_exe()
}

pub fn shutdown_followed_processes() -> Result<()> {
    platform::unload_launch_agent();
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
    let exe = gui_exe()?;
    platform::ensure_gui_running(snapshot, &exe)
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
    let exe = helper_exe()?;
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
    let mut command = Command::new(&exe);
    command
        .arg(role)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    platform::prepare_background_command(&mut command);
    let child = command
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

#[cfg(target_os = "windows")]
fn ensure_lifecycle_running_after_install() -> Result<()> {
    let snapshot = inspect_processes()?;
    if !snapshot.lifecycle_pids.is_empty() {
        return Ok(());
    }
    spawn_role_process(
        "lifecycle",
        &config::config_dir().join("lifecycle.out.log"),
        &config::config_dir().join("lifecycle.err.log"),
        "started Codex Sentinel lifecycle helper after installing Windows Run key",
    )
}

fn followed_process_pids(snapshot: &ProcessSnapshot) -> Vec<u32> {
    snapshot
        .gui_pids
        .iter()
        .chain(snapshot.daemon_pids.iter())
        .chain(snapshot.control_worker_pids.iter())
        .copied()
        .collect()
}

fn stop_followed_processes(snapshot: &ProcessSnapshot) -> Result<()> {
    for pid in followed_process_pids(snapshot) {
        tracing::info!(
            pid,
            "stopping Codex Sentinel process because Codex is not running"
        );
        terminate_pid(pid)?;
    }
    Ok(())
}

fn terminate_pid(pid: u32) -> Result<()> {
    platform::terminate_pid(pid)
}

fn inspect_processes() -> Result<ProcessSnapshot> {
    let own_exes = sentinel_exes()?;
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

        if codex::is_codex_app_process(&name, &cmd_text) {
            snapshot.codex_running = true;
            continue;
        }

        if pid == current_pid || !is_known_sentinel_binary(process.exe(), &cmd, &own_exes) {
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

fn is_known_sentinel_binary(exe: Option<&Path>, cmd: &[String], known_exes: &[PathBuf]) -> bool {
    if exe.is_some_and(|path| known_exes.iter().any(|known| same_path(path, known))) {
        return true;
    }
    cmd.first()
        .map(Path::new)
        .is_some_and(|path| known_exes.iter().any(|known| same_path(path, known)))
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
                | "debug-visible-send-plan"
                | "--debug-visible-send-plan"
                | "debug-new-chat"
                | "--debug-new-chat"
                | "debug-new-direct"
                | "--debug-new-direct"
                | "debug-thread-failure-state"
                | "--debug-thread-failure-state"
                | "debug-app-server-thread"
                | "--debug-app-server-thread"
                | "open-desktop-permissions"
                | "--open-desktop-permissions"
                | "hook-status"
                | "--hook-status"
                | "install-hooks"
                | "--install-hooks"
                | "hook-stop"
                | "--hook-stop"
                | "notify-completion"
                | "--notify-completion"
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

fn helper_exe() -> Result<PathBuf> {
    platform::helper_exe(&current_exe()?)
}

fn gui_exe() -> Result<PathBuf> {
    platform::gui_exe(&current_exe()?)
}

fn sentinel_exes() -> Result<Vec<PathBuf>> {
    let current = current_exe()?;
    let mut exes = vec![
        current.clone(),
        platform::helper_exe(&current)?,
        platform::gui_exe(&current)?,
    ];
    exes.sort();
    exes.dedup_by(|left, right| same_path(left, right));
    Ok(exes)
}

fn launch_agent_path() -> PathBuf {
    platform::launch_agent_path()
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
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "debug-visible-send-plan"])),
            SentinelRole::Other
        );
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "debug-thread-failure-state"])),
            SentinelRole::Other
        );
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "debug-app-server-thread"])),
            SentinelRole::Other
        );
        assert_eq!(
            sentinel_role(&cmd(&["codex-sentinel", "notify-completion"])),
            SentinelRole::Other
        );
    }

    #[test]
    fn codex_absent_stop_plan_keeps_lifecycle_helper_running() {
        let snapshot = ProcessSnapshot {
            codex_running: false,
            gui_pids: vec![101],
            daemon_pids: vec![202],
            control_worker_pids: vec![303],
            lifecycle_pids: vec![404],
        };

        assert_eq!(followed_process_pids(&snapshot), vec![101, 202, 303]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_launch_agent_path_targets_run_key() {
        let path = launch_agent_path();
        let text = path.display().to_string();
        assert!(text.contains("CurrentVersion\\Run"));
        assert!(text.contains("local.codex-sentinel"));
    }
}
