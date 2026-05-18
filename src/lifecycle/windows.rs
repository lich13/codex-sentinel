use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use super::ProcessSnapshot;

use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const HELPER_EXE: &str = "codex-sentinel-cli.exe";

pub(super) fn install_launch_agent(exe: &Path) -> Result<PathBuf> {
    let path = launch_agent_path();
    let launcher = gui_exe(exe)?;
    let reg_value = hidden_lifecycle_run_command(&launcher);
    let status = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            super::LABEL,
            "/t",
            "REG_SZ",
            "/d",
            &reg_value,
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .with_context(|| "failed to write Windows Run key")?;
    if status.success() {
        Ok(path)
    } else {
        Err(anyhow!("reg add Run key failed with {status}"))
    }
}

fn hidden_lifecycle_run_command(exe: &Path) -> String {
    format!(
        "{} lifecycle",
        windows_command_arg(&exe.display().to_string())
    )
}

fn windows_command_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

pub(super) fn ensure_gui_running(snapshot: &ProcessSnapshot, exe: &Path) -> Result<()> {
    if !snapshot.gui_pids.is_empty() {
        return Ok(());
    }

    tracing::info!(exe = %exe.display(), "starting Codex Sentinel GUI because Codex is running");
    let status = Command::new(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("failed to open {}", exe.display()))?;
    let _ = status;
    Ok(())
}

pub(super) fn terminate_pid(pid: u32) -> Result<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .with_context(|| format!("failed to terminate pid {pid}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("taskkill /PID {pid} failed with {status}"))
    }
}

pub(super) fn launch_agent_path() -> PathBuf {
    PathBuf::from(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\local.codex-sentinel")
}

pub(super) fn prepare_background_command(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

pub(super) fn unload_launch_agent() {
    let _ = Command::new("reg")
        .args([
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            super::LABEL,
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

pub(super) fn helper_exe(current: &Path) -> Result<PathBuf> {
    if current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(HELPER_EXE))
    {
        return Ok(current.to_path_buf());
    }
    let helper = current
        .parent()
        .map(|parent| parent.join(HELPER_EXE))
        .unwrap_or_else(|| current.to_path_buf());
    Ok(helper)
}

pub(super) fn gui_exe(current: &Path) -> Result<PathBuf> {
    if current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(HELPER_EXE))
    {
        if let Some(parent) = current.parent() {
            let mut candidates = std::fs::read_dir(parent)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(Result::ok))
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with("Codex Sentinel_")
                                && name.ends_with("_windows_x64.exe")
                        })
                })
                .collect::<Vec<_>>();
            candidates.sort();
            if let Some(gui) = candidates.pop() {
                return Ok(gui);
            }
        }
    }
    Ok(current.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_key_uses_gui_launcher_without_powershell() {
        let command =
            hidden_lifecycle_run_command(Path::new(r"C:\Program Files\Codex Sentinel\app.exe"));
        assert_eq!(
            command,
            r#""C:\Program Files\Codex Sentinel\app.exe" lifecycle"#
        );
        assert!(!command.contains("powershell.exe"));
        assert!(!command.contains("cmd.exe"));
        assert!(!command.contains("-WindowStyle"));
        assert!(command.contains(r"C:\Program Files\Codex Sentinel\app.exe"));
    }

    #[test]
    fn helper_exe_resolves_next_to_gui_launcher() {
        let helper = helper_exe(Path::new(
            r"C:\Program Files\Codex Sentinel\Codex Sentinel_0.1.0_windows_x64.exe",
        ))
        .unwrap();

        assert_eq!(
            helper,
            PathBuf::from(r"C:\Program Files\Codex Sentinel\codex-sentinel-cli.exe")
        );
    }
}
