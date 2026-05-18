# Codex Sentinel Windows Capability Probe

- Generated: 2026-05-15T15:27:35.9064098+08:00
- Repo: `C:\data\codex-sentinel`
- Branch: `windows-port-probe`
- HEAD: `254ac8873351e194fddd49119467594e3fa64e34`
- PowerShell: `5.1.19041.4648`
- OS: `Microsoft Windows NT 10.0.19045.0`
- Dry run send: `True`

## Git Baseline

```text
## windows-port-probe
M  .github/workflows/release-build.yml
M  Cargo.lock
MM Cargo.toml
M  README.md
M  package.json
A  reports/append-stderr.txt
A  reports/append-stdout.txt
A  reports/hover-codex-data-sidebar.png
A  reports/hover-codex-data.png
A  reports/hover-data.png
A  reports/live-hover-data-after-click.png
A  reports/live-hover-data-clicked.png
A  reports/live-hover-data.png
A  reports/live-hover-project-row.png
A  reports/live-project-row-action-clicked.png
A  reports/rollout-019e1f5a-image-review.md
A  reports/rollout-019e1f5a-images/image_1.png
A  reports/rollout-019e1f5a-images/image_2.png
A  reports/windows-final-test-report.md
A  reports/windows-probe-20260512-152750/codex-uia-tree.txt
A  reports/windows-probe-20260512-152750/codex-window-bitblt.png
A  reports/windows-probe-20260512-152750/codex-window-minimized-bitblt.png
A  reports/windows-probe-20260512-152750/codex-window-minimized-printwindow.png
A  reports/windows-probe-20260512-152750/codex-window-occluded-bitblt.png
A  reports/windows-probe-20260512-152750/codex-window-occluded-printwindow.png
A  reports/windows-probe-20260512-152750/codex-window-printwindow.png
A  reports/windows-probe-20260512-152750/windows-probe-report.md
A  reports/windows-probe-20260513-100940/codex-uia-tree.txt
A  reports/windows-probe-20260513-100940/codex-window-bitblt.png
A  reports/windows-probe-20260513-100940/codex-window-minimized-bitblt.png
A  reports/windows-probe-20260513-100940/codex-window-minimized-printwindow.png
A  reports/windows-probe-20260513-100940/codex-window-occluded-bitblt.png
A  reports/windows-probe-20260513-100940/codex-window-occluded-printwindow.png
A  reports/windows-probe-20260513-100940/codex-window-printwindow.png
A  reports/windows-probe-20260513-100940/read-latest-thread.py
A  reports/windows-probe-20260513-100940/windows-probe-report.md
A  reports/windows-probe-20260513-122011/codex-project-row-hover.png
A  reports/windows-probe-20260513-122011/codex-uia-tree.txt
A  reports/windows-probe-20260513-122011/codex-window-bitblt.png
A  reports/windows-probe-20260513-122011/codex-window-printwindow.png
A  reports/windows-probe-20260513-122011/read-latest-thread.py
A  reports/windows-probe-20260513-122040/codex-project-row-hover.png
A  reports/windows-probe-20260513-122040/codex-uia-tree.txt
A  reports/windows-probe-20260513-122040/codex-window-bitblt.png
A  reports/windows-probe-20260513-122040/codex-window-printwindow.png
A  reports/windows-probe-20260513-122040/read-latest-thread.py
A  reports/windows-probe-20260513-122057/codex-project-row-hover.png
A  reports/windows-probe-20260513-122057/codex-uia-tree.txt
A  reports/windows-probe-20260513-122057/codex-window-bitblt.png
A  reports/windows-probe-20260513-122057/codex-window-printwindow.png
A  reports/windows-probe-20260513-122057/read-latest-thread.py
A  reports/windows-probe-20260513-122112/codex-project-row-hover.png
A  reports/windows-probe-20260513-122112/codex-uia-tree.txt
A  reports/windows-probe-20260513-122112/codex-window-bitblt.png
A  reports/windows-probe-20260513-122112/codex-window-printwindow.png
A  reports/windows-probe-20260513-122112/read-latest-thread.py
A  reports/windows-probe-20260513-122224/codex-project-row-hover.png
A  reports/windows-probe-20260513-122224/codex-uia-tree.txt
A  reports/windows-probe-20260513-122224/codex-window-bitblt.png
A  reports/windows-probe-20260513-122224/codex-window-printwindow.png
A  reports/windows-probe-20260513-122224/read-latest-thread.py
A  reports/windows-probe-20260513-122224/windows-probe-report.md
A  reports/windows-probe-20260513-122638/codex-project-row-hover.png
A  reports/windows-probe-20260513-122638/codex-uia-tree.txt
A  reports/windows-probe-20260513-122638/codex-window-bitblt.png
A  reports/windows-probe-20260513-122638/codex-window-printwindow.png
A  reports/windows-probe-20260513-122638/read-latest-thread.py
A  reports/windows-probe-20260513-122638/windows-probe-report.md
A  reports/windows-probe-20260513-141950/codex-project-row-hover.png
A  reports/windows-probe-20260513-141950/codex-uia-tree.txt
A  reports/windows-probe-20260513-141950/codex-window-bitblt.png
A  reports/windows-probe-20260513-141950/codex-window-printwindow.png
A  reports/windows-probe-20260513-141950/read-latest-thread.py
A  reports/windows-probe-20260513-141950/windows-probe-report.md
A  reports/windows-visible-recovery-prototype.md
A  scripts/package-windows.ps1
A  scripts/windows-probe.ps1
M  src/app_server_probe.rs
MM src/codex.rs
M  src/control_queue.rs
R  src/desktop_control.rs -> src/desktop_control/macos.rs
A  src/desktop_control/mod.rs
A  src/desktop_control/unsupported.rs
AM src/desktop_control/windows.rs
M  src/hooks.rs
MM src/lifecycle.rs
AM src/lifecycle/macos.rs
AM src/lifecycle/unsupported.rs
AM src/lifecycle/windows.rs
M  src/main.rs
M  src/telegram.rs
M  src/ui.rs
M  tauri.conf.json
M  ui/src/main.ts
?? reports/windows-probe-20260514-172139/
?? src/bin/
origin	https://github.com/lich13/codex-sentinel.git (fetch)
origin	https://github.com/lich13/codex-sentinel.git (push)
```

