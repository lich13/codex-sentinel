use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

#[cfg(target_os = "macos")]
use core_foundation::base::{CFType, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::boolean::CFBoolean;
#[cfg(target_os = "macos")]
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::string::{CFString, CFStringRef};
#[cfg(target_os = "macos")]
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton, KeyCode};
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
#[cfg(target_os = "macos")]
use core_graphics::geometry::CGPoint;
#[cfg(target_os = "macos")]
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};
#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopControlStatus {
    pub mode: String,
    pub accessibility_granted: bool,
    pub screen_recording_granted: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisibleContinueResult {
    pub thread_id: String,
    pub turn_id: String,
    pub transport: String,
}

pub fn inspect() -> DesktopControlStatus {
    let accessibility_granted = accessibility_enabled();
    let screen_recording_granted = screen_recording_preflight();
    let mut notes = Vec::new();
    if !accessibility_granted {
        notes
            .push("需要在系统设置 -> 隐私与安全性 -> 辅助功能 中允许 Codex Sentinel，才能在 Codex APP 可见窗口内点击和输入。如果开关已经是开启状态，说明本地重签名后 TCC 记录过期，请先关掉再打开一次。".to_string());
    }
    if !screen_recording_granted {
        notes.push(
            "屏幕录制权限用于后续窗口截图与状态观测；一键继续只依赖辅助功能权限。".to_string(),
        );
    }

    DesktopControlStatus {
        mode: "visible_desktop".to_string(),
        accessibility_granted,
        screen_recording_granted,
        notes,
    }
}

pub fn open_permission_settings() -> Result<()> {
    request_accessibility_prompt();
    open_settings_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    )?;
    open_settings_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    )
}

fn request_accessibility_prompt() {
    #[cfg(target_os = "macos")]
    {
        let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        let _ = unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) };
    }
}

pub fn visible_input_ready() -> bool {
    accessibility_enabled()
}

pub fn continue_thread_visible(thread_id: &str, prompt: &str) -> Result<VisibleContinueResult> {
    if !accessibility_enabled() {
        return Err(anyhow!(
            "缺少辅助功能权限，无法在 Codex APP 可见窗口内发送指令。请在系统设置 -> 隐私与安全性 -> 辅助功能 中允许 Codex Sentinel 后重试。"
        ));
    }

    submit_visible_prompt(thread_id, prompt).with_context(|| {
        format!("failed to submit visible continue prompt to Codex thread {thread_id}")
    })?;

    Ok(VisibleContinueResult {
        thread_id: thread_id.to_string(),
        turn_id: visible_turn_id(),
        transport: "visible_desktop".to_string(),
    })
}

fn accessibility_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Same primitive used by open-codex-computer-use: check whether this
        // process is trusted by macOS Accessibility/TCC before attempting UI input.
        unsafe { AXIsProcessTrusted() != 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn screen_recording_preflight() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn open_settings_url(url: &str) -> Result<()> {
    let status = Command::new("open").arg(url).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("open {url} failed with {status}"))
    }
}

fn submit_visible_prompt(thread_id: &str, prompt: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let previous_clipboard = read_clipboard().ok();
        let result = submit_visible_prompt_macos(thread_id, prompt);
        if let Some(previous_clipboard) = previous_clipboard {
            let _ = write_clipboard(&previous_clipboard);
        }
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (thread_id, prompt);
        Err(anyhow!(
            "visible desktop control is only available on macOS"
        ))
    }
}

