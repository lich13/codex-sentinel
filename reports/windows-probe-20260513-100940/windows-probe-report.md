# Codex Sentinel Windows Capability Probe

- Generated: 2026-05-13T10:09:48.3787711+08:00
- Repo: `C:\data\codex-sentinel`
- Branch: `windows-port-probe`
- HEAD: `99ab8dae2022bc91590560c0c219cfa40a673e71`
- PowerShell: `5.1.19041.4648`
- OS: `Microsoft Windows NT 10.0.19045.0`
- Dry run send: `True`

## Git Baseline

```text
## windows-port-probe
 M .github/workflows/release-build.yml
 M Cargo.lock
 M Cargo.toml
 M package.json
 M src/app_server_probe.rs
 M src/codex.rs
 M src/control_queue.rs
 D src/desktop_control.rs
 M src/hooks.rs
 M src/lifecycle.rs
?? reports/
?? scripts/windows-probe.ps1
?? src/desktop_control/
?? src/lifecycle/
origin	https://github.com/lich13/codex-sentinel.git (fetch)
origin	https://github.com/lich13/codex-sentinel.git (push)
```

## Codex Processes

| PID | Name | Exe | Command line |
| --- | --- | --- | --- |
| 2824 | codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe" app-server --analytics-default-enabled |
| 5808 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" |
| 7000 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=utility --utility-sub-type=network.mojom.NetworkService --lang=zh-CN --service-sandbox-type=none --user-data-dir="C:\Us... |
| 9196 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=crashpad-handler --user-data-dir=C:\Users\Administrator\AppData\Roaming\Codex /prefetch:4 --no-rate-limit --monitor-sel... |
| 9504 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=gpu-process --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --gpu-preferences=SAAAAAAAAADgAAAEAAAAAAAAAA... |
| 11668 | node_repl.exe | `C:\Users\Administrator\AppData\Local\OpenAI\Codex\bin\node_repl.exe` | "C:\Users\Administrator\AppData\Local\OpenAI\Codex\bin\node_repl.exe" |
| 11924 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=renderer --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --standard-schemes=app --secure-schemes=app,sen... |
| 13076 | node_repl.exe | `C:\Users\Administrator\AppData\Local\OpenAI\Codex\bin\node_repl.exe` | "C:\Users\Administrator\AppData\Local\OpenAI\Codex\bin\node_repl.exe" |

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
- URI tested: `codex://threads/019e1af0-e8b8-76d0-9b28-9793fc36090c`
- URI open error: ``
- Codex window found after open: `True`
- Foreground after open: hwnd `0x5022C`, pid `5808`, title `Codex`

## Codex Windows

| HWND | PID | Visible | Minimized | Rect | Title | Class |
| --- | --- | --- | --- | --- | --- | --- |
| `0x5022C` | 5808 | True | False | `-8,-8,1936x1056` | Codex | Chrome_WidgetWin_1 |
| `0x20128` | 5808 | False | False | `26,26,1440x759` |  | Chrome_WidgetWin_0 |

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
| normal | PrintWindow | True | False | 1936x1056, 114052 bytes | 72 | 0 | 0 | [codex-window-printwindow.png](codex-window-printwindow.png) |
| normal | BitBlt | True | False | 1936x1056, 141421 bytes | 128 | 0 | 1 | [codex-window-bitblt.png](codex-window-bitblt.png) |
| occluded-left-sidebar | PrintWindow | True | False | 1936x1056, 114050 bytes | 71 | 0 | 0 | [codex-window-occluded-printwindow.png](codex-window-occluded-printwindow.png) |
| occluded-left-sidebar | BitBlt | True | False | 1936x1056, 107462 bytes | 51 | 0 | 0 | [codex-window-occluded-bitblt.png](codex-window-occluded-bitblt.png) |
| minimized | PrintWindow | True | True | 1936x1056, 8210 bytes | 2 | 0 | 0 | [codex-window-minimized-printwindow.png](codex-window-minimized-printwindow.png) |
| minimized | BitBlt | True | False | 1936x1056, 893999 bytes | 891 | 2 | 32 | [codex-window-minimized-bitblt.png](codex-window-minimized-bitblt.png) |

## Findings

- PASS: Codex Desktop processes are discoverable through Win32/CIM.
- PASS: At least one Codex CLI candidate exists.
- PASS: `codex://threads/<id>` was accepted by Windows shell and a Codex window was found after open.
- PASS: Codex HWND enumeration works.
- PASS: UI Automation can attach to the Codex main window and dump a tree.
- PASS: PrintWindow produced at least one nonblank Codex capture.
- PASS: BitBlt produced at least one nonblank Codex capture.
- RISK: UIA did not expose an obvious edit/value candidate.

## Release Verification

- `target\release\codex-sentinel.exe hook-status` reported `installed_app_command=true` and the installed Stop hook pointed at the release exe path.
- `target\release\codex-sentinel.exe desktop-control-status` reported the Codex HWND and the current visible-send gate state.
- `cargo run -- debug-visible-send-plan` stayed dry-run only and reported the current composer/send-point estimates without clicking or typing.

## Next Steps

- Platformize `desktop_control` and `lifecycle` before enabling Windows MVP builds; current Rust code still contains Unix/macOS-only calls.
- Use UIA first if the dumped tree exposes stable edit/button/list semantics; otherwise constrain screenshot analysis to left sidebar thread rows.
- Keep automatic visible recovery disabled on Windows until send and failure-marker detection are verified outside dry-run mode.
