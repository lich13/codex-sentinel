use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::size_of;
use std::process::Command;
use std::ptr::{copy_nonoverlapping, null_mut};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use sysinfo::System;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_RETURN,
    VK_V,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST,
    IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow,
};

const PW_RENDERFULLCONTENT: u32 = 2;
const CF_UNICODETEXT: u32 = 13;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    let window = find_codex_window();
    let input_ready = visible_input_ready();
    let state_ready = window.as_ref().is_some_and(|window| !window.minimized);
    let mut notes = Vec::new();

    if let Some(window) = &window {
        if window.minimized {
            notes.push(
                "Codex window is minimized; visible recovery cannot inspect or send until it is restored."
                    .to_string(),
            );
        }
    } else {
        notes.push("Codex main window was not found through Win32 HWND enumeration.".to_string());
    }

    if window.is_some() && !input_ready {
        notes.push(
            "Visible prompt submission becomes available once the Codex window is visible and not minimized."
                .to_string(),
        );
    }

    DesktopControlStatus {
        mode: "visible_desktop_windows".to_string(),
        accessibility_granted: input_ready,
        screen_recording_granted: state_ready,
        notes,
    }
}

pub fn open_permission_settings() -> Result<()> {
    Ok(())
}

pub fn visible_input_ready() -> bool {
    visible_state_ready()
}

pub fn visible_state_ready() -> bool {
    find_codex_window().is_some_and(|window| !window.minimized)
}

pub fn debug_visible_send_plan() -> VisibleSendDebugPlan {
    let window = find_codex_window();
    let mut notes = Vec::new();
    let (
        window_debug,
        existing_thread_focus,
        existing_thread_send_points,
        new_thread_focus,
        new_thread_send_points,
    ) = if let Some(window) = &window {
        if window.minimized {
            notes.push(
                "Codex window is minimized; real visible send should restore the window first."
                    .to_string(),
            );
        }
        (
            Some(VisibleWindowDebug::from(window)),
            Some(focus_point(window, ComposerPlacement::ExistingThread)),
            (0..VISIBLE_SUBMIT_DEBUG_ATTEMPTS)
                .map(|attempt| {
                    send_button_point(window, ComposerPlacement::ExistingThread, attempt)
                })
                .collect(),
            Some(focus_point(window, ComposerPlacement::NewThread)),
            (0..VISIBLE_SUBMIT_DEBUG_ATTEMPTS)
                .map(|attempt| send_button_point(window, ComposerPlacement::NewThread, attempt))
                .collect(),
        )
    } else {
        notes.push("Codex main window was not found; no send points can be planned.".to_string());
        (None, None, Vec::new(), None, Vec::new())
    };

    notes.push(
        "This is a dry-run plan only; it does not paste text, click, press keys, or submit."
            .to_string(),
    );

    VisibleSendDebugPlan {
        platform: "windows".to_string(),
        send_enabled: visible_input_ready(),
        window: window_debug,
        existing_thread_focus,
        existing_thread_send_points,
        new_thread_focus,
        new_thread_send_points,
        notes,
    }
}

pub fn prepare_existing_thread_visible(thread_id: &str) -> Result<()> {
    let uri = format!("codex://threads/{thread_id}");
    open_uri(&uri)?;
    let window = wait_for_codex_window(Duration::from_secs(5))?;
    show_window_best_effort(&window);
    thread::sleep(Duration::from_millis(1_400));
    Ok(())
}

pub fn inspect_thread_failure_state(thread_id: &str) -> Result<VisibleThreadFailureState> {
    prepare_existing_thread_visible(thread_id)?;
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    let image = capture_window(&window)?;
    image_thread_failure_state(&image)
}

pub fn prepare_new_thread_visible(path: Option<&str>) -> Result<()> {
    open_codex_app(path)?;
    let window = wait_for_codex_window(Duration::from_secs(5))?;
    activate_window_for_input(&window)?;
    thread::sleep(Duration::from_millis(220));

    let button = top_new_thread_button_point(&window);
    click_screen_point(button.x, button.y)?;
    wait_for_new_thread_page(&window, Duration::from_secs(4))
}

pub fn submit_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    ensure_send_enabled()?;
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    activate_window_for_input(&window)?;
    submit_prompt_by_keyboard(&window, prompt, attempt, ComposerPlacement::ExistingThread)
}

pub fn submit_new_thread_prompt_to_visible_window(prompt: &str, attempt: usize) -> Result<()> {
    ensure_send_enabled()?;
    let window = wait_for_codex_window(Duration::from_secs(4))?;
    activate_window_for_input(&window)?;
    submit_prompt_by_keyboard(&window, prompt, attempt, ComposerPlacement::NewThread)
}

pub fn visible_turn_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("visible-desktop-{millis}")
}

#[derive(Debug, Clone)]
struct CodexWindow {
    hwnd: HWND,
    pid: u32,
    title: String,
    class_name: String,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    visible: bool,
    minimized: bool,
}

impl CodexWindow {
    fn area(&self) -> i64 {
        i64::from(self.width.max(0)) * i64::from(self.height.max(0))
    }
}

impl From<&CodexWindow> for VisibleWindowDebug {
    fn from(window: &CodexWindow) -> Self {
        Self {
            hwnd: format!("0x{:X}", window.hwnd as usize),
            pid: window.pid,
            title: window.title.clone(),
            class_name: window.class_name.clone(),
            left: window.left,
            top: window.top,
            width: window.width,
            height: window.height,
            visible: window.visible,
            minimized: window.minimized,
        }
    }
}

