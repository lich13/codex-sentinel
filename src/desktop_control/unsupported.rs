use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DesktopControlStatus {
    pub mode: String,
    pub accessibility_granted: bool,
    pub screen_recording_granted: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisibleNewThreadResult {
    pub thread_id: Option<String>,
    pub turn_id: String,
    pub transport: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisibleSendDebugPlan {
    pub platform: String,
    pub send_enabled: bool,
    pub window: Option<VisibleWindowDebug>,
    pub existing_thread_focus: Option<ScreenPoint>,
    pub existing_thread_send_points: Vec<ScreenPoint>,
    pub new_thread_focus: Option<ScreenPoint>,
    pub new_thread_send_points: Vec<ScreenPoint>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisibleWindowDebug {
    pub hwnd: String,
    pub pid: u32,
    pub title: String,
    pub class_name: String,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub visible: bool,
    pub minimized: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleThreadFailureState {
    Failed,
    StoppedMarker,
    NotFailed,
}

pub fn inspect() -> DesktopControlStatus {
    DesktopControlStatus {
        mode: "visible_desktop_unsupported".to_string(),
        accessibility_granted: false,
        screen_recording_granted: false,
        notes: vec!["visible desktop control is only implemented on macOS and Windows".to_string()],
    }
}

pub fn open_permission_settings() -> Result<()> {
    Err(anyhow!(
        "visible desktop permissions are not implemented on this platform"
    ))
}

pub fn visible_input_ready() -> bool {
    false
}

pub fn visible_state_ready() -> bool {
    false
}

pub fn debug_visible_send_plan() -> VisibleSendDebugPlan {
    VisibleSendDebugPlan {
        platform: "unsupported".to_string(),
        send_enabled: false,
        window: None,
        existing_thread_focus: None,
        existing_thread_send_points: Vec::new(),
        new_thread_focus: None,
        new_thread_send_points: Vec::new(),
        notes: vec!["Visible desktop send is not supported on this platform.".to_string()],
    }
}

pub fn prepare_existing_thread_visible(thread_id: &str) -> Result<()> {
    let _ = thread_id;
    Err(anyhow!(
        "visible desktop thread opening is not implemented on this platform"
    ))
}

pub fn inspect_thread_failure_state(thread_id: &str) -> Result<VisibleThreadFailureState> {
    let _ = thread_id;
    Err(anyhow!(
        "visible desktop state inspection is not implemented on this platform"
    ))
}

pub fn prepare_new_thread_visible(path: Option<&str>) -> Result<()> {
    let _ = path;
    Err(anyhow!(
        "visible desktop new-thread control is not implemented on this platform"
    ))
}

pub fn submit_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    let _ = (prompt, attempt);
    Err(anyhow!(
        "visible desktop prompt submission is not implemented on this platform"
    ))
}

pub fn submit_new_thread_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    let _ = (prompt, attempt);
    Err(anyhow!(
        "visible desktop new-thread prompt submission is not implemented on this platform"
    ))
}

pub fn visible_turn_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("visible-desktop-{millis}")
}