#[cfg(target_os = "macos")]
fn submit_visible_prompt_macos(thread_id: &str, prompt: &str) -> Result<()> {
    open_codex_thread(thread_id)?;
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    let click_x = window.x + window.width / 2.0;
    let click_y = window.y + window.height - 74.0;

    click_at(window.pid, click_x, click_y)?;
    std::thread::sleep(Duration::from_millis(120));

    write_clipboard(prompt.as_bytes())?;
    post_command_v(window.pid)?;
    std::thread::sleep(Duration::from_millis(120));
    post_key(window.pid, KeyCode::RETURN, false, CGEventFlags::empty())?;
    std::thread::sleep(Duration::from_millis(150));
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct CodexWindow {
    pid: i32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
fn open_codex_thread(thread_id: &str) -> Result<()> {
    let uri = format!("codex://threads/{thread_id}");
    let status = Command::new("open")
        .arg(&uri)
        .status()
        .with_context(|| format!("failed to open {uri}"))?;
    if !status.success() {
        return Err(anyhow!("open {uri} failed with {status}"));
    }
    std::thread::sleep(Duration::from_millis(900));

    let _ = Command::new("open").args(["-a", "Codex"]).status();
    std::thread::sleep(Duration::from_millis(250));
    Ok(())
}

#[cfg(target_os = "macos")]
fn wait_for_codex_window(timeout: Duration) -> Result<CodexWindow> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = find_codex_window() {
            return Ok(window);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Codex window not found after opening the target thread"
            ));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

#[cfg(target_os = "macos")]
fn find_codex_window() -> Option<CodexWindow> {
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let windows = copy_window_info(option, kCGNullWindowID)?;
    for raw_value in windows.get_all_values() {
        let value = unsafe { CFType::wrap_under_get_rule(raw_value as _) };
        let Some(untyped_dict) = value.downcast::<CFDictionary>() else {
            continue;
        };
        let dict: CFDictionary<CFString, CFType> =
            unsafe { TCFType::wrap_under_get_rule(untyped_dict.as_concrete_TypeRef()) };
        if dict_string(&dict, unsafe { kCGWindowOwnerName }).as_deref() != Some("Codex") {
            continue;
        }
        if dict_i64(&dict, unsafe { kCGWindowLayer }).unwrap_or(-1) != 0 {
            continue;
        }
        let pid = dict_i64(&dict, unsafe { kCGWindowOwnerPID })? as i32;
        let bounds = dict_bounds(&dict)?;
        if bounds.width < 320.0 || bounds.height < 240.0 {
            continue;
        }
        return Some(CodexWindow {
            pid,
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        });
    }
    None
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct WindowBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
fn dict_bounds(dict: &CFDictionary<CFString, CFType>) -> Option<WindowBounds> {
    let value = dict_value(dict, unsafe { kCGWindowBounds })?;
    let untyped_bounds = value.downcast::<CFDictionary>()?;
    let bounds: CFDictionary<CFString, CFType> =
        unsafe { TCFType::wrap_under_get_rule(untyped_bounds.as_concrete_TypeRef()) };
    Some(WindowBounds {
        x: dict_f64_by_name(&bounds, "X")?,
        y: dict_f64_by_name(&bounds, "Y")?,
        width: dict_f64_by_name(&bounds, "Width")?,
        height: dict_f64_by_name(&bounds, "Height")?,
    })
}

#[cfg(target_os = "macos")]
fn dict_value(dict: &CFDictionary<CFString, CFType>, key_ref: CFStringRef) -> Option<CFType> {
    let key = unsafe { CFString::wrap_under_get_rule(key_ref) };
    dict.find(&key).map(|value| value.clone())
}

#[cfg(target_os = "macos")]
fn dict_value_by_name(dict: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<CFType> {
    let key = CFString::from_static_string(key);
    dict.find(&key).map(|value| value.clone())
}

#[cfg(target_os = "macos")]
fn dict_string(dict: &CFDictionary<CFString, CFType>, key_ref: CFStringRef) -> Option<String> {
    dict_value(dict, key_ref)
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
}

#[cfg(target_os = "macos")]
fn dict_i64(dict: &CFDictionary<CFString, CFType>, key_ref: CFStringRef) -> Option<i64> {
    dict_value(dict, key_ref)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64().or_else(|| number.to_i32().map(i64::from)))
}

#[cfg(target_os = "macos")]
fn dict_f64_by_name(dict: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<f64> {
    dict_value_by_name(dict, key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| {
            number
                .to_f64()
                .or_else(|| number.to_i64().map(|value| value as f64))
                .or_else(|| number.to_i32().map(f64::from))
        })
}

#[cfg(target_os = "macos")]
fn click_at(pid: i32, x: f64, y: f64) -> Result<()> {
    let source = event_source()?;
    let point = CGPoint::new(x, y);
    for event_type in [
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
    ] {
        let event =
            CGEvent::new_mouse_event(source.clone(), event_type, point, CGMouseButton::Left)
                .map_err(|_| anyhow!("failed to create mouse event"))?;
        event.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(35));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn post_command_v(pid: i32) -> Result<()> {
    let flags = CGEventFlags::CGEventFlagCommand;
    post_key(pid, KeyCode::COMMAND, true, flags)?;
    post_key(pid, KeyCode::ANSI_V, true, flags)?;
    post_key(pid, KeyCode::ANSI_V, false, flags)?;
    post_key(pid, KeyCode::COMMAND, false, CGEventFlags::empty())
}

#[cfg(target_os = "macos")]
fn post_key(pid: i32, keycode: u16, keydown: bool, flags: CGEventFlags) -> Result<()> {
    let source = event_source()?;
    let event = CGEvent::new_keyboard_event(source, keycode, keydown)
        .map_err(|_| anyhow!("failed to create keyboard event"))?;
    event.set_flags(flags);
    event.post_to_pid(pid);
    std::thread::sleep(Duration::from_millis(35));
    Ok(())
}

#[cfg(target_os = "macos")]
fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow!("failed to create CGEvent source"))
}

fn read_clipboard() -> Result<Vec<u8>> {
    let output = Command::new("pbpaste")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("failed to read clipboard")?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(anyhow!("pbpaste failed with {}", output.status))
    }
}

fn write_clipboard(bytes: &[u8]) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to start pbcopy")?;
    child
        .stdin
        .take()
        .context("missing pbcopy stdin")?
        .write_all(bytes)?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("pbcopy failed with {status}"))
    }
}

fn visible_turn_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("visible-desktop-{millis}")
}
