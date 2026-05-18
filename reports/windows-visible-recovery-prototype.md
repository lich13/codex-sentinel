# Windows Visible Recovery Prototype

- Date: 2026-05-15
- Branch: `windows-port-probe`
- Upstream merged: `254ac8873351e194fddd49119467594e3fa64e34`

## Implementation

- `desktop_control` is platformized into `macos`, `windows`, and `unsupported` implementations behind one public interface.
- Windows visible control uses Win32 for process/window detection, `codex://` protocol activation for existing threads, `PrintWindow` screenshots for state inspection, and clipboard/keyboard input for prompt submission.
- Windows does not expose macOS Accessibility permission UX; `open_permission_settings()` is a no-op on Windows.
- Existing-thread preparation opens `codex://threads/<id>`, restores/raises Codex, and verifies a visible non-minimized window before input.
- Failure-state inspection classifies only marker regions in the selected left-sidebar row:
  - red marker => `Failed`
  - blue marker => `StoppedMarker`
  - no marker => `NotFailed`
  - no selected row / blank capture => hard error, no broad red-pixel fallback
- New-thread preparation opens Codex for the requested path, raises Codex with `SetWindowPos`, clicks the New Chat entry, then requires a screenshot-based new-page check before returning success.
- The new-page check intentionally rejects busy existing-thread pages by requiring a quiet upper main area, a centered composer panel, and no bottom existing-thread composer.
- Visible prompt confirmation uses the stricter upstream behavior: a fresh thread/app-server signal must be paired with a latest user prompt match in state/rollout data.
- Windows lifecycle uses HKCU Run key instead of a Windows Service because UI automation needs the interactive user session.
- Hook install/status resolves the current Windows helper exe and quotes paths with spaces correctly.
- App-server and Codex CLI probes use `CREATE_NO_WINDOW`, so Codex CLI startup does not leave a black console window.
- The packaged Windows GUI is split into a GUI-subsystem launcher plus `codex-sentinel-cli.exe`; CLI commands keep terminal output, GUI and lifecycle startup stay windowless except for the Tauri panel.
- Windows Run key now points directly at `"Codex Sentinel_0.1.0_windows_x64.exe" lifecycle`; the launcher starts `codex-sentinel-cli.exe lifecycle` with `CREATE_NO_WINDOW`, so startup no longer depends on a hidden PowerShell wrapper.
- The Windows packaging script must not rebuild the main Tauri helper with plain cargo after `tauri build`; doing so makes the WebView load the Vite dev URL (`127.0.0.1:5173`) instead of bundled `ui-dist`.

## Verified

```text
cargo test desktop_control::windows::tests::
cargo test
npm run build:ui
npm run package:windows
.\dist\codex-sentinel-cli.exe install-launch-agent
reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v local.codex-sentinel
.\dist\codex-sentinel-cli.exe debug-new-chat C:\data
.\dist\codex-sentinel-cli.exe new "Codex Sentinel disposable Windows new-thread verification. Reply OK only." C:\data
.\dist\codex-sentinel-cli.exe append 019e2aa3-ec29-7613-9b3f-51637611f0e3 "Codex Sentinel disposable Windows existing-thread append verification. Reply OK only."
.\dist\codex-sentinel-cli.exe debug-app-server-thread 019e2aa3-ec29-7613-9b3f-51637611f0e3
.\dist\codex-sentinel-cli.exe debug-thread-failure-state 019e2aa3-ec29-7613-9b3f-51637611f0e3
.\dist\codex-sentinel-cli.exe recoverable
```

Observed:

```text
debug-new-chat: returns success only after new-page screenshot detection passes
new thread: 019e2aa3-ec29-7613-9b3f-51637611f0e3
new thread cwd: \\?\C:\data
new thread first_user_message: Codex Sentinel disposable Windows new-thread verification. Reply OK only.
new app-server latest_turn_status: completed
append result: ok=true
append rollout tail: latest user prompt matches the append prompt
append app-server latest_turn_status: completed
failure state: NotFailed
recoverable: []
Run key: "C:\data\codex-sentinel\dist\Codex Sentinel_0.1.0_windows_x64.exe" lifecycle
VisibleConsoleWindows: 0
```

## Safety Behavior

- If Codex cannot be raised to foreground, visible input returns an error.
- If New Chat is clicked but the screenshot does not look like the new-thread composer, `debug-new-chat` and `new` fail before sending.
- If the new-thread prompt is pasted but the send button cannot be located in the expected new-thread composer region, sending is refused.
- If Codex is minimized, blank, locked, or unavailable to `PrintWindow`, state inspection fails explicitly.

## Remaining Work

- Collect real Windows `Failed` and `StoppedMarker` screenshots from Codex Desktop and add fixture tests.
- Re-probe if Codex Desktop changes theme/layout, because current UIA semantics are not rich enough to replace screenshot checks.
- Verify Telegram delivery with a real configured bot token; process lifecycle already starts.
