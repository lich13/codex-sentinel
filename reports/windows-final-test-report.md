# Windows Port Test Report

- Date: 2026-05-18
- Branch: `windows-port-probe`
- Upstream merged: `8b378516dea45e90d19406cbbada859981fd5ec6`
- Windows Codex Desktop: Microsoft Store install under `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0`
- Packaged launcher: `C:\data\codex-sentinel\dist\Codex Sentinel_0.1.0_windows_x64.exe`
- Packaged helper: `C:\data\codex-sentinel\dist\codex-sentinel-cli.exe`

## Verified Capabilities

- Windows process discovery finds Codex Desktop, the bundled app-server process, and PATH Codex CLI candidates.
- `codex://threads/<thread_id>` opens/focuses visible Codex threads.
- Win32 HWND enumeration finds the Codex main `Chrome_WidgetWin_1` window.
- UI Automation can attach, but still exposes only Chromium/WebView shell nodes, so semantic UIA control is not available on this build.
- `PrintWindow` captures normal Electron content; minimized/locked/session-unavailable states fail explicitly.
- Failure-state inspection is constrained to the selected left-sidebar row and returns `NotFailed` for verified normal threads.
- Stop hook install/status is Windows-aware and points at `dist\codex-sentinel-cli.exe hook-stop`.
- HKCU Run-key autostart now points directly at the GUI-subsystem launcher with `lifecycle`, avoiding PowerShell/cmd startup windows entirely.
- `install-launch-agent` now starts the Windows lifecycle helper immediately after writing the Run key; it no longer waits until the next login.
- The lifecycle loop keeps the lifecycle helper running when Codex exits; only the followed GUI, Telegram daemon, and control-worker are stopped.
- Packaged GUI startup uses the GUI launcher and helper split; no visible `ConsoleWindowClass` remains after startup or lifecycle simulation.
- Closing the Windows GUI minimizes the Tauri window to the taskbar instead of exiting or hiding; macOS keeps the existing hide behavior.
- Windows lifecycle simulation starts GUI, daemon, control-worker, and lifecycle helpers, then all are detected by `lifecycle-status`.
- Windows visible new-thread control now raises Codex with `SetWindowPos`, opens New Chat, and requires a conservative screenshot-based new-page check before returning success.
- Disposable new-thread send created thread `019e2aa3-ec29-7613-9b3f-51637611f0e3` with cwd `C:\data` and matching first user prompt.
- Existing-thread append to the same disposable thread succeeded through the visible Codex window and was confirmed in the rollout tail and app-server probe.
- `recoverable` returned `[]` while retry/error log noise existed, so logs remain only a prefilter and do not trigger recovery without visible-state confirmation.
- Windows package script leaves the main Tauri helper to `tauri build` so it keeps the bundled `ui-dist` production context, then builds only the GUI launcher with plain cargo. This avoids the packaged panel trying to load `http://127.0.0.1:5173`.
- Windows package script stops currently running packaged Sentinel processes before replacing `dist` exes, so rebuilding no longer fails when lifecycle/GUI are active.
- Packaged panel smoke test loads the real Sentinel dashboard from bundled assets; evidence screenshot: `reports\sentinel-panel-after-fix.png`.
- Healthy Windows visible-control diagnostics no longer render as a warning panel; `desktop-control-status` returns `notes=[]` when the Codex window is visible and send-ready. Evidence screenshot: `reports\sentinel-panel-notes-fixed.png`.
- Upstream `origin/main` changes through `8b37851` are merged: subagent/internal Codex threads are hidden from user-facing status and hook events, Telegram feedback pagination is present, recovery confirmation modes are split, and the tray includes the clear-archived action.
- Windows test home handling was aligned with upstream `CODEX_SENTINEL_TEST_HOME` support so subagent-thread filtering tests use the isolated test database on Windows.

## Commands Passed

