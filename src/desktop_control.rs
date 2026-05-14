#[cfg(target_os = "macos")]
use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::io::Write;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
#[cfg(target_os = "macos")]
use foreign_types::ForeignType;
#[cfg(target_os = "macos")]
use image::GenericImageView;
use serde::Serialize;

#[cfg(target_os = "macos")]
const EXISTING_THREAD_OPEN_SETTLE: Duration = Duration::from_millis(800);
#[cfg(target_os = "macos")]
const COMPOSER_AFTER_FOCUS_SETTLE: Duration = Duration::from_millis(160);
#[cfg(target_os = "macos")]
const COMPOSER_AFTER_CLEAR_SETTLE: Duration = Duration::from_millis(120);
#[cfg(target_os = "macos")]
const COMPOSER_AFTER_INSERT_SETTLE: Duration = Duration::from_millis(170);
#[cfg(target_os = "macos")]
const POST_SEND_SETTLE: Duration = Duration::from_millis(520);
#[cfg(target_os = "macos")]
const PASTEBOARD_SETTLE: Duration = Duration::from_millis(120);
#[cfg(target_os = "macos")]
const COMPOSER_CLEAR_PASS_SETTLE: Duration = Duration::from_millis(90);
#[cfg(target_os = "macos")]
const EXISTING_THREAD_RIGHT_PANEL_WIDTH: f64 = 420.0;
#[cfg(target_os = "macos")]
const EXISTING_THREAD_COMPOSER_MAX_WIDTH: f64 = 980.0;

#[cfg(target_os = "macos")]
use core_foundation::base::{Boolean, CFType, CFTypeRef, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::boolean::CFBoolean;
#[cfg(target_os = "macos")]
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::string::{CFString, CFStringRef};
#[cfg(target_os = "macos")]
use core_graphics::event::EventField;
#[cfg(target_os = "macos")]
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton, KeyCode};
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
#[cfg(target_os = "macos")]
use core_graphics::geometry::CGPoint;
#[cfg(target_os = "macos")]
use core_graphics::sys::CGEventRef;
#[cfg(target_os = "macos")]
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID,
};
#[cfg(target_os = "macos")]
use sysinfo::System;
#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut Boolean,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
}

#[cfg(target_os = "macos")]
type AXUIElementRef = CFTypeRef;