## Codex Processes

| PID | Name | Exe | Command line |
| --- | --- | --- | --- |
| 2508 | codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe" app-server --analytics-default-enabled |
| 2528 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" |
| 7616 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=renderer --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --standard-schemes=app --secure-schemes=app,sen... |
| 8540 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=renderer --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --standard-schemes=app --secure-schemes=app,sen... |
| 9316 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=utility --utility-sub-type=network.mojom.NetworkService --lang=zh-CN --service-sandbox-type=none --user-data-dir="C:\Us... |
| 9680 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=gpu-process --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --gpu-preferences=SAAAAAAAAADgAAAEAAAAAAAAAA... |
| 10388 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=renderer --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --standard-schemes=app --secure-schemes=app,sen... |
| 11848 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=crashpad-handler --user-data-dir=C:\Users\Administrator\AppData\Roaming\Codex /prefetch:4 --no-rate-limit --monitor-sel... |

## Codex CLI Candidates

| Source | Exists | Path |
| --- | --- | --- |
| desktop-bundled | True | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` |
| running-process | True | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` |
| PATH:codex.exe | True | `C:\Users\Administrator\AppData\Local\OpenAI\Codex\bin\codex.exe` |
| PATH:codex.cmd | True | `C:\Users\Administrator\AppData\Roaming\npm\codex.cmd` |
| PATH:codex | True | `C:\Users\Administrator\AppData\Roaming\npm\codex.ps1` |
| PATH:codex.ps1 | True | `C:\Users\Administrator\AppData\Roaming\npm\codex.ps1` |

## Recent Thread / URI

- Latest thread id: `019e1af0-e8b8-76d0-9b28-9793fc36090c`
- Latest thread title: 移植 Codex Sentinel 到 Windows
- Latest thread cwd: `\\?\C:\data`
- Latest rollout: `\\?\C:\Users\Administrator\.codex\sessions\2026\05\12\rollout-2026-05-12T14-47-36-019e1af0-e8b8-76d0-9b28-9793fc36090c.jsonl`
- URI open was skipped or no thread id was available.

## Codex Windows

| HWND | PID | Visible | Minimized | Rect | Title | Class |
| --- | --- | --- | --- | --- | --- | --- |
| `0x50446` | 2528 | True | False | `-8,-8,1936x1056` | Codex | Chrome_WidgetWin_1 |
| `0xE0826` | 2528 | False | False | `26,26,1440x759` |  | Chrome_WidgetWin_0 |
| `0x2E08CC` | 2528 | False | False | `725,52,470x640` | Codex | Chrome_WidgetWin_1 |
| `0x1D0918` | 2528 | False | False | `725,402,470x290` | Codex | Chrome_WidgetWin_1 |

