#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    if let Err(err) = run() {
        eprintln!("failed to launch Codex Sentinel: {err}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let launcher = std::env::current_exe()?;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let helper = find_helper_executable(&launcher).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "could not find codex-sentinel-cli.exe next to {}",
                launcher.display()
            ),
        )
    })?;

    let mut command = Command::new(helper);
    command.args(&args);
    if let Some(mode) = hidden_spawn_mode(&args) {
        command.stdin(Stdio::null());
        match mode {
            HiddenSpawnMode::Gui => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            HiddenSpawnMode::Background(role) => {
                let (stdout, stderr) = role_log_paths(role);
                if let Some(parent) = stdout.parent() {
                    fs::create_dir_all(parent)?;
                }
                let stdout = OpenOptions::new().create(true).append(true).open(stdout)?;
                let stderr = OpenOptions::new().create(true).append(true).open(stderr)?;
                command.stdout(stdout).stderr(stderr);
            }
        }
        prepare_hidden_command(&mut command);
        command.spawn()?;
        return Ok(());
    }

    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn find_helper_executable(launcher: &Path) -> Option<PathBuf> {
    helper_candidates(launcher)
        .into_iter()
        .find(|path| path.exists())
}

fn helper_candidates(launcher: &Path) -> Vec<PathBuf> {
    let dir = launcher.parent().unwrap_or_else(|| Path::new("."));
    vec![
        dir.join("codex-sentinel-cli.exe"),
        dir.join("codex-sentinel.exe"),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HiddenSpawnMode {
    Gui,
    Background(&'static str),
}

fn hidden_spawn_mode(args: &[OsString]) -> Option<HiddenSpawnMode> {
    if args.is_empty() {
        return Some(HiddenSpawnMode::Gui);
    }
    let role = args.first()?.to_string_lossy();
    match role.as_ref() {
        "lifecycle" | "--lifecycle" => Some(HiddenSpawnMode::Background("lifecycle")),
        "daemon" | "--daemon" => Some(HiddenSpawnMode::Background("telegram-daemon")),
        "control-worker" | "--control-worker" => {
            Some(HiddenSpawnMode::Background("control-worker"))
        }
        _ => None,
    }
}

fn role_log_paths(role: &str) -> (PathBuf, PathBuf) {
    let dir = sentinel_config_dir();
    (
        dir.join(format!("{role}.out.log")),
        dir.join(format!("{role}.err.log")),
    )
}

fn sentinel_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex-sentinel")
}

fn prepare_hidden_command(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = command;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_candidates_prefer_packaged_cli_name() {
        let candidates =
            helper_candidates(Path::new(r"C:\dist\Codex Sentinel_0.1.0_windows_x64.exe"));

        assert_eq!(
            candidates[0],
            PathBuf::from(r"C:\dist\codex-sentinel-cli.exe")
        );
        assert_eq!(candidates[1], PathBuf::from(r"C:\dist\codex-sentinel.exe"));
    }

    #[test]
    fn gui_and_background_roles_launch_hidden() {
        assert_eq!(hidden_spawn_mode(&[]), Some(HiddenSpawnMode::Gui));
        assert_eq!(
            hidden_spawn_mode(&[OsString::from("lifecycle")]),
            Some(HiddenSpawnMode::Background("lifecycle"))
        );
        assert_eq!(
            hidden_spawn_mode(&[OsString::from("daemon")]),
            Some(HiddenSpawnMode::Background("telegram-daemon"))
        );
        assert_eq!(
            hidden_spawn_mode(&[OsString::from("control-worker")]),
            Some(HiddenSpawnMode::Background("control-worker"))
        );
        assert_eq!(hidden_spawn_mode(&[OsString::from("hook-status")]), None);
    }
}