#[derive(Debug, Clone)]
struct CapturedImage {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

impl CapturedImage {
    fn rgb_at(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let index = (y * self.width + x) * 4;
        (self.bgra[index + 2], self.bgra[index + 1], self.bgra[index])
    }

    fn is_mostly_blank(&self) -> bool {
        if self.width == 0 || self.height == 0 {
            return true;
        }
        let step_x = (self.width / 80).max(1);
        let step_y = (self.height / 80).max(1);
        let mut samples = 0usize;
        let mut non_dark = 0usize;
        let mut y = 0usize;
        while y < self.height {
            let mut x = 0usize;
            while x < self.width {
                let (r, g, b) = self.rgb_at(x, y);
                samples += 1;
                if u16::from(r) + u16::from(g) + u16::from(b) > 30 {
                    non_dark += 1;
                }
                x += step_x;
            }
            y += step_y;
        }
        samples == 0 || (non_dark as f64 / samples as f64) < 0.02
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerPlacement {
    ExistingThread,
    NewThread,
}

const VISIBLE_SUBMIT_DEBUG_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Copy)]
struct SelectedRow {
    y_start: usize,
    y_end: usize,
}

#[derive(Debug, Clone, Copy)]
struct MarkerRegion {
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
}

#[derive(Debug, Clone, Copy)]
struct MarkerLimits {
    min_pixels: usize,
    min_width: usize,
    max_width: usize,
    min_height: usize,
    max_height: usize,
}

fn open_uri(uri: &str) -> Result<()> {
    let mut command = codex_uri_launch_command(uri);
    prepare_hidden_launch_command(&mut command);
    let status = command
        .status()
        .with_context(|| format!("failed to open {uri}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("opening {uri} failed with {status}"))
    }
}

fn open_codex_app(path: Option<&str>) -> Result<()> {
    let launch = codex_visible_launch_strategy(path);
    let mut command = codex_visible_launch_command(&launch);
    let status = command.status().with_context(|| {
        if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
            format!("failed to launch Codex visible app for {path}")
        } else {
            "failed to launch Codex visible app".to_string()
        }
    })?;
    if status.success() {
        Ok(())
    } else if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
        Err(anyhow!("Codex visible app for {path} failed with {status}"))
    } else {
        Err(anyhow!("Codex visible app launch failed with {status}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexVisibleLaunch {
    Protocol(&'static str),
    HiddenCliApp(String),
}

fn codex_visible_launch_strategy(path: Option<&str>) -> CodexVisibleLaunch {
    match path.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => CodexVisibleLaunch::HiddenCliApp(path.to_string()),
        None => CodexVisibleLaunch::Protocol("codex://"),
    }
}

fn codex_visible_launch_command(launch: &CodexVisibleLaunch) -> Command {
    let mut command = match launch {
        CodexVisibleLaunch::Protocol(uri) => codex_uri_launch_command(uri),
        CodexVisibleLaunch::HiddenCliApp(path) => hidden_cli_app_launch_command(path),
    };
    prepare_hidden_launch_command(&mut command);
    command
}

fn hidden_cli_app_launch_command(path: &str) -> Command {
    hidden_cli_app_launch_command_with_cli(crate::app_server_probe::codex_cli_path(), path)
}

fn hidden_cli_app_launch_command_with_cli(cli: impl AsRef<std::ffi::OsStr>, path: &str) -> Command {
    let mut command = Command::new(cli);
    command.args(["app", path]);
    command
}

fn codex_uri_launch_command(uri: &str) -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "start", "", uri]);
    command
}

fn prepare_hidden_launch_command(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(test)]
fn windows_hidden_launch_creation_flags() -> u32 {
    CREATE_NO_WINDOW
}

fn wait_for_codex_window(timeout: Duration) -> Result<CodexWindow> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = find_codex_window() {
            return Ok(window);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Codex main window not found through Win32 after opening visible thread"
            ));
        }
        thread::sleep(Duration::from_millis(120));
    }
}

fn find_codex_window() -> Option<CodexWindow> {
    let mut windows = Vec::<CodexWindow>::new();
    unsafe {
        EnumWindows(
            Some(enum_windows_proc),
            (&mut windows as *mut Vec<CodexWindow>) as LPARAM,
        );
    }
    let codex_pids = codex_app_pids();
    windows.sort_by_key(|window| std::cmp::Reverse(window.area()));
    windows
        .into_iter()
        .find(|window| is_codex_main_window(window, &codex_pids))
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    let windows = unsafe { &mut *(lparam as *mut Vec<CodexWindow>) };
    if let Some(info) = window_info(hwnd) {
        windows.push(info);
    }
    1
}

fn window_info(hwnd: HWND) -> Option<CodexWindow> {
    let mut rect = RECT::default();
    let got_rect = unsafe { GetWindowRect(hwnd, &mut rect) };
    if got_rect == 0 {
        return None;
    }
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    if width <= 0 || height <= 0 {
        return None;
    }

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }

    Some(CodexWindow {
        hwnd,
        pid,
        title: window_text(hwnd),
        class_name: window_class(hwnd),
        left: rect.left,
        top: rect.top,
        width,
        height,
        visible: unsafe { IsWindowVisible(hwnd) != 0 },
        minimized: unsafe { IsIconic(hwnd) != 0 },
    })
}