#[cfg(target_os = "macos")]
const AX_ERROR_SUCCESS: i32 = 0;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventKeyboardSetUnicodeString(
        event: CGEventRef,
        string_length: libc::size_t,
        unicode_string: *const u16,
    );
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleThreadFailureState {
    Failed,
    StoppedMarker,
    NotFailed,
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

pub fn visible_state_ready() -> bool {
    accessibility_enabled() && screen_recording_preflight()
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

#[cfg(target_os = "macos")]
pub fn prepare_existing_thread_visible(thread_id: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        open_codex_thread(thread_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = thread_id;
        Err(anyhow!(
            "visible desktop control is only available on macOS"
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn inspect_thread_failure_state(thread_id: &str) -> Result<VisibleThreadFailureState> {
    prepare_existing_thread_visible(thread_id)?;
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    std::thread::sleep(Duration::from_millis(450));
    codex_window_thread_failure_state(&window)
}

#[cfg(not(target_os = "macos"))]
pub fn inspect_thread_failure_state(thread_id: &str) -> Result<VisibleThreadFailureState> {
    let _ = thread_id;
    Err(anyhow!(
        "visible desktop state inspection is only available on macOS"
    ))
}

#[cfg(target_os = "macos")]
pub fn prepare_new_thread_visible(path: Option<&str>) -> Result<()> {
    open_codex_new_thread(path)
}

#[cfg(target_os = "macos")]
pub fn submit_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    submit_prompt_to_current_codex_window(prompt, attempt, ComposerPlacement::ExistingThread)
}

#[cfg(target_os = "macos")]
pub fn submit_new_thread_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    submit_prompt_to_current_codex_window(prompt, attempt, ComposerPlacement::NewThread)
}

#[cfg(target_os = "macos")]
fn submit_prompt_to_current_codex_window(
    prompt: &str,
    attempt: usize,
    placement: ComposerPlacement,
) -> Result<()> {
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    dismiss_input_overlays(window.pid)?;
    let focus_points = prompt_focus_points(&window, attempt, placement);
    for point in &focus_points {
        click_at(window.pid, point.x, point.y)?;
        std::thread::sleep(Duration::from_millis(90));
    }
    std::thread::sleep(COMPOSER_AFTER_FOCUS_SETTLE);

    clear_current_input(window.pid, attempt)?;
    std::thread::sleep(COMPOSER_AFTER_CLEAR_SETTLE);
    if let Some(point) = focus_points.first() {
        click_at(window.pid, point.x, point.y)?;
        std::thread::sleep(COMPOSER_AFTER_FOCUS_SETTLE);
    }
    insert_prompt_text(window.pid, prompt, attempt)?;
    std::thread::sleep(COMPOSER_AFTER_INSERT_SETTLE);
    trigger_send(&window, attempt, placement)?;
    std::thread::sleep(POST_SEND_SETTLE);
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct CodexWindow {
    pid: i32,
    window_id: i64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
enum ComposerPlacement {
    ExistingThread,
    NewThread,
}

#[cfg(target_os = "macos")]
fn open_codex_thread(thread_id: &str) -> Result<()> {
    let uri = format!("codex://threads/{thread_id}");
    open_codex_uri(&uri)?;
    activate_codex_app()?;
    wait_for_codex_window(Duration::from_secs(4))?;
    std::thread::sleep(EXISTING_THREAD_OPEN_SETTLE);
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_codex_new_thread(path: Option<&str>) -> Result<()> {
    if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
        open_path_with_codex(path)?;
    } else {
        open_codex_app()?;
    }
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    if let Err(err) = trigger_codex_new_chat_menu() {
        tracing::debug!(
            "Codex New Chat menu command failed, falling back to sidebar and Cmd+N: {err:#}"
        );
        click_codex_new_chat_button(&window).or_else(|click_err| {
            tracing::debug!(
                "Codex sidebar New Chat click failed, falling back to Cmd+N: {click_err:#}"
            );
            post_command_n(window.pid)
        })?;
    }
    std::thread::sleep(Duration::from_millis(1200));
    Ok(())
}

#[cfg(target_os = "macos")]
fn trigger_codex_new_chat_menu() -> Result<()> {
    let script = r#"tell application "Codex" to activate
    delay 0.15
    tell application "System Events"
        tell process "Codex"
            tell menu bar 1
                tell menu bar item "File"
                    tell menu 1
                        click menu item "New Chat"
                    end tell
                end tell
            end tell
        end tell
    end tell"#;
    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .context("failed to run Codex New Chat menu command")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("Codex New Chat menu command failed with {status}"))
    }
}

#[cfg(target_os = "macos")]
fn open_codex_uri(uri: &str) -> Result<()> {
    let status = Command::new("open")
        .arg(uri)
        .status()
        .with_context(|| format!("failed to open {uri}"))?;
    if !status.success() {
        return Err(anyhow!("open {uri} failed with {status}"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn activate_codex_app() -> Result<()> {
    let status = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "Codex" to activate"#)
        .status()
        .context("failed to activate Codex")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("activate Codex failed with {status}"))
    }
}

#[cfg(target_os = "macos")]
fn open_path_with_codex(path: &str) -> Result<()> {
    let status = Command::new("open")
        .args(["-g", "-a", "Codex", path])
        .status()
        .with_context(|| format!("failed to open {path} with Codex"))?;
    if !status.success() {
        return Err(anyhow!("open -a Codex {path} failed with {status}"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_codex_app() -> Result<()> {
    let status = Command::new("open")
        .args(["-g", "-a", "Codex"])
        .status()
        .context("failed to open Codex")?;
    if !status.success() {
        return Err(anyhow!("open -a Codex failed with {status}"));
    }
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
    let codex_pids = codex_main_pids();
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
        if !codex_pids.is_empty() && !codex_pids.contains(&pid) {
            continue;
        }
        let window_id = dict_i64(&dict, unsafe { kCGWindowNumber })?;
        let bounds = dict_bounds(&dict)?;
        if bounds.width < 320.0 || bounds.height < 240.0 {
            continue;
        }
        return Some(CodexWindow {
            pid,
            window_id,
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        });
    }
    None
}

#[cfg(target_os = "macos")]
fn codex_window_thread_failure_state(window: &CodexWindow) -> Result<VisibleThreadFailureState> {
    if !screen_recording_preflight() {
        return Err(anyhow!(
            "缺少屏幕录制权限，无法确认 Codex 线程是否显示红色错误标记。"
        ));
    }
    let path = capture_window_png(window)?;
    let scale = capture_scale(window, &path).unwrap_or(1.0);
    let result = image_thread_failure_state(&path, scale);
    let _ = std::fs::remove_file(&path);
    result
}

#[cfg(target_os = "macos")]
fn capture_scale(window: &CodexWindow, path: &PathBuf) -> Result<f64> {
    let image = image::open(path)
        .with_context(|| format!("failed to read screenshot {}", path.display()))?;
    let (width, _) = image.dimensions();
    if window.width <= 0.0 {
        return Ok(1.0);
    }
    Ok((width as f64 / window.width).clamp(0.75, 3.0))
}

#[cfg(target_os = "macos")]
fn capture_window_png(window: &CodexWindow) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "codex-sentinel-window-{}-{}.png",
        window.window_id,
        visible_turn_id()
    ));
    let rect = format!(
        "{},{},{},{}",
        window.x.max(0.0).round() as i64,
        window.y.max(0.0).round() as i64,
        window.width.max(1.0).round() as i64,
        window.height.max(1.0).round() as i64
    );
    let status = Command::new("screencapture")
        .args(["-x", "-R", &rect])
        .arg(&path)
        .status()
        .context("failed to capture Codex window screenshot")?;
    if status.success() {
        Ok(path)
    } else {
        Err(anyhow!("screencapture failed with {status}"))
    }
}

#[cfg(target_os = "macos")]
fn image_thread_failure_state(path: &PathBuf, scale: f64) -> Result<VisibleThreadFailureState> {
    let image = image::open(path)
        .with_context(|| format!("failed to read screenshot {}", path.display()))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Ok(VisibleThreadFailureState::NotFailed);
    }

    let layout = sidebar_marker_layout(width, height, scale);
    let red_limits = MarkerLimits::failure(scale);
    let blue_limits = MarkerLimits::stopped(scale);

    if has_marker_component(&image, layout.left_icon, is_codex_error_red, red_limits)
        || has_marker_component(&image, layout.status_icon, is_codex_error_red, red_limits)
    {
        return Ok(VisibleThreadFailureState::Failed);
    }

    if has_marker_component(
        &image,
        layout.status_icon,
        is_codex_stopped_blue,
        blue_limits,
    ) {
        return Ok(VisibleThreadFailureState::StoppedMarker);
    }

    Ok(VisibleThreadFailureState::NotFailed)
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct SidebarMarkerLayout {
    left_icon: MarkerRegion,
    status_icon: MarkerRegion,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct MarkerRegion {
    x_start: u32,
    x_end: u32,
    y_start: u32,
    y_end: u32,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct MarkerLimits {
    min_pixels: usize,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[cfg(target_os = "macos")]
impl MarkerLimits {
    fn failure(scale: f64) -> Self {
        Self {
            min_pixels: scaled_usize(8.0, scale),
            min_width: scaled_u32(2.0, scale),
            max_width: scaled_u32(30.0, scale),
            min_height: scaled_u32(4.0, scale),
            max_height: scaled_u32(30.0, scale),
        }
    }

    fn stopped(scale: f64) -> Self {
        Self {
            min_pixels: scaled_usize(8.0, scale),
            min_width: scaled_u32(3.0, scale),
            max_width: scaled_u32(18.0, scale),
            min_height: scaled_u32(3.0, scale),
            max_height: scaled_u32(18.0, scale),
        }
    }
}

#[cfg(target_os = "macos")]
fn sidebar_marker_layout(width: u32, height: u32, scale: f64) -> SidebarMarkerLayout {
    let scale = scale.clamp(0.75, 3.0);
    let expected_sidebar_width = scaled_u32(300.0, scale).min(width);
    let sidebar_width = if width <= expected_sidebar_width.saturating_mul(2) {
        width
    } else {
        expected_sidebar_width
    };
    let y_start = scaled_u32(44.0, scale).max(height.saturating_mul(40) / 1000);
    let y_end = height
        .saturating_mul(760)
        .checked_div(1000)
        .unwrap_or(height)
        .max(y_start.saturating_add(1))
        .min(height);

    let left_icon = MarkerRegion {
        x_start: scaled_u32(6.0, scale).min(width),
        x_end: scaled_u32(92.0, scale).min(sidebar_width).max(1),
        y_start,
        y_end,
    };
    let status_icon = MarkerRegion {
        x_start: sidebar_width.saturating_sub(scaled_u32(58.0, scale)),
        x_end: sidebar_width.saturating_sub(scaled_u32(8.0, scale)).max(1),
        y_start,
        y_end,
    };

    SidebarMarkerLayout {
        left_icon: clamp_region(left_icon, width, height),
        status_icon: clamp_region(status_icon, width, height),
    }
}

#[cfg(target_os = "macos")]
fn clamp_region(region: MarkerRegion, width: u32, height: u32) -> MarkerRegion {
    let x_start = region.x_start.min(width);
    let y_start = region.y_start.min(height);
    MarkerRegion {
        x_start,
        x_end: region.x_end.min(width).max(x_start),
        y_start,
        y_end: region.y_end.min(height).max(y_start),
    }
}

#[cfg(target_os = "macos")]
fn has_marker_component<F>(
    image: &image::DynamicImage,
    region: MarkerRegion,
    predicate: F,
    limits: MarkerLimits,
) -> bool
where
    F: Fn(u8, u8, u8) -> bool,
{
    let width = region.x_end.saturating_sub(region.x_start);
    let height = region.y_end.saturating_sub(region.y_start);
    if width == 0 || height == 0 {
        return false;
    }
    let len = width as usize * height as usize;
    let mut mask = vec![false; len];
    for local_y in 0..height {
        for local_x in 0..width {
            let [r, g, b, _] = image
                .get_pixel(region.x_start + local_x, region.y_start + local_y)
                .0;
            if predicate(r, g, b) {
                mask[(local_y * width + local_x) as usize] = true;
            }
        }
    }

    let mut visited = vec![false; len];
    let mut stack = Vec::new();
    for idx in 0..len {
        if !mask[idx] || visited[idx] {
            continue;
        }
        let mut pixels = 0usize;
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        visited[idx] = true;
        stack.push(idx);

        while let Some(current) = stack.pop() {
            let x = (current as u32) % width;
            let y = (current as u32) / width;
            pixels += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);

            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height.saturating_sub(1));
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width.saturating_sub(1));
            for next_y in y0..=y1 {
                for next_x in x0..=x1 {
                    if next_x == x && next_y == y {
                        continue;
                    }
                    let next = (next_y * width + next_x) as usize;
                    if mask[next] && !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
        }

        let component_width = max_x.saturating_sub(min_x).saturating_add(1);
        let component_height = max_y.saturating_sub(min_y).saturating_add(1);
        if pixels >= limits.min_pixels
            && (limits.min_width..=limits.max_width).contains(&component_width)
            && (limits.min_height..=limits.max_height).contains(&component_height)
        {
            return true;
        }
    }

    false
}

#[cfg(target_os = "macos")]
fn is_codex_error_red(r: u8, g: u8, b: u8) -> bool {
    r >= 190 && g <= 120 && b <= 120 && r.saturating_sub(g) >= 70 && r.saturating_sub(b) >= 70
}

#[cfg(target_os = "macos")]
fn is_codex_stopped_blue(r: u8, g: u8, b: u8) -> bool {
    b >= 150 && g >= 80 && r <= 140 && b.saturating_sub(r) >= 45
}

#[cfg(target_os = "macos")]
fn scaled_u32(points: f64, scale: f64) -> u32 {
    (points * scale.clamp(0.75, 3.0)).round().max(1.0) as u32
}

#[cfg(target_os = "macos")]
fn scaled_usize(points: f64, scale: f64) -> usize {
    (points * scale.clamp(0.75, 3.0)).round().max(1.0) as usize
}

#[cfg(target_os = "macos")]
fn codex_main_pids() -> HashSet<i32> {
    let mut sys = System::new_all();
    sys.refresh_all();
    sys.processes()
        .values()
        .filter_map(|process| {
            let name = process.name().to_string_lossy();
            let cmd = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            if name == "Codex" && cmd.contains("/Applications/Codex.app/Contents/MacOS/Codex") {
                Some(process.pid().as_u32() as i32)
            } else {
                None
            }
        })
        .collect()
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
    let window = find_codex_window().ok_or_else(|| anyhow!("Codex window not found"))?;
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
        prepare_targeted_mouse_event(&event, &window, point, 1);
        event.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(35));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn click_codex_new_chat_button(window: &CodexWindow) -> Result<()> {
    let x = (window.x + 116.0).clamp(window.x + 24.0, window.x + window.width - 24.0);
    let y = (window.y + 62.0).clamp(window.y + 48.0, window.y + window.height - 24.0);
    click_at(window.pid, x, y)?;
    std::thread::sleep(Duration::from_millis(1_100));
    Ok(())
}

#[cfg(target_os = "macos")]
fn prompt_focus_points(
    window: &CodexWindow,
    attempt: usize,
    placement: ComposerPlacement,
) -> Vec<CGPoint> {
    match placement {
        ComposerPlacement::ExistingThread => existing_thread_prompt_focus_points(window, attempt),
        ComposerPlacement::NewThread => new_thread_prompt_focus_points(window, attempt),
    }
}

#[cfg(target_os = "macos")]
fn existing_thread_prompt_focus_points(window: &CodexWindow, attempt: usize) -> Vec<CGPoint> {
    let bounds = existing_thread_composer_bounds(window, true);
    let x_center = bounds.center_x();
    let x_left = clamp_window_x(window, bounds.left + 88.0);
    let x_right = clamp_window_x(window, bounds.right - 180.0);
    let y_upper = clamp_window_y(window, window.y + window.height - 106.0);
    let y_middle = clamp_window_y(window, window.y + window.height - 88.0);
    let y_lower = clamp_window_y(window, window.y + window.height - 70.0);

    let points = match attempt % 4 {
        0 => vec![
            CGPoint::new(x_left, y_middle),
            CGPoint::new(x_center, y_middle),
        ],
        1 => vec![
            CGPoint::new(x_right, y_upper),
            CGPoint::new(x_center, y_middle),
        ],
        2 => vec![
            CGPoint::new(x_center, y_lower),
            CGPoint::new(x_left, y_lower),
        ],
        _ => vec![
            CGPoint::new(x_left, y_middle),
            CGPoint::new(x_right, y_middle),
        ],
    };
    points
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct ExistingThreadComposerBounds {
    left: f64,
    right: f64,
}

#[cfg(target_os = "macos")]
impl ExistingThreadComposerBounds {
    fn center_x(&self) -> f64 {
        self.left + ((self.right - self.left) / 2.0)
    }
}

#[cfg(target_os = "macos")]
fn existing_thread_composer_bounds(
    window: &CodexWindow,
    reserve_right_panel: bool,
) -> ExistingThreadComposerBounds {
    let reserve_width = if reserve_right_panel && window.width >= 1_300.0 {
        EXISTING_THREAD_RIGHT_PANEL_WIDTH.min(window.width * 0.30)
    } else {
        0.0
    };
    let main_left = window.x;
    let main_right = window.x + window.width - reserve_width;
    let main_width = (main_right - main_left).max(360.0);
    let composer_width = main_width
        .min(EXISTING_THREAD_COMPOSER_MAX_WIDTH)
        .max(360.0);
    let left = main_left + ((main_width - composer_width) / 2.0);
    let right = left + composer_width;
    ExistingThreadComposerBounds {
        left: clamp_window_x(window, left),
        right: clamp_window_x(window, right),
    }
}

#[cfg(target_os = "macos")]
fn new_thread_prompt_focus_points(window: &CodexWindow, attempt: usize) -> Vec<CGPoint> {
    let x_center = clamp_window_x(window, window.x + (window.width * 0.56));
    let x_left = clamp_window_x(window, window.x + (window.width * 0.45));
    let x_right = clamp_window_x(window, window.x + (window.width * 0.66));
    let y_center = clamp_window_y(window, window.y + (window.height * 0.49));
    let y_upper = clamp_window_y(window, window.y + (window.height * 0.46));
    let y_lower = clamp_window_y(window, window.y + (window.height * 0.52));

    match attempt % 4 {
        0 => vec![
            CGPoint::new(x_center, y_center),
            CGPoint::new(x_left, y_center),
        ],
        1 => vec![
            CGPoint::new(x_left, y_upper),
            CGPoint::new(x_center, y_center),
        ],
        2 => vec![
            CGPoint::new(x_right, y_center),
            CGPoint::new(x_center, y_lower),
        ],
        _ => vec![
            CGPoint::new(x_center, y_upper),
            CGPoint::new(x_right, y_lower),
        ],
    }
}

#[cfg(target_os = "macos")]
fn send_button_point(
    window: &CodexWindow,
    attempt: usize,
    placement: ComposerPlacement,
) -> CGPoint {
    match placement {
        ComposerPlacement::ExistingThread => existing_thread_send_button_point(window, attempt),
        ComposerPlacement::NewThread => new_thread_send_button_point(window, attempt),
    }
}

#[cfg(target_os = "macos")]
fn send_button_points(
    window: &CodexWindow,
    attempt: usize,
    placement: ComposerPlacement,
) -> Vec<CGPoint> {
    match placement {
        ComposerPlacement::ExistingThread => existing_thread_send_button_points(window, attempt),
        ComposerPlacement::NewThread => vec![new_thread_send_button_point(window, attempt)],
    }
}

#[cfg(target_os = "macos")]
fn existing_thread_send_button_points(window: &CodexWindow, attempt: usize) -> Vec<CGPoint> {
    let y = match attempt % 3 {
        1 => window.y + window.height - 94.0,
        2 => window.y + window.height - 74.0,
        _ => window.y + window.height - 84.0,
    };
    let y = clamp_window_y(window, y);
    let mut points = Vec::new();
    for reserve_right_panel in [true, false] {
        let bounds = existing_thread_composer_bounds(window, reserve_right_panel);
        let x = match attempt % 3 {
            1 => bounds.right - 42.0,
            2 => bounds.right - 24.0,
            _ => bounds.right - 32.0,
        };
        let point = CGPoint::new(clamp_window_x(window, x), y);
        if points.iter().all(|existing: &CGPoint| {
            (existing.x - point.x).abs() > 12.0 || (existing.y - point.y).abs() > 12.0
        }) {
            points.push(point);
        }
    }
    points
}

#[cfg(target_os = "macos")]
fn existing_thread_send_button_point(window: &CodexWindow, attempt: usize) -> CGPoint {
    existing_thread_send_button_points(window, attempt)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            CGPoint::new(
                clamp_window_x(window, window.x + window.width - 58.0),
                clamp_window_y(window, window.y + window.height - 84.0),
            )
        })
}

#[cfg(target_os = "macos")]
fn new_thread_send_button_point(window: &CodexWindow, attempt: usize) -> CGPoint {
    let x = match attempt % 3 {
        1 => window.x + (window.width * 0.820),
        2 => window.x + (window.width * 0.807),
        _ => window.x + (window.width * 0.813),
    };
    let y = match attempt % 3 {
        1 => window.y + (window.height * 0.466),
        2 => window.y + (window.height * 0.482),
        _ => window.y + (window.height * 0.472),
    };
    CGPoint::new(clamp_window_x(window, x), clamp_window_y(window, y))
}

#[cfg(target_os = "macos")]
fn trigger_send(window: &CodexWindow, attempt: usize, placement: ComposerPlacement) -> Result<()> {
    if matches!(placement, ComposerPlacement::ExistingThread) {
        match existing_thread_send_method(attempt) {
            ExistingThreadSendMethod::CommandReturn => {
                post_key_press(
                    window.pid,
                    KeyCode::RETURN,
                    CGEventFlags::CGEventFlagCommand,
                )?;
            }
            ExistingThreadSendMethod::SendButton => {
                for point in send_button_points(window, attempt, placement) {
                    click_at(window.pid, point.x, point.y)?;
                    std::thread::sleep(Duration::from_millis(120));
                }
            }
            ExistingThreadSendMethod::Return => {
                post_key_press(window.pid, KeyCode::RETURN, CGEventFlags::empty())?;
            }
        }
        return Ok(());
    }

    match attempt % 4 {
        0 => {
            let point = send_button_point(window, attempt, placement);
            click_at(window.pid, point.x, point.y)?;
        }
        1 => {
            post_key_press(window.pid, KeyCode::RETURN, CGEventFlags::empty())?;
        }
        2 => {
            post_key_press(window.pid, KeyCode::TAB, CGEventFlags::empty())?;
            post_key_press(window.pid, KeyCode::SPACE, CGEventFlags::empty())?;
        }
        _ => {
            post_key_press(window.pid, KeyCode::TAB, CGEventFlags::empty())?;
            post_key_press(window.pid, KeyCode::TAB, CGEventFlags::empty())?;
            post_key_press(window.pid, KeyCode::SPACE, CGEventFlags::empty())?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingThreadSendMethod {
    CommandReturn,
    SendButton,
    Return,
}

#[cfg(target_os = "macos")]
fn existing_thread_send_method(attempt: usize) -> ExistingThreadSendMethod {
    match attempt % 4 {
        0 | 2 => ExistingThreadSendMethod::SendButton,
        1 => ExistingThreadSendMethod::CommandReturn,
        _ => ExistingThreadSendMethod::Return,
    }
}

#[cfg(target_os = "macos")]
fn clamp_window_x(window: &CodexWindow, x: f64) -> f64 {
    x.clamp(window.x + 24.0, window.x + window.width - 24.0)
}

#[cfg(target_os = "macos")]
fn clamp_window_y(window: &CodexWindow, y: f64) -> f64 {
    y.clamp(window.y + 80.0, window.y + window.height - 36.0)
}

#[cfg(target_os = "macos")]
fn prepare_targeted_mouse_event(
    event: &CGEvent,
    window: &CodexWindow,
    screen_point: CGPoint,
    click_state: i64,
) {
    event.set_location(screen_point);
    event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click_state);
    event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, 0);
    event.set_integer_value_field(EventField::MOUSE_EVENT_SUB_TYPE, 3);
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER,
        window.window_id,
    );
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
        window.window_id,
    );
    let local_point = CGPoint::new(screen_point.x - window.x, screen_point.y - window.y);
    set_window_location(event.as_ptr(), local_point);
}

#[cfg(target_os = "macos")]
type CGEventSetWindowLocationFn = unsafe extern "C" fn(CGEventRef, CGPoint);

#[cfg(target_os = "macos")]
fn set_window_location(event: CGEventRef, local_point: CGPoint) {
    static SET_WINDOW_LOCATION: OnceLock<Option<CGEventSetWindowLocationFn>> = OnceLock::new();
    let setter = SET_WINDOW_LOCATION.get_or_init(|| {
        let symbol = CString::new("CGEventSetWindowLocation").ok()?;
        let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe {
                std::mem::transmute::<*mut libc::c_void, CGEventSetWindowLocationFn>(ptr)
            })
        }
    });
    if let Some(setter) = setter {
        unsafe {
            setter(event, local_point);
        }
    }
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
fn post_command_n(pid: i32) -> Result<()> {
    let flags = CGEventFlags::CGEventFlagCommand;
    post_key(pid, KeyCode::COMMAND, true, flags)?;
    post_key(pid, KeyCode::ANSI_N, true, flags)?;
    post_key(pid, KeyCode::ANSI_N, false, flags)?;
    post_key(pid, KeyCode::COMMAND, false, CGEventFlags::empty())
}

#[cfg(target_os = "macos")]
fn clear_current_input(pid: i32, attempt: usize) -> Result<()> {
    for step in clear_input_steps(attempt) {
        match step {
            ClearInputStep::AccessibilityFocusedValueReset => {
                let _ = clear_focused_accessibility_value(pid);
            }
            ClearInputStep::KeyboardLineClear => {
                clear_focused_text_line(pid)?;
            }
            ClearInputStep::DeleteKey => {
                post_key_press(pid, KeyCode::DELETE, CGEventFlags::empty())?;
            }
            ClearInputStep::ForwardDeleteKey => {
                post_key_press(pid, KeyCode::FORWARD_DELETE, CGEventFlags::empty())?;
            }
        }
        std::thread::sleep(COMPOSER_CLEAR_PASS_SETTLE);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearInputStep {
    AccessibilityFocusedValueReset,
    KeyboardLineClear,
    DeleteKey,
    ForwardDeleteKey,
}

#[cfg(target_os = "macos")]
fn clear_input_steps(attempt: usize) -> Vec<ClearInputStep> {
    let mut steps = vec![ClearInputStep::AccessibilityFocusedValueReset];
    for _ in 0..clear_passes(attempt) {
        steps.push(ClearInputStep::KeyboardLineClear);
        for keycode in safe_clear_keycodes(attempt) {
            steps.push(match keycode {
                KeyCode::FORWARD_DELETE => ClearInputStep::ForwardDeleteKey,
                _ => ClearInputStep::DeleteKey,
            });
        }
    }
    steps
}

#[cfg(target_os = "macos")]
fn clear_passes(attempt: usize) -> usize {
    if attempt == 0 { 2 } else { 3 }
}

#[cfg(target_os = "macos")]
fn safe_clear_keycodes(attempt: usize) -> Vec<u16> {
    let mut keycodes = vec![KeyCode::DELETE, KeyCode::FORWARD_DELETE];
    if attempt > 0 {
        keycodes.extend([KeyCode::DELETE, KeyCode::DELETE, KeyCode::FORWARD_DELETE]);
    }
    keycodes
}

#[cfg(target_os = "macos")]
fn dismiss_input_overlays(pid: i32) -> Result<()> {
    post_key_press(pid, KeyCode::ESCAPE, CGEventFlags::empty())?;
    std::thread::sleep(COMPOSER_CLEAR_PASS_SETTLE);
    Ok(())
}

#[cfg(target_os = "macos")]
fn clear_focused_text_line(pid: i32) -> Result<()> {
    post_control_key_press(pid, KeyCode::ANSI_A)?;
    post_control_key_press(pid, KeyCode::ANSI_K)
}

#[cfg(target_os = "macos")]
fn clear_focused_accessibility_value(pid: i32) -> Result<()> {
    let app = unsafe { AXUIElementCreateApplication(pid as libc::pid_t) };
    if app.is_null() {
        return Err(anyhow!("failed to create AX application element"));
    }
    let _app_guard = unsafe { CFType::wrap_under_create_rule(app) };
    let focused_attribute = CFString::from_static_string("AXFocusedUIElement");
    let mut focused: CFTypeRef = std::ptr::null();
    let copy_result = unsafe {
        AXUIElementCopyAttributeValue(app, focused_attribute.as_concrete_TypeRef(), &mut focused)
    };
    if copy_result != AX_ERROR_SUCCESS || focused.is_null() {
        return Err(anyhow!("failed to read AXFocusedUIElement: {copy_result}"));
    }
    let _focused_guard = unsafe { CFType::wrap_under_create_rule(focused) };
    clear_accessibility_element_value(focused)
}

#[cfg(target_os = "macos")]
fn clear_accessibility_element_value(element: AXUIElementRef) -> Result<()> {
    let value_attribute = CFString::from_static_string("AXValue");
    let mut settable: Boolean = 0;
    let settable_result = unsafe {
        AXUIElementIsAttributeSettable(
            element,
            value_attribute.as_concrete_TypeRef(),
            &mut settable,
        )
    };
    if settable_result != AX_ERROR_SUCCESS || settable == 0 {
        return Err(anyhow!("AXValue is not settable: {settable_result}"));
    }
    let empty = CFString::from_static_string("");
    let set_result = unsafe {
        AXUIElementSetAttributeValue(
            element,
            value_attribute.as_concrete_TypeRef(),
            empty.as_CFTypeRef(),
        )
    };
    if set_result == AX_ERROR_SUCCESS {
        Ok(())
    } else {
        Err(anyhow!("failed to clear AXValue: {set_result}"))
    }
}

#[cfg(target_os = "macos")]
fn post_control_key_press(pid: i32, keycode: u16) -> Result<()> {
    let flags = CGEventFlags::CGEventFlagControl;
    post_key(pid, KeyCode::CONTROL, true, flags)?;
    post_key(pid, keycode, true, flags)?;
    post_key(pid, keycode, false, flags)?;
    post_key(pid, KeyCode::CONTROL, false, CGEventFlags::empty())
}

#[cfg(target_os = "macos")]
fn insert_prompt_text(pid: i32, prompt: &str, attempt: usize) -> Result<()> {
    match text_input_method(attempt) {
        TextInputMethod::Unicode => type_unicode_text(pid, prompt)?,
        TextInputMethod::ClipboardPaste => {
            write_clipboard(prompt.as_bytes())?;
            std::thread::sleep(PASTEBOARD_SETTLE);
            post_command_v(pid)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextInputMethod {
    Unicode,
    ClipboardPaste,
}

#[cfg(target_os = "macos")]
fn text_input_method(attempt: usize) -> TextInputMethod {
    match attempt % 4 {
        0 | 2 => TextInputMethod::ClipboardPaste,
        1 => TextInputMethod::Unicode,
        _ => TextInputMethod::Unicode,
    }
}

#[cfg(target_os = "macos")]
fn type_unicode_text(pid: i32, text: &str) -> Result<()> {
    for chunk in text.encode_utf16().collect::<Vec<_>>().chunks(64) {
        let source = event_source()?;
        for keydown in [true, false] {
            let event = CGEvent::new_keyboard_event(source.clone(), 0, keydown)
                .map_err(|_| anyhow!("failed to create unicode keyboard event"))?;
            unsafe {
                CGEventKeyboardSetUnicodeString(event.as_ptr(), chunk.len(), chunk.as_ptr());
            }
            event.post_to_pid(pid);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}

fn post_key_press(pid: i32, keycode: u16, flags: CGEventFlags) -> Result<()> {
    post_key(pid, keycode, true, flags)?;
    post_key(pid, keycode, false, flags)
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

pub fn visible_turn_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("visible-desktop-{millis}")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn marker_fixture<F>(name: &str, draw: F) -> PathBuf
    where
        F: FnOnce(&mut RgbaImage),
    {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("codex-sentinel-{name}-{nanos}.png"));
        let mut image = RgbaImage::from_pixel(1400, 700, Rgba([245, 245, 245, 255]));
        draw(&mut image);
        image.save(&path).expect("save marker fixture");
        path
    }

    fn fill_rect(
        image: &mut RgbaImage,
        x0: u32,
        y0: u32,
        width: u32,
        height: u32,
        color: Rgba<u8>,
    ) {
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                image.put_pixel(x, y, color);
            }
        }
    }

    fn state_for_fixture(path: &PathBuf) -> VisibleThreadFailureState {
        let state = image_thread_failure_state(path, 2.0).expect("inspect fixture");
        let _ = std::fs::remove_file(path);
        state
    }

    #[test]
    fn visible_submit_prefers_clipboard_before_unicode_fallback() {
        assert_eq!(text_input_method(0), TextInputMethod::ClipboardPaste);
        assert_eq!(text_input_method(1), TextInputMethod::Unicode);
        assert_eq!(text_input_method(2), TextInputMethod::ClipboardPaste);
        assert_eq!(text_input_method(3), TextInputMethod::Unicode);
    }

    #[test]
    fn existing_thread_submit_prefers_send_button_before_shortcut_fallbacks() {
        assert_eq!(
            existing_thread_send_method(0),
            ExistingThreadSendMethod::SendButton
        );
        assert_eq!(
            existing_thread_send_method(1),
            ExistingThreadSendMethod::CommandReturn
        );
        assert_eq!(
            existing_thread_send_method(2),
            ExistingThreadSendMethod::SendButton
        );
        assert_eq!(
            existing_thread_send_method(3),
            ExistingThreadSendMethod::Return
        );
    }

    #[test]
    fn clear_current_input_avoids_global_select_all() {
        assert_eq!(clear_passes(0), 2);
        assert_eq!(clear_passes(1), 3);

        for attempt in 0..4 {
            let keycodes = safe_clear_keycodes(attempt);
            assert!(keycodes.contains(&KeyCode::DELETE));
            assert!(keycodes.contains(&KeyCode::FORWARD_DELETE));
            assert!(!keycodes.contains(&KeyCode::ANSI_A));
        }
    }

    #[test]
    fn clear_line_shortcut_uses_control_not_command() {
        assert_ne!(
            CGEventFlags::CGEventFlagControl,
            CGEventFlags::CGEventFlagCommand
        );
        assert_eq!(KeyCode::ANSI_A, 0);
        assert_eq!(KeyCode::ANSI_K, 40);
    }

    #[test]
    fn clear_current_input_prioritizes_accessibility_value_reset() {
        let steps = clear_input_steps(1);
        assert_eq!(
            steps.first(),
            Some(&ClearInputStep::AccessibilityFocusedValueReset)
        );
        assert!(steps.contains(&ClearInputStep::KeyboardLineClear));
        assert!(steps.contains(&ClearInputStep::DeleteKey));
        assert!(steps.contains(&ClearInputStep::ForwardDeleteKey));
    }

    #[test]
    fn existing_thread_send_points_cover_side_panel_and_full_width_layouts() {
        let window = CodexWindow {
            pid: 123,
            window_id: 456,
            x: 0.0,
            y: 0.0,
            width: 1792.0,
            height: 1280.0,
        };
        let points = existing_thread_send_button_points(&window, 0);

        assert_eq!(points.len(), 2);
        assert!((points[0].x - 1144.0).abs() <= 2.0);
        assert!((points[1].x - 1354.0).abs() <= 2.0);
        assert!(points.iter().all(|point| (point.y - 1196.0).abs() <= 2.0));
    }

    #[test]
    fn existing_thread_open_settle_is_short() {
        assert!(EXISTING_THREAD_OPEN_SETTLE <= Duration::from_millis(900));
        assert!(COMPOSER_AFTER_FOCUS_SETTLE <= Duration::from_millis(180));
        assert!(COMPOSER_AFTER_INSERT_SETTLE <= Duration::from_millis(180));
    }

    #[test]
    fn detects_red_failure_marker_left_of_thread_title() {
        let path = marker_fixture("red-left", |image| {
            fill_rect(image, 76, 150, 10, 24, Rgba([230, 48, 48, 255]));
            fill_rect(image, 72, 168, 18, 8, Rgba([230, 48, 48, 255]));
        });

        assert_eq!(state_for_fixture(&path), VisibleThreadFailureState::Failed);
    }

    #[test]
    fn detects_red_failure_marker_in_sidebar_status_slot() {
        let path = marker_fixture("red-status", |image| {
            fill_rect(image, 542, 150, 22, 22, Rgba([225, 58, 58, 255]));
        });

        assert_eq!(state_for_fixture(&path), VisibleThreadFailureState::Failed);
    }

    #[test]
    fn detects_blue_stopped_marker_in_sidebar_status_slot() {
        let path = marker_fixture("blue-status", |image| {
            fill_rect(image, 550, 154, 12, 12, Rgba([52, 138, 255, 255]));
        });

        assert_eq!(
            state_for_fixture(&path),
            VisibleThreadFailureState::StoppedMarker
        );
    }

    #[test]
    fn red_failure_marker_wins_over_blue_stopped_marker() {
        let path = marker_fixture("red-and-blue", |image| {
            fill_rect(image, 76, 150, 10, 24, Rgba([230, 48, 48, 255]));
            fill_rect(image, 550, 154, 12, 12, Rgba([52, 138, 255, 255]));
        });

        assert_eq!(state_for_fixture(&path), VisibleThreadFailureState::Failed);
    }

    #[test]
    fn ignores_red_pixels_in_main_chat_content() {
        let path = marker_fixture("red-content", |image| {
            fill_rect(image, 700, 180, 18, 18, Rgba([230, 48, 48, 255]));
        });

        assert_eq!(
            state_for_fixture(&path),
            VisibleThreadFailureState::NotFailed
        );
    }

    #[test]
    fn ignores_gray_sidebar_spinner() {
        let path = marker_fixture("gray-status", |image| {
            fill_rect(image, 542, 150, 22, 22, Rgba([135, 135, 135, 255]));
        });

        assert_eq!(
            state_for_fixture(&path),
            VisibleThreadFailureState::NotFailed
        );
    }
}
