use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use super::ProcessSnapshot;
use crate::config;

pub(super) fn install_launch_agent(exe: &Path) -> Result<PathBuf> {
    let path = launch_agent_path();
    let stdout = config::config_dir().join("lifecycle.out.log");
    let stderr = config::config_dir().join("lifecycle.err.log");
    fs::create_dir_all(config::config_dir())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&path, launch_agent_plist(exe, &stdout, &stderr).as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    reload_launch_agent(&path)?;
    Ok(path)
}

pub(super) fn ensure_gui_running(snapshot: &ProcessSnapshot, exe: &Path) -> Result<()> {
    if !snapshot.gui_pids.is_empty() {
        return Ok(());
    }

    let app = app_bundle_path(exe)?;
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

pub(super) fn terminate_pid(pid: u32) -> Result<()> {
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

pub(super) fn launch_agent_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", super::LABEL))
}

pub(super) fn prepare_background_command(_command: &mut Command) {}

pub(super) fn unload_launch_agent() {
    let Ok(uid) = current_uid() else {
        return;
    };
    let domain = format!("gui/{uid}");
    let path = launch_agent_path();
    let service = format!("{domain}/{}", super::LABEL);
    let _ = Command::new("launchctl")
        .args(["bootout", &domain])
        .arg(&path)
        .status();
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .status();
}

pub(super) fn helper_exe(current: &Path) -> Result<PathBuf> {
    Ok(current.to_path_buf())
}

pub(super) fn gui_exe(current: &Path) -> Result<PathBuf> {
    Ok(current.to_path_buf())
}

fn app_bundle_path(exe: &Path) -> Result<PathBuf> {
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
        label = super::LABEL,
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

    let service = format!("{domain}/{}", super::LABEL);
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