fn window_text(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let capacity = len.max(0) as usize + 1;
    let mut buffer = vec![0u16; capacity];
    let read = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..read.max(0) as usize])
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = vec![0u16; 256];
    let read = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..read.max(0) as usize])
}

fn codex_app_pids() -> HashSet<u32> {
    let mut sys = System::new_all();
    sys.refresh_all();
    sys.processes()
        .values()
        .filter_map(|process| {
            let cmd = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let name = process.name().to_string_lossy();
            crate::codex::is_codex_app_process(&name, &cmd).then_some(process.pid().as_u32())
        })
        .collect()
}

fn is_codex_main_window(window: &CodexWindow, codex_pids: &HashSet<u32>) -> bool {
    window.visible
        && window.width >= 500
        && window.height >= 350
        && window.class_name == "Chrome_WidgetWin_1"
        && (codex_pids.contains(&window.pid) || window.title == "Codex")
}

fn show_window_best_effort(window: &CodexWindow) {
    unsafe {
        if window.minimized {
            ShowWindow(window.hwnd, SW_RESTORE);
        } else {
            ShowWindow(window.hwnd, SW_SHOW);
        }
        raise_window_for_input(window);
        SetForegroundWindow(window.hwnd);
    }
}

fn raise_window_for_input(window: &CodexWindow) {
    unsafe {
        SetWindowPos(window.hwnd, HWND_TOPMOST, 0, 0, 0, 0, window_raise_flags());
        SetWindowPos(
            window.hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            window_raise_flags(),
        );
    }
}

fn window_raise_flags() -> u32 {
    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW
}

fn activate_window_for_input(window: &CodexWindow) -> Result<()> {
    show_window_best_effort(window);
    thread::sleep(Duration::from_millis(250));
    if foreground_matches_window(window)? {
        return Ok(());
    }

    let point = window_activation_point(window);
    click_screen_point(point.x, point.y)?;
    thread::sleep(Duration::from_millis(250));
    if foreground_matches_window(window)? {
        return Ok(());
    }

    Err(anyhow!(
        "Codex window did not become foreground after activation; visible input is unsafe"
    ))
}

fn foreground_matches_window(window: &CodexWindow) -> Result<bool> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Err(anyhow!("Windows did not report a foreground window"));
    }
    Ok(foreground == window.hwnd || foreground_pid(foreground) == Some(window.pid))
}

fn foreground_pid(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    (pid != 0).then_some(pid)
}

fn capture_window(window: &CodexWindow) -> Result<CapturedImage> {
    if window.minimized {
        return Err(anyhow!(
            "Codex window is minimized; Windows visible recovery cannot inspect minimized Electron content reliably"
        ));
    }
    let width = usize::try_from(window.width).context("invalid Codex window width")?;
    let height = usize::try_from(window.height).context("invalid Codex window height")?;
    if width == 0 || height == 0 {
        return Err(anyhow!("Codex window has an empty rectangle"));
    }

    let hdc = unsafe { CreateCompatibleDC(null_mut()) };
    if hdc.is_null() {
        return Err(anyhow!(
            "CreateCompatibleDC failed for Codex window capture"
        ));
    }

    let mut bits: *mut c_void = null_mut();
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: window.width,
        biHeight: -window.height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..BITMAPINFOHEADER::default()
    };

    let bitmap = unsafe { CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
    if bitmap.is_null() || bits.is_null() {
        unsafe {
            DeleteDC(hdc);
        }
        return Err(anyhow!("CreateDIBSection failed for Codex window capture"));
    }

    let old = unsafe { SelectObject(hdc, bitmap as HGDIOBJ) };
    let ok = unsafe { PrintWindow(window.hwnd, hdc, PW_RENDERFULLCONTENT) != 0 };
    let byte_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("Codex window capture is too large"))?;
    let mut bgra = vec![0u8; byte_len];
    unsafe {
        copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), byte_len);
        if !old.is_null() {
            SelectObject(hdc, old);
        }
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(hdc);
    }

    if !ok {
        return Err(anyhow!("PrintWindow failed for Codex window capture"));
    }

    let image = CapturedImage {
        width,
        height,
        bgra,
    };
    if image.is_mostly_blank() {
        return Err(anyhow!(
            "Codex PrintWindow capture was blank; the window may be minimized, locked, or unavailable to desktop capture"
        ));
    }
    Ok(image)
}

fn image_thread_failure_state(image: &CapturedImage) -> Result<VisibleThreadFailureState> {
    let selected_row = locate_selected_sidebar_row(image).ok_or_else(|| {
        anyhow!(
            "could not locate the selected Codex thread row in the left sidebar; refusing broad pixel classification"
        )
    })?;
    let (left_icon, status_icon) = marker_regions(image.width, image.height, selected_row);

    if has_marker_component(
        image,
        left_icon,
        is_codex_error_red,
        MarkerLimits::failure(),
    ) || has_marker_component(
        image,
        status_icon,
        is_codex_error_red,
        MarkerLimits::failure(),
    ) {
        return Ok(VisibleThreadFailureState::Failed);
    }

    if has_marker_component(
        image,
        status_icon,
        is_codex_stopped_blue,
        MarkerLimits::stopped(),
    ) {
        return Ok(VisibleThreadFailureState::StoppedMarker);
    }

    Ok(VisibleThreadFailureState::NotFailed)
}

