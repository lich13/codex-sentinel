# Codex Sentinel Windows Capability Probe

- Generated: 2026-05-12T15:27:57.2141348+08:00
- Repo: `C:\data\codex-sentinel`
- Branch: `windows-port-probe`
- HEAD: `57af9630b385ccaf47d7a0837515ccc9b5014d71`
- PowerShell: `5.1.19041.4648`
- OS: `Microsoft Windows NT 10.0.19045.0`
- Dry run send: `True`

## Git Baseline

```text
## windows-port-probe
 M package.json
?? reports/
?? scripts/windows-probe.ps1
origin	https://github.com/lich13/codex-sentinel.git (fetch)
origin	https://github.com/lich13/codex-sentinel.git (push)
```

## Codex Processes

| PID | Name | Exe | Command line |
| --- | --- | --- | --- |
| 1236 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" |
| 4488 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=crashpad-handler --user-data-dir=C:\Users\Administrator\AppData\Roaming\Codex /prefetch:4 --no-rate-limit --monitor-sel... |
| 13000 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=renderer --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --standard-schemes=app --secure-schemes=app,sen... |
| 13608 | codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\resources\codex.exe" app-server --analytics-default-enabled |
| 13952 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=utility --utility-sub-type=network.mojom.NetworkService --lang=zh-CN --service-sandbox-type=none --user-data-dir="C:\Us... |
| 14232 | Codex.exe | `C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe` | "C:\Program Files\WindowsApps\OpenAI.Codex_26.506.3741.0_x64__2p2nqsd0c76g0\app\Codex.exe" --type=gpu-process --user-data-dir="C:\Users\Administrator\AppData\Roaming\Codex" --gpu-preferences=SAAAAAAAAADgAAAEAAAAAAAAAA... |

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
- Latest rollout: `C:\Users\Administrator\.codex\sessions\2026\05\12\rollout-2026-05-12T14-47-36-019e1af0-e8b8-76d0-9b28-9793fc36090c.jsonl`
- URI tested: `codex://threads/019e1af0-e8b8-76d0-9b28-9793fc36090c`
- URI open error: ``
- Codex window found after open: `True`
- Foreground after open: hwnd `0x608A6`, pid `1236`, title `Codex`

## Codex Windows

| HWND | PID | Visible | Minimized | Rect | Title | Class |
| --- | --- | --- | --- | --- | --- | --- |
| `0x608A6` | 1236 | True | False | `-8,-8,1936x1056` | Codex | Chrome_WidgetWin_1 |
| `0x4090C` | 1236 | False | False | `104,104,1440x759` |  | Chrome_WidgetWin_0 |

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
| normal | PrintWindow | True | False | 1936x1056, 142956 bytes | 74 | 0 | 0 | [codex-window-printwindow.png](codex-window-printwindow.png) |
| normal | BitBlt | True | False | 1936x1056, 157462 bytes | 77 | 0 | 0 | [codex-window-bitblt.png](codex-window-bitblt.png) |
| occluded-left-sidebar | PrintWindow | True | False | 1936x1056, 142956 bytes | 74 | 0 | 0 | [codex-window-occluded-printwindow.png](codex-window-occluded-printwindow.png) |
| occluded-left-sidebar | BitBlt | True | False | 1936x1056, 137039 bytes | 59 | 0 | 0 | [codex-window-occluded-bitblt.png](codex-window-occluded-bitblt.png) |
| minimized | PrintWindow | True | True | 1936x1056, 8210 bytes | 2 | 0 | 0 | [codex-window-minimized-printwindow.png](codex-window-minimized-printwindow.png) |
| minimized | BitBlt | True | False | 1936x1056, 193916 bytes | 144 | 1 | 10 | [codex-window-minimized-bitblt.png](codex-window-minimized-bitblt.png) |

## Findings

- PASS: Codex Desktop processes are discoverable through Win32/CIM.
- PASS: At least one Codex CLI candidate exists.
- PASS: `codex://threads/<id>` was accepted by Windows shell and a Codex window was found after open.
- PASS: Codex HWND enumeration works.
- PASS: UI Automation can attach to the Codex main window and dump a tree.
- PASS: PrintWindow produced at least one nonblank Codex capture.
- PASS: BitBlt produced at least one nonblank Codex capture.
- RISK: UIA did not expose an obvious edit/value candidate.

## Next Steps

- Platformize `desktop_control` and `lifecycle` before enabling Windows MVP builds; current Rust code still contains Unix/macOS-only calls.
- Use UIA first if the dumped tree exposes stable edit/button/list semantics; otherwise constrain screenshot analysis to left sidebar thread rows.
- Keep automatic visible recovery disabled on Windows until send and failure-marker detection are verified outside dry-run mode.