## UI Automation

- UIA node count dumped: `17`
- UIA walker: `RawViewWalker`
- Tree artifact: [codex-uia-tree.txt](codex-uia-tree.txt)
- Edit candidates: `0`
- Button/invoke candidates: `4`
- Send attempt requested: `False`
- Send attempt result: `none` / ok `False` / Not requested; dry run only.
- Focus/mouse impact: UIA dry run does not move the mouse; URI open and minimize/restore tests can change focus.

### UIA Search Hits
- No keyword hits were found in the UIA tree.

### UIA Tree Snippet
```text
- ControlType.Window name='Codex' aid='' class='Chrome_WidgetWin_1' rect=(-8,-8,1936,1056) patterns=[]
  - ControlType.Pane name='' aid='' class='Intermediate D3D Window' rect=(0,0,1920,1040) patterns=[]
  - ControlType.Pane name='Codex' aid='' class='RootView' rect=(0,0,1920,1040) patterns=[]
    - ControlType.Pane name='' aid='' class='NonClientView' rect=(0,0,1920,1040) patterns=[]
      - ControlType.Pane name='' aid='' class='WinFrameView' rect=(0,0,1920,1040) patterns=[]
        - ControlType.Pane name='' aid='' class='WinCaptionButtonContainer' rect=(1783,0,137,36) patterns=[]
          - ControlType.Button name='最小化' aid='' class='WinCaptionButton' rect=(1783,0,45,36) patterns=[Invoke]
          - ControlType.Button name='最大化' aid='' class='WinCaptionButton' rect=(1828,0,46,36) patterns=[Invoke]
          - ControlType.Button name='恢复' aid='' class='WinCaptionButton' rect=(1828,0,46,36) patterns=[Invoke]
          - ControlType.Button name='关闭' aid='' class='WinCaptionButton' rect=(1874,0,46,36) patterns=[Invoke]
        - ControlType.Pane name='' aid='' class='ClientView' rect=(0,0,1920,1040) patterns=[]
          - ControlType.Pane name='' aid='' class='View' rect=(0,0,1920,1040) patterns=[]
            - ControlType.Pane name='' aid='' class='View' rect=(0,0,1920,1040) patterns=[]
              - ControlType.Pane name='' aid='' class='View' rect=(0,0,1920,1040) patterns=[]
                - ControlType.Document name='' aid='' class='WebView' rect=(inf,inf,inf,inf) patterns=[]
                - ControlType.Pane name='Chrome Legacy Window' aid='26' class='Chrome_RenderWidgetHostHWND' rect=(0,0,1920,1040) patterns=[]
              - ControlType.Pane name='' aid='' class='View' rect=(0,0,1920,1040) patterns=[]
```

## Screenshot / Pixel Probe

| Label | Method | API ok | Mostly blank | Size | Unique sample colors | Left red samples | Left blue samples | Artifact |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| normal | PrintWindow | True | False | 1936x1056, 130918 bytes | 75 | 0 | 0 | [codex-window-printwindow.png](codex-window-printwindow.png) |
| normal | BitBlt | True | False | 1936x1056, 158513 bytes | 131 | 0 | 1 | [codex-window-bitblt.png](codex-window-bitblt.png) |

## Project Row Hover Probe

- Not requested. Pass -HoverProjectRowProbe to capture the sidebar project hover state; pass -ClickProjectRowActionProbe only when opening a new visible chat is acceptable.

## Findings

- PASS: Codex Desktop processes are discoverable through Win32/CIM.
- PASS: At least one Codex CLI candidate exists.
- UNKNOWN: URI open was not tested.
- PASS: Codex HWND enumeration works.
- PASS: UI Automation can attach to the Codex main window and dump a tree.
- PASS: PrintWindow produced at least one nonblank Codex capture.
- PASS: BitBlt produced at least one nonblank Codex capture.
- RISK: UIA did not expose an obvious edit/value candidate.

## Next Steps

- Keep Windows visible control behind the Rust platform layer (src/desktop_control/windows.rs) and avoid moving Win32/UIA assumptions into macOS code.
- Use UIA first if a future Codex build exposes stable edit/button/list semantics; current verified fallback is Win32 HWND + constrained left-sidebar screenshot analysis.
- For real send probes, use disposable threads or explicit user-approved targets only; visible send uses focus, clipboard, and keyboard input.
- Collect real Failed/StoppedMarker sidebar samples and tune the selected-row marker thresholds against those fixtures.