fn locate_selected_sidebar_row(image: &CapturedImage) -> Option<SelectedRow> {
    let sidebar_width = sidebar_width(image.width);
    if sidebar_width < 80 || image.height < 220 {
        return None;
    }
    let x_start = 12usize.min(sidebar_width.saturating_sub(1));
    let x_end = sidebar_width.saturating_sub(8).max(x_start + 1);
    let step = 3usize;
    let sample_count = ((x_end - x_start) / step).max(1);
    let threshold = (sample_count * 42 / 100).max(18);
    let y_start = (image.height / 5)
        .max(150)
        .min(image.height.saturating_sub(1));
    let y_end = image.height.saturating_sub(88).max(y_start + 1);

    let mut best: Option<(usize, usize, usize)> = None;
    let mut band_start = None::<usize>;
    let mut band_best = 0usize;
    let mut band_total = 0usize;
    for y in y_start..y_end {
        let mut score = 0usize;
        let mut x = x_start;
        while x < x_end {
            let (r, g, b) = image.rgb_at(x, y);
            if is_selected_sidebar_fill(r, g, b) {
                score += 1;
            }
            x += step;
        }

        if score >= threshold {
            if band_start.is_none() {
                band_start = Some(y);
                band_best = score;
                band_total = 0;
            }
            band_best = band_best.max(score);
            band_total += score;
        } else if let Some(start) = band_start.take() {
            update_selected_row_candidate(&mut best, start, y, band_best, band_total);
        }
    }
    if let Some(start) = band_start {
        update_selected_row_candidate(&mut best, start, y_end, band_best, band_total);
    }

    best.map(|(start, end, _)| SelectedRow {
        y_start: start.saturating_sub(4),
        y_end: (end + 4).min(image.height),
    })
}

fn update_selected_row_candidate(
    best: &mut Option<(usize, usize, usize)>,
    start: usize,
    end: usize,
    band_best: usize,
    band_total: usize,
) {
    let height = end.saturating_sub(start);
    if !(14..=64).contains(&height) {
        return;
    }
    let score = band_best.saturating_mul(1000) + band_total / height.max(1);
    if best.is_none_or(|(_, _, best_score)| score > best_score) {
        *best = Some((start, end, score));
    }
}

fn is_selected_sidebar_fill(r: u8, g: u8, b: u8) -> bool {
    (226..=250).contains(&r)
        && (226..=250).contains(&g)
        && (226..=250).contains(&b)
        && r.abs_diff(g) <= 6
        && r.abs_diff(b) <= 6
        && g.abs_diff(b) <= 6
}

fn sidebar_width(width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    let proportional = width.saturating_mul(18) / 100;
    proportional.clamp(260, 340).min(width)
}

fn marker_regions(width: usize, height: usize, row: SelectedRow) -> (MarkerRegion, MarkerRegion) {
    let sidebar_width = sidebar_width(width);
    let y_start = row.y_start.min(height);
    let y_end = row.y_end.min(height).max(y_start + 1);
    let left_icon = clamp_region(
        MarkerRegion {
            x_start: 8,
            x_end: 104.min(sidebar_width),
            y_start,
            y_end,
        },
        width,
        height,
    );
    let status_icon = clamp_region(
        MarkerRegion {
            x_start: sidebar_width.saturating_sub(76),
            x_end: sidebar_width.saturating_sub(6).max(1),
            y_start,
            y_end,
        },
        width,
        height,
    );
    (left_icon, status_icon)
}

fn clamp_region(region: MarkerRegion, width: usize, height: usize) -> MarkerRegion {
    let x_start = region.x_start.min(width);
    let y_start = region.y_start.min(height);
    MarkerRegion {
        x_start,
        x_end: region.x_end.min(width).max(x_start),
        y_start,
        y_end: region.y_end.min(height).max(y_start),
    }
}