```text
git fetch origin --prune
git merge --ff-only origin/main
cargo test desktop_control::windows::tests::
cargo fmt -- --check
cargo test
cargo test hooks::tests::recent_hook_events_hide_subagent_thread_events
cargo test codex_absent_stop_plan_keeps_lifecycle_helper_running
npm run build:ui
npm run package:windows
.\dist\codex-sentinel-cli.exe install-hooks
.\dist\codex-sentinel-cli.exe install-launch-agent
.\dist\codex-sentinel-cli.exe hook-status
.\dist\codex-sentinel-cli.exe lifecycle-status
.\dist\codex-sentinel-cli.exe desktop-control-status
.\dist\codex-sentinel-cli.exe status
.\dist\codex-sentinel-cli.exe debug-new-chat C:\data
.\dist\codex-sentinel-cli.exe new "Codex Sentinel disposable Windows new-thread verification. Reply OK only." C:\data
.\dist\codex-sentinel-cli.exe append 019e2aa3-ec29-7613-9b3f-51637611f0e3 "Codex Sentinel disposable Windows existing-thread append verification. Reply OK only."
.\dist\codex-sentinel-cli.exe debug-app-server-thread 019e2aa3-ec29-7613-9b3f-51637611f0e3
.\dist\codex-sentinel-cli.exe debug-thread-failure-state 019e2aa3-ec29-7613-9b3f-51637611f0e3
.\dist\codex-sentinel-cli.exe recoverable
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-probe.ps1 -SkipUriOpen -SkipMinimizeTest -SkipOcclusionTest
Start packaged GUI, enumerate windows, verify no ConsoleWindowClass
Post WM_CLOSE to packaged GUI, verify Tauri window remains minimized
Execute installed Run key command, verify lifecycle/gui/daemon/control-worker start hidden with no visible console window
```

## Latest Evidence

```text
hook-status: installed_app_command=true
installed command: "C:\data\codex-sentinel\dist\codex-sentinel-cli.exe" hook-stop
Run key: "C:\data\codex-sentinel\dist\Codex Sentinel_0.1.0_windows_x64.exe" lifecycle
lifecycle-status after latest package/install: codex_running=true, sentinel_gui_running=true, daemon_running=true, control_worker_running=true, lifecycle_running=true
lifecycle pids after latest package/install: gui=3744, daemon=5712, control_worker=14060, lifecycle=5608
desktop-control-status: mode=visible_desktop_windows, Codex HWND found, visible send ready
latest cargo test: 125 main tests + 2 GUI launcher tests passed
latest UI/package: npm run build:ui passed; npm run package:windows passed
latest status: recovery.kind=None, recoverable=[]
new-thread DB match: 019e2aa3-ec29-7613-9b3f-51637611f0e3, cwd=\\?\C:\data
app-server after new: latest_turn_status=completed, latest_turn_error=null
append result: ok=true, message=已在 Codex APP 内发送追加指令。
app-server after append: latest_turn_status=completed, latest_turn_error=null
debug-thread-failure-state: NotFailed
recoverable: []
GUI black-window check: VisibleConsoleWindows=0
GUI close check: SentinelStillRunning=true, SentinelMinimized=true
Run key simulation: lifecycle/gui/daemon/control-worker all true; visible ConsoleWindows=0
latest dry probe: reports\windows-probe-20260515-155137\windows-probe-report.md
```

## Remaining Risks

- Real `Failed` and `StoppedMarker` sidebar samples are still needed to tune thresholds beyond synthetic tests and current `NotFailed` samples.
- UIA remains semantically sparse; Windows visible control uses Win32, screenshots, clipboard, keyboard, and brief focus/mouse interaction.
- New Chat readiness is tuned against the current light-theme Codex Desktop layout. Dark theme or a large UI redesign should be re-probed before claiming stable recovery.
- Locked desktop, minimized Codex, or no interactive user session cannot be operated safely and should pause with explicit errors.
- Telegram network delivery was not verified because this machine does not provide a configured bot token here; daemon lifecycle/process startup is verified.
- Full Codex-exit behavior was unit-tested at the lifecycle stop-plan layer, but not live-tested by killing Codex because this Codex Desktop session is hosting the current agent thread.
