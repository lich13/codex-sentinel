use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow};

use super::ProcessSnapshot;

pub(super) fn install_launch_agent(_exe: &Path) -> Result<PathBuf> {
    Err(anyhow!(
        "lifecycle installation is not supported on this platform"
    ))
}

pub(super) fn ensure_gui_running(_snapshot: &ProcessSnapshot, _exe: &Path) -> Result<()> {
    Err(anyhow!(
        "lifecycle GUI follow mode is not supported on this platform"
    ))
}

pub(super) fn terminate_pid(_pid: u32) -> Result<()> {
    Err(anyhow!(
        "lifecycle process termination is not supported on this platform"
    ))
}

pub(super) fn launch_agent_path() -> PathBuf {
    PathBuf::from("unsupported-platform")
}

pub(super) fn prepare_background_command(_command: &mut Command) {}

pub(super) fn unload_launch_agent() {}

pub(super) fn helper_exe(current: &Path) -> Result<PathBuf> {
    Ok(current.to_path_buf())
}

pub(super) fn gui_exe(current: &Path) -> Result<PathBuf> {
    Ok(current.to_path_buf())
}