fn has_marker_component<F>(
    image: &CapturedImage,
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

    let mut mask = vec![false; width * height];
    for local_y in 0..height {
        for local_x in 0..width {
            let (r, g, b) = image.rgb_at(region.x_start + local_x, region.y_start + local_y);
            mask[local_y * width + local_x] = predicate(r, g, b);
        }
    }

    let mut visited = vec![false; mask.len()];
    for index in 0..mask.len() {
        if !mask[index] || visited[index] {
            continue;
        }

        let mut stack = vec![index];
        visited[index] = true;
        let mut pixels = 0usize;
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0usize;
        let mut max_y = 0usize;

        while let Some(current) = stack.pop() {
            let x = current % width;
            let y = current / width;
            pixels += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);

            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            for next_y in y0..=y1 {
                for next_x in x0..=x1 {
                    if next_x == x && next_y == y {
                        continue;
                    }
                    let next = next_y * width + next_x;
                    if mask[next] && !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
        }

        let component_width = max_x.saturating_sub(min_x) + 1;
        let component_height = max_y.saturating_sub(min_y) + 1;
        if pixels >= limits.min_pixels
            && (limits.min_width..=limits.max_width).contains(&component_width)
            && (limits.min_height..=limits.max_height).contains(&component_height)
        {
            return true;
        }
    }

    false
}

impl MarkerLimits {
    fn failure() -> Self {
        Self {
            min_pixels: 8,
            min_width: 2,
            max_width: 34,
            min_height: 4,
            max_height: 34,
        }
    }

    fn stopped() -> Self {
        Self {
            min_pixels: 6,
            min_width: 3,
            max_width: 22,
            min_height: 3,
            max_height: 22,
        }
    }
}

fn is_codex_error_red(r: u8, g: u8, b: u8) -> bool {
    r >= 190 && g <= 120 && b <= 120 && r.saturating_sub(g) >= 70 && r.saturating_sub(b) >= 70
}

fn is_codex_stopped_blue(r: u8, g: u8, b: u8) -> bool {
    b >= 150 && g >= 80 && r <= 140 && b.saturating_sub(r) >= 45
}

fn ensure_send_enabled() -> Result<()> {
    if visible_input_ready() {
        Ok(())
    } else {
        Err(anyhow!(
            "Windows visible prompt submission needs a visible Codex window that is not minimized"
        ))
    }
}

fn submit_prompt_by_keyboard(
    window: &CodexWindow,
    prompt: &str,
    attempt: usize,
    placement: ComposerPlacement,
) -> Result<()> {
    write_clipboard(prompt)?;
    focus_composer_by_coordinate(window, placement)?;
    post_chord(VK_CONTROL, VK_V)?;
    thread::sleep(Duration::from_millis(180));
    if placement == ComposerPlacement::NewThread
        && locate_send_button_point(window, placement).is_none()
    {
        return Err(anyhow!(
            "Codex new-thread composer was not ready after paste; refusing to send into an unknown page"
        ));
    }
    match attempt % 4 {
        0 | 2 => click_send_button(window, placement, attempt)?,
        1 => post_key_press(VK_RETURN)?,
        _ => post_chord(VK_CONTROL, VK_RETURN)?,
    }
    thread::sleep(Duration::from_millis(420));
    Ok(())
}

fn focus_composer_by_coordinate(window: &CodexWindow, placement: ComposerPlacement) -> Result<()> {
    let point = focus_point(window, placement);
    click_screen_point(point.x, point.y)
}

fn focus_point(window: &CodexWindow, placement: ComposerPlacement) -> ScreenPoint {
    let (x, y) = match placement {
        ComposerPlacement::ExistingThread => (
            window.left + (f64::from(window.width) * 0.55).round() as i32,
            window.top + window.height - 128,
        ),
        ComposerPlacement::NewThread => (
            window.left + (f64::from(window.width) * 0.56).round() as i32,
            window.top + (f64::from(window.height) * 0.472).round() as i32,
        ),
    };
    ScreenPoint { x, y }
}

fn click_send_button(
    window: &CodexWindow,
    placement: ComposerPlacement,
    attempt: usize,
) -> Result<()> {
    let point = locate_send_button_point(window, placement)
        .unwrap_or_else(|| send_button_point(window, placement, attempt));
    click_screen_point(point.x, point.y)
}

fn send_button_point(
    window: &CodexWindow,
    placement: ComposerPlacement,
    attempt: usize,
) -> ScreenPoint {
    let width = f64::from(window.width);
    let height = f64::from(window.height);
    let (x_factor, y_factor) = match placement {
        ComposerPlacement::ExistingThread => match attempt % 2 {
            0 => (0.760, 0.938),
            _ => (0.755, 0.941),
        },
        ComposerPlacement::NewThread => match attempt % 2 {
            0 => (0.760, 0.523),
            _ => (0.755, 0.528),
        },
    };
    ScreenPoint {
        x: window.left + (width * x_factor).round() as i32,
        y: window.top + (height * y_factor).round() as i32,
    }
}

fn top_new_thread_button_point(window: &CodexWindow) -> ScreenPoint {
    ScreenPoint {
        x: window.left + 40,
        y: window.top + 76,
    }
}

fn window_activation_point(window: &CodexWindow) -> ScreenPoint {
    ScreenPoint {
        x: window.left + window.width / 2,
        y: window.top + 24,
    }
}

fn wait_for_new_thread_page(window: &CodexWindow, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(image) = capture_window(window) {
            if image_new_thread_page_ready(&image) {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "Codex new-thread page was not detected after clicking New Chat; refusing to send into the current thread"
            ));
        }
        thread::sleep(Duration::from_millis(180));
    }
}

fn image_new_thread_page_ready(image: &CapturedImage) -> bool {
    if image.width < 900 || image.height < 650 {
        return false;
    }

    let quiet_upper_main = dark_ratio(
        image,
        proportional_region(image.width, image.height, 0.340, 0.090, 0.775, 0.330),
    );
    let center_composer_fill = light_gray_ratio(
        image,
        proportional_region(image.width, image.height, 0.380, 0.430, 0.770, 0.590),
    );
    let bottom_composer_fill = light_gray_ratio(
        image,
        proportional_region(image.width, image.height, 0.380, 0.835, 0.770, 0.980),
    );

    quiet_upper_main <= 0.006 && center_composer_fill >= 0.12 && bottom_composer_fill <= 0.02
}

fn proportional_region(
    width: usize,
    height: usize,
    x_start: f64,
    y_start: f64,
    x_end: f64,
    y_end: f64,
) -> MarkerRegion {
    clamp_region(
        MarkerRegion {
            x_start: (width as f64 * x_start).round() as usize,
            x_end: (width as f64 * x_end).round() as usize,
            y_start: (height as f64 * y_start).round() as usize,
            y_end: (height as f64 * y_end).round() as usize,
        },
        width,
        height,
    )
}

fn light_gray_ratio(image: &CapturedImage, region: MarkerRegion) -> f64 {
    color_ratio(image, region, |r, g, b| {
        (228..=250).contains(&r)
            && (228..=250).contains(&g)
            && (228..=250).contains(&b)
            && r.abs_diff(g) <= 8
            && r.abs_diff(b) <= 8
            && g.abs_diff(b) <= 8
    })
}

fn dark_ratio(image: &CapturedImage, region: MarkerRegion) -> f64 {
    color_ratio(image, region, |r, g, b| r <= 80 && g <= 80 && b <= 80)
}

fn color_ratio<F>(image: &CapturedImage, region: MarkerRegion, predicate: F) -> f64
where
    F: Fn(u8, u8, u8) -> bool,
{
    let width = region.x_end.saturating_sub(region.x_start);
    let height = region.y_end.saturating_sub(region.y_start);
    let total = width.saturating_mul(height);
    if total == 0 {
        return 0.0;
    }
    let mut hits = 0usize;
    for y in region.y_start..region.y_end {
        for x in region.x_start..region.x_end {
            let (r, g, b) = image.rgb_at(x, y);
            if predicate(r, g, b) {
                hits += 1;
            }
        }
    }
    hits as f64 / total as f64
}

fn locate_send_button_point(
    window: &CodexWindow,
    placement: ComposerPlacement,
) -> Option<ScreenPoint> {
    let image = capture_window(window).ok()?;
    let region = send_button_search_region(image.width, image.height, placement);
    locate_dark_button_component(&image, region).map(|point| ScreenPoint {
        x: window.left + point.x,
        y: window.top + point.y,
    })
}

fn send_button_search_region(
    width: usize,
    height: usize,
    placement: ComposerPlacement,
) -> MarkerRegion {
    let (y_start, y_end) = match placement {
        ComposerPlacement::ExistingThread => (
            height.saturating_mul(88) / 100,
            height.saturating_mul(98) / 100,
        ),
        ComposerPlacement::NewThread => (
            height.saturating_mul(40) / 100,
            height.saturating_mul(76) / 100,
        ),
    };
    clamp_region(
        MarkerRegion {
            x_start: width.saturating_mul(66) / 100,
            x_end: width.saturating_mul(82) / 100,
            y_start,
            y_end,
        },
        width,
        height,
    )
}

fn locate_dark_button_component(
    image: &CapturedImage,
    region: MarkerRegion,
) -> Option<ScreenPoint> {
    let width = region.x_end.saturating_sub(region.x_start);
    let height = region.y_end.saturating_sub(region.y_start);
    if width == 0 || height == 0 {
        return None;
    }

    let mut mask = vec![false; width * height];
    for local_y in 0..height {
        for local_x in 0..width {
            let (r, g, b) = image.rgb_at(region.x_start + local_x, region.y_start + local_y);
            mask[local_y * width + local_x] = is_send_button_dark(r, g, b);
        }
    }

    let mut visited = vec![false; mask.len()];
    let mut best = None::<(usize, usize, usize, usize, usize)>;
    for index in 0..mask.len() {
        if !mask[index] || visited[index] {
            continue;
        }

        let mut stack = vec![index];
        visited[index] = true;
        let mut pixels = 0usize;
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0usize;
        let mut max_y = 0usize;

        while let Some(current) = stack.pop() {
            let x = current % width;
            let y = current / width;
            pixels += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);

            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            for next_y in y0..=y1 {
                for next_x in x0..=x1 {
                    if next_x == x && next_y == y {
                        continue;
                    }
                    let next = next_y * width + next_x;
                    if mask[next] && !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
        }

        let component_width = max_x.saturating_sub(min_x) + 1;
        let component_height = max_y.saturating_sub(min_y) + 1;
        if pixels >= 140
            && (18..=56).contains(&component_width)
            && (18..=56).contains(&component_height)
            && component_width.abs_diff(component_height) <= 16
            && best.is_none_or(|(_, _, _, _, best_pixels)| pixels > best_pixels)
        {
            best = Some((min_x, min_y, max_x, max_y, pixels));
        }
    }

    best.map(|(min_x, min_y, max_x, max_y, _)| ScreenPoint {
        x: (region.x_start + ((min_x + max_x) / 2)) as i32,
        y: (region.y_start + ((min_y + max_y) / 2)) as i32,
    })
}

fn is_send_button_dark(r: u8, g: u8, b: u8) -> bool {
    r <= 70 && g <= 70 && b <= 70
}

fn click_screen_point(x: i32, y: i32) -> Result<()> {
    let mut original = POINT::default();
    let restore_cursor = unsafe { GetCursorPos(&mut original) != 0 };
    let moved = unsafe { SetCursorPos(x, y) != 0 };
    if !moved {
        return Err(anyhow!("SetCursorPos failed while focusing Codex composer"));
    }
    post_mouse_button(false)?;
    post_mouse_button(true)?;
    if restore_cursor {
        unsafe {
            SetCursorPos(original.x, original.y);
        }
    }
    thread::sleep(Duration::from_millis(160));
    Ok(())
}

fn write_clipboard(text: &str) -> Result<()> {
    let wide = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let byte_len = wide
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| anyhow!("clipboard text is too large"))?;

    let opened = unsafe { OpenClipboard(null_mut()) != 0 };
    if !opened {
        return Err(anyhow!("OpenClipboard failed"));
    }
    let _guard = ClipboardGuard;

    if unsafe { EmptyClipboard() == 0 } {
        return Err(anyhow!("EmptyClipboard failed"));
    }

    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_len) };
    if handle.is_null() {
        return Err(anyhow!("GlobalAlloc failed for clipboard text"));
    }
    let locked = unsafe { GlobalLock(handle) };
    if locked.is_null() {
        return Err(anyhow!("GlobalLock failed for clipboard text"));
    }
    unsafe {
        copy_nonoverlapping(wide.as_ptr() as *const u8, locked as *mut u8, byte_len);
        GlobalUnlock(handle);
    }

    let stored = unsafe { SetClipboardData(CF_UNICODETEXT, handle) };
    if stored.is_null() {
        return Err(anyhow!("SetClipboardData failed for unicode text"));
    }
    Ok(())
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

fn post_chord(modifier: u16, key: u16) -> Result<()> {
    send_keyboard_inputs(&[
        key_input(modifier, false),
        key_input(key, false),
        key_input(key, true),
        key_input(modifier, true),
    ])
}

fn post_key_press(key: u16) -> Result<()> {
    send_keyboard_inputs(&[key_input(key, false), key_input(key, true)])
}

fn post_mouse_button(keyup: bool) -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    };
    let flags = if keyup {
        MOUSEEVENTF_LEFTUP
    } else {
        MOUSEEVENTF_LEFTDOWN
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_inputs(&[input])
}

fn key_input(key: u16, keyup: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if keyup { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_keyboard_inputs(inputs: &[INPUT]) -> Result<()> {
    send_inputs(inputs)?;
    thread::sleep(Duration::from_millis(35));
    Ok(())
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(anyhow!(
            "SendInput sent {sent} of {} requested input events",
            inputs.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> CapturedImage {
        let mut image = solid_image(1400, 700, [255, 255, 255]);
        fill_rect(&mut image, 16, 180, 236, 32, [243, 243, 243]);
        image
    }

    fn solid_image(width: usize, height: usize, rgb: [u8; 3]) -> CapturedImage {
        let mut bgra = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            bgra.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
        CapturedImage {
            width,
            height,
            bgra,
        }
    }

    fn fill_rect(image: &mut CapturedImage, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
        for yy in y..(y + h).min(image.height) {
            for xx in x..(x + w).min(image.width) {
                let index = (yy * image.width + xx) * 4;
                image.bgra[index] = rgb[2];
                image.bgra[index + 1] = rgb[1];
                image.bgra[index + 2] = rgb[0];
                image.bgra[index + 3] = 255;
            }
        }
    }

    fn test_window() -> CodexWindow {
        CodexWindow {
            hwnd: null_mut(),
            pid: 42,
            title: "Codex".to_string(),
            class_name: "Chrome_WidgetWin_1".to_string(),
            left: -8,
            top: -8,
            width: 1936,
            height: 1056,
            visible: true,
            minimized: false,
        }
    }

    #[test]
    fn plans_existing_thread_focus_above_send_button() {
        let window = test_window();
        let focus = focus_point(&window, ComposerPlacement::ExistingThread);
        let send = send_button_point(&window, ComposerPlacement::ExistingThread, 0);

        assert!(focus.x > window.left + 700);
        assert!(focus.x < window.left + 1300);
        assert!(focus.y < send.y);
        assert!(send.y > window.top + window.height - 110);
    }

    #[test]
    fn plans_new_thread_focus_and_send_near_centered_composer() {
        let window = test_window();
        let focus = focus_point(&window, ComposerPlacement::NewThread);
        let send = send_button_point(&window, ComposerPlacement::NewThread, 0);

        assert!(focus.x > window.left + 700);
        assert!(focus.x < window.left + 1300);
        assert!(focus.y > window.top + window.height / 3);
        assert!(focus.y < window.top + (window.height * 3 / 5));
        assert!(send.y > focus.y);
        assert!(send.y < window.top + (window.height * 3 / 5));
    }

    #[test]
    fn locates_dark_send_button_component_in_composer_region() {
        let mut image = solid_image(1936, 1056, [255, 255, 255]);
        fill_rect(&mut image, 1454, 974, 32, 32, [18, 18, 18]);
        let region =
            send_button_search_region(image.width, image.height, ComposerPlacement::ExistingThread);
        let point = locate_dark_button_component(&image, region).expect("send button");

        assert!((1468..=1472).contains(&point.x));
        assert!((988..=992).contains(&point.y));
    }

    #[test]
    fn detects_selected_sidebar_row() {
        let image = fixture();
        let row = locate_selected_sidebar_row(&image).expect("selected row");
        assert!(row.y_start <= 180);
        assert!(row.y_end >= 212);
    }

    #[test]
    fn plans_top_new_thread_button_in_sidebar_header() {
        let window = test_window();
        let point = top_new_thread_button_point(&window);

        assert!(point.x > window.left + 24);
        assert!(point.x < window.left + 64);
        assert!(point.y > window.top + 60);
        assert!(point.y < window.top + 96);
    }

    #[test]
    fn plans_window_activation_point_in_titlebar() {
        let window = test_window();
        let point = window_activation_point(&window);

        assert!(point.x > window.left + window.width / 3);
        assert!(point.x < window.left + (window.width * 2 / 3));
        assert!(point.y >= window.top + 12);
        assert!(point.y <= window.top + 40);
    }

    #[test]
    fn window_raise_flags_do_not_move_or_resize() {
        assert_ne!(window_raise_flags() & SWP_NOMOVE, 0);
        assert_ne!(window_raise_flags() & SWP_NOSIZE, 0);
        assert_ne!(window_raise_flags() & SWP_SHOWWINDOW, 0);
    }

    #[test]
    fn detects_ready_new_thread_page_from_center_composer_and_sidebar_highlight() {
        let mut image = solid_image(1936, 1056, [255, 255, 255]);
        fill_rect(&mut image, 16, 52, 276, 32, [243, 243, 243]);
        fill_rect(&mut image, 748, 470, 732, 138, [245, 245, 245]);

        assert!(image_new_thread_page_ready(&image));
    }

    #[test]
    fn detects_ready_new_thread_page_without_persistent_sidebar_highlight() {
        let mut image = solid_image(1936, 1056, [255, 255, 255]);
        fill_rect(&mut image, 748, 470, 732, 138, [245, 245, 245]);

        assert!(image_new_thread_page_ready(&image));
    }

    #[test]
    fn rejects_busy_existing_thread_as_new_thread_page() {
        let mut image = solid_image(1936, 1056, [255, 255, 255]);
        fill_rect(&mut image, 740, 120, 360, 16, [32, 32, 32]);
        fill_rect(&mut image, 740, 160, 600, 16, [32, 32, 32]);
        fill_rect(&mut image, 748, 908, 732, 96, [245, 245, 245]);

        assert!(!image_new_thread_page_ready(&image));
    }

    #[test]
    fn codex_window_matching_uses_process_identity_before_title_fallback() {
        let mut window = test_window();
        window.title = "Codex - C:\\data".to_string();
        let mut codex_pids = HashSet::new();
        codex_pids.insert(window.pid);

        assert!(is_codex_main_window(&window, &codex_pids));

        codex_pids.clear();
        assert!(!is_codex_main_window(&window, &codex_pids));
        window.title = "Codex".to_string();
        assert!(is_codex_main_window(&window, &codex_pids));
    }

    #[test]
    fn visible_app_launch_without_path_uses_protocol_activation() {
        assert_eq!(
            codex_visible_launch_strategy(None),
            CodexVisibleLaunch::Protocol("codex://")
        );
        assert_eq!(
            codex_visible_launch_strategy(Some("  ")),
            CodexVisibleLaunch::Protocol("codex://")
        );
    }

    #[test]
    fn visible_app_launch_with_path_uses_hidden_cli_app() {
        assert_eq!(
            codex_visible_launch_strategy(Some(r" C:\data ")),
            CodexVisibleLaunch::HiddenCliApp(r"C:\data".to_string())
        );
    }

    #[test]
    fn hidden_cli_app_command_uses_app_subcommand() {
        let command = hidden_cli_app_launch_command_with_cli(
            r"C:\Users\me\AppData\Local\OpenAI\Codex\bin\codex.exe",
            r"C:\data",
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            command.get_program().to_string_lossy(),
            r"C:\Users\me\AppData\Local\OpenAI\Codex\bin\codex.exe"
        );
        assert_eq!(args, vec!["app", r"C:\data"]);
    }

    #[test]
    fn visible_app_launch_commands_are_hidden_on_windows() {
        assert_ne!(windows_hidden_launch_creation_flags() & CREATE_NO_WINDOW, 0);
    }

    #[test]
    fn detects_red_failure_marker_left_of_selected_row() {
        let mut image = fixture();
        fill_rect(&mut image, 76, 188, 10, 24, [230, 48, 48]);
        fill_rect(&mut image, 72, 206, 18, 8, [230, 48, 48]);

        assert_eq!(
            image_thread_failure_state(&image).expect("state"),
            VisibleThreadFailureState::Failed
        );
    }

    #[test]
    fn detects_red_failure_marker_in_selected_status_slot() {
        let mut image = fixture();
        fill_rect(&mut image, 218, 190, 22, 22, [225, 58, 58]);

        assert_eq!(
            image_thread_failure_state(&image).expect("state"),
            VisibleThreadFailureState::Failed
        );
    }

    #[test]
    fn detects_blue_stopped_marker_in_selected_status_slot() {
        let mut image = fixture();
        fill_rect(&mut image, 224, 196, 12, 12, [52, 138, 255]);

        assert_eq!(
            image_thread_failure_state(&image).expect("state"),
            VisibleThreadFailureState::StoppedMarker
        );
    }

    #[test]
    fn red_failure_marker_wins_over_blue_marker() {
        let mut image = fixture();
        fill_rect(&mut image, 76, 188, 10, 24, [230, 48, 48]);
        fill_rect(&mut image, 224, 196, 12, 12, [52, 138, 255]);

        assert_eq!(
            image_thread_failure_state(&image).expect("state"),
            VisibleThreadFailureState::Failed
        );
    }

    #[test]
    fn ignores_red_pixels_outside_selected_row() {
        let mut image = fixture();
        fill_rect(&mut image, 700, 190, 18, 18, [230, 48, 48]);
        fill_rect(&mut image, 218, 260, 22, 22, [225, 58, 58]);

        assert_eq!(
            image_thread_failure_state(&image).expect("state"),
            VisibleThreadFailureState::NotFailed
        );
    }

    #[test]
    fn refuses_broad_classification_without_selected_row() {
        let image = solid_image(1400, 700, [255, 255, 255]);
        assert!(image_thread_failure_state(&image).is_err());
    }
}
