# Windows capability probe for Codex Sentinel.
#
# The default run is intentionally non-destructive: it opens a codex:// thread
# URI when a thread id is available, enumerates windows/UIA/screenshot behavior,
# and reports input/send capabilities without submitting text. Pass
# -SendProbePrompt only when you explicitly want to send the probe prompt into
# the currently visible Codex window.

[CmdletBinding()]
param(
    [string]$ThreadId,
    [string]$OutputDir = (Join-Path (Get-Location).Path "reports"),
    [switch]$SkipUriOpen,
    [switch]$SkipMinimizeTest,
    [switch]$SkipOcclusionTest,
    [switch]$SendProbePrompt,
    [switch]$HoverProjectRowProbe,
    [switch]$ClickProjectRowActionProbe,
    [string]$ProbePrompt = "Codex Sentinel Windows probe. Please reply with OK."
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
$env:PYTHONIOENCODING = "utf-8"

$script:Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$script:RunDir = Join-Path $OutputDir ("windows-probe-" + $script:Timestamp)
New-Item -ItemType Directory -Force -Path $script:RunDir | Out-Null

function Shorten {
    param([AllowNull()][string]$Value, [int]$Max = 180)
    if ([string]::IsNullOrEmpty($Value)) {
        return ""
    }
    $single = ($Value -replace "\s+", " ").Trim()
    if ($single.Length -le $Max) {
        return $single
    }
    return $single.Substring(0, [Math]::Max(0, $Max - 3)) + "..."
}

function Escape-Md {
    param([AllowNull()][string]$Value)
    if ($null -eq $Value) {
        return ""
    }
    return ($Value -replace "\|", "\|") -replace "`r?`n", " "
}

function Md-Code {
    param([AllowNull()][object]$Value)
    if ($null -eq $Value) {
        return "``"
    }
    return '`' + (Escape-Md ([string]$Value)) + '`'
}

function Add-NativeTypes {
    if ("WinProbe.Native" -as [type]) {
        return
    }
    Add-Type -ReferencedAssemblies @("System.Drawing") -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace WinProbe {
  public static class Native {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
      public int Left;
      public int Top;
      public int Right;
      public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
      public int X;
      public int Y;
    }

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll", SetLastError=true)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll", SetLastError=true)]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT lpPoint);

    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);

    public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    public const uint MOUSEEVENTF_LEFTUP = 0x0004;

    [DllImport("user32.dll")]
    public static extern IntPtr GetDC(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdcBlt, uint nFlags);

    [DllImport("gdi32.dll", SetLastError=true)]
    public static extern bool BitBlt(
      IntPtr hdcDest, int nXDest, int nYDest, int nWidth, int nHeight,
      IntPtr hdcSrc, int nXSrc, int nYSrc, int dwRop);
  }
}
"@
}

function Get-WindowTextValue {
    param([IntPtr]$Hwnd)
    $builder = New-Object System.Text.StringBuilder 1024
    [void][WinProbe.Native]::GetWindowText($Hwnd, $builder, $builder.Capacity)
    return $builder.ToString()
}

function Get-WindowClassValue {
    param([IntPtr]$Hwnd)
    $builder = New-Object System.Text.StringBuilder 512
    [void][WinProbe.Native]::GetClassName($Hwnd, $builder, $builder.Capacity)
    return $builder.ToString()
}

function Get-WindowRectObject {
    param([IntPtr]$Hwnd)
    $rect = New-Object WinProbe.Native+RECT
    if (-not [WinProbe.Native]::GetWindowRect($Hwnd, [ref]$rect)) {
        return $null
    }
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    return [pscustomobject]@{
        Left = $rect.Left
        Top = $rect.Top
        Right = $rect.Right
        Bottom = $rect.Bottom
        Width = $width
        Height = $height
    }
}

function Get-CodexProcesses {
    $all = Get-CimInstance Win32_Process
    $all | Where-Object {
        $_.Name -ieq "Codex.exe" -or
        $_.Name -ieq "codex.exe" -or
        ($_.ExecutablePath -and $_.ExecutablePath -match "\\OpenAI\.Codex_|\\Codex\\|\\codex\.exe$") -or
        ($_.CommandLine -and $_.CommandLine -match "OpenAI\.Codex_|\\Codex\\|codex(\.exe|\.ps1)?")
    } | Sort-Object ProcessId
}

function Get-CliCandidates {
    param($Processes)
    $candidates = New-Object System.Collections.Generic.List[object]
    foreach ($proc in $Processes) {
        if ($proc.Name -ceq "codex.exe" -and $proc.ExecutablePath) {
            [void]$candidates.Add([pscustomobject]@{
                Source = "running-process"
                Path = $proc.ExecutablePath
                Exists = (Test-Path -LiteralPath $proc.ExecutablePath)
            })
        }
    }
    foreach ($appProc in ($Processes | Where-Object { $_.Name -ceq "Codex.exe" -and $_.ExecutablePath })) {
        $resourcesCli = Join-Path (Split-Path -Parent $appProc.ExecutablePath) "resources\codex.exe"
        [void]$candidates.Add([pscustomobject]@{
            Source = "desktop-bundled"
            Path = $resourcesCli
            Exists = (Test-Path -LiteralPath $resourcesCli)
        })
    }
    foreach ($name in @("codex.exe", "codex.cmd", "codex.ps1", "codex")) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($cmd) {
            [void]$candidates.Add([pscustomobject]@{
                Source = "PATH:$name"
                Path = $cmd.Source
                Exists = (Test-Path -LiteralPath $cmd.Source)
            })
        }
    }
    $candidates | Sort-Object Path, Source -Unique
}

function Get-GitText {
    param([string[]]$GitArgs)
    try {
        $value = (& git @GitArgs 2>$null) -join "`n"
        return $value.Trim()
    } catch {
        return ""
    }
}

function Find-Python {
    foreach ($cmd in @("python.exe", "python", "py.exe", "py")) {
        $resolved = Get-Command $cmd -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($resolved) {
            return $resolved.Source
        }
    }
    return $null
}

function Get-LatestCodexThread {
    $db = Join-Path $env:USERPROFILE ".codex\state_5.sqlite"
    if (-not (Test-Path -LiteralPath $db)) {
        return $null
    }
    $python = Find-Python
    if (-not $python) {
        return $null
    }
    $code = @'
import json, sqlite3, sys
db = sys.argv[1]
conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
conn.row_factory = sqlite3.Row
row = conn.execute("SELECT id, title, cwd, updated_at, rollout_path FROM threads WHERE coalesce(archived, 0) = 0 ORDER BY updated_at DESC LIMIT 1").fetchone()
print(json.dumps(dict(row) if row else None, ensure_ascii=False))
'@
    try {
        $helperPath = Join-Path $script:RunDir "read-latest-thread.py"
        Set-Content -LiteralPath $helperPath -Value $code -Encoding UTF8
        $pythonLeaf = Split-Path -Leaf $python
        if ($pythonLeaf -ieq "py.exe" -or $pythonLeaf -ieq "py") {
            $raw = (& $python -3 $helperPath $db 2>$null) -join "`n"
        } else {
            $raw = (& $python $helperPath $db 2>$null) -join "`n"
        }
        if ([string]::IsNullOrWhiteSpace($raw)) {
            return $null
        }
        return $raw | ConvertFrom-Json
    } catch {
        return $null
    }
}

function Get-AllWindows {
    Add-NativeTypes
    $windows = New-Object System.Collections.Generic.List[object]
    $callback = [WinProbe.Native+EnumWindowsProc]{
        param([IntPtr]$Hwnd, [IntPtr]$Param)
        $pidRef = [uint32]0
        [void][WinProbe.Native]::GetWindowThreadProcessId($Hwnd, [ref]$pidRef)
        $rect = Get-WindowRectObject -Hwnd $Hwnd
        if ($null -eq $rect) {
            return $true
        }
        $title = Get-WindowTextValue -Hwnd $Hwnd
        $class = Get-WindowClassValue -Hwnd $Hwnd
        [void]$windows.Add([pscustomobject]@{
            Hwnd = ("0x{0:X}" -f $Hwnd.ToInt64())
            HwndInt64 = $Hwnd.ToInt64()
            Pid = [int]$pidRef
            Title = $title
            ClassName = $class
            Visible = [WinProbe.Native]::IsWindowVisible($Hwnd)
            Minimized = [WinProbe.Native]::IsIconic($Hwnd)
            Left = $rect.Left
            Top = $rect.Top
            Width = $rect.Width
            Height = $rect.Height
            Right = $rect.Right
            Bottom = $rect.Bottom
        })
        return $true
    }
    [void][WinProbe.Native]::EnumWindows($callback, [IntPtr]::Zero)
    return $windows
}

function Get-ForegroundWindowInfo {
    Add-NativeTypes
    $hwnd = [WinProbe.Native]::GetForegroundWindow()
    if ($hwnd -eq [IntPtr]::Zero) {
        return $null
    }
    $pidRef = [uint32]0
    [void][WinProbe.Native]::GetWindowThreadProcessId($hwnd, [ref]$pidRef)
    return [pscustomobject]@{
        Hwnd = ("0x{0:X}" -f $hwnd.ToInt64())
        HwndInt64 = $hwnd.ToInt64()
        Pid = [int]$pidRef
        Title = Get-WindowTextValue -Hwnd $hwnd
        ClassName = Get-WindowClassValue -Hwnd $hwnd
    }
}

function Get-CodexWindows {
    param($Processes)
    $pidSet = @{}
    foreach ($proc in $Processes) {
        $pidSet[[int]$proc.ProcessId] = $true
    }
    Get-AllWindows | Where-Object {
        $pidSet.ContainsKey([int]$_.Pid) -and
        ($_.Width -gt 120) -and
        ($_.Height -gt 80)
    } | Sort-Object @{ Expression = { -1 * ($_.Width * $_.Height) } }
}

function Analyze-Image {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }
    Add-Type -AssemblyName System.Drawing
    $bmp = [System.Drawing.Bitmap]::FromFile($Path)
    try {
        $stepX = [Math]::Max(1, [int]($bmp.Width / 60))
        $stepY = [Math]::Max(1, [int]($bmp.Height / 60))
        $samples = 0
        $nonDark = 0
        $redLeft = 0
        $blueLeft = 0
        $colors = @{}
        $leftLimit = [int]($bmp.Width * 0.32)
        for ($y = 0; $y -lt $bmp.Height; $y += $stepY) {
            for ($x = 0; $x -lt $bmp.Width; $x += $stepX) {
                $c = $bmp.GetPixel($x, $y)
                $samples += 1
                if (($c.R + $c.G + $c.B) -gt 30) {
                    $nonDark += 1
                }
                $key = "{0:X2}{1:X2}{2:X2}" -f $c.R, $c.G, $c.B
                $colors[$key] = $true
                if ($x -lt $leftLimit) {
                    if ($c.R -ge 170 -and $c.G -le 120 -and $c.B -le 120) {
                        $redLeft += 1
                    }
                    if ($c.B -ge 140 -and $c.R -le 140 -and $c.G -ge 80) {
                        $blueLeft += 1
                    }
                }
            }
        }
        return [pscustomobject]@{
            Width = $bmp.Width
            Height = $bmp.Height
            SampleCount = $samples
            NonDarkSamples = $nonDark
            UniqueSampleColors = $colors.Count
            LeftSidebarRedSamples = $redLeft
            LeftSidebarBlueSamples = $blueLeft
            MostlyBlank = ($samples -gt 0 -and ($nonDark / [double]$samples) -lt 0.02)
            FileSize = (Get-Item -LiteralPath $Path).Length
        }
    } finally {
        $bmp.Dispose()
    }
}

function Capture-PrintWindow {
    param($Window, [string]$Path, [string]$Label)
    Add-NativeTypes
    Add-Type -AssemblyName System.Drawing
    $width = [Math]::Max(1, [int]$Window.Width)
    $height = [Math]::Max(1, [int]$Window.Height)
    $bmp = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $graphics.GetHdc()
    $ok = $false
    try {
        $ok = [WinProbe.Native]::PrintWindow([IntPtr]::new([int64]$Window.HwndInt64), $hdc, 2)
    } finally {
        $graphics.ReleaseHdc($hdc)
        $graphics.Dispose()
    }
    try {
        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $bmp.Dispose()
    }
    $stats = Analyze-Image -Path $Path
    return [pscustomobject]@{
        Label = $Label
        Method = "PrintWindow"
        Ok = [bool]$ok
        Path = $Path
        Stats = $stats
    }
}

function Capture-BitBlt {
    param($Window, [string]$Path, [string]$Label)
    Add-NativeTypes
    Add-Type -AssemblyName System.Drawing
    $width = [Math]::Max(1, [int]$Window.Width)
    $height = [Math]::Max(1, [int]$Window.Height)
    $bmp = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bmp)
    $dest = $graphics.GetHdc()
    $screen = [WinProbe.Native]::GetDC([IntPtr]::Zero)
    $ok = $false
    try {
        $ok = [WinProbe.Native]::BitBlt($dest, 0, 0, $width, $height, $screen, [int]$Window.Left, [int]$Window.Top, 0x00CC0020)
    } finally {
        if ($screen -ne [IntPtr]::Zero) {
            [void][WinProbe.Native]::ReleaseDC([IntPtr]::Zero, $screen)
        }
        $graphics.ReleaseHdc($dest)
        $graphics.Dispose()
    }
    try {
        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $bmp.Dispose()
    }
    $stats = Analyze-Image -Path $Path
    return [pscustomobject]@{
        Label = $Label
        Method = "BitBlt"
        Ok = [bool]$ok
        Path = $Path
        Stats = $stats
    }
}

function Get-SidebarWidth {
    param([int]$WindowWidth)
    if ($WindowWidth -le 0) {
        return 0
    }
    $proportional = [int][Math]::Floor($WindowWidth * 0.18)
    $clamped = [Math]::Max(260, [Math]::Min(340, $proportional))
    return [Math]::Min($WindowWidth, $clamped)
}

function Get-ProjectSidebarWidth {
    param([int]$WindowWidth)
    $proportional = [int][Math]::Round([Math]::Max(1, $WindowWidth) * 0.158)
    return [Math]::Max(290, [Math]::Min(320, $proportional))
}

function Get-ProjectRowCenterY {
    param([int]$WindowHeight)
    return [Math]::Max(96, [Math]::Min(280, $WindowHeight - 80))
}

function Invoke-ProjectRowHoverProbe {
    param($Window, [bool]$ClickAction)
    Add-NativeTypes
    $sidebarWidth = Get-ProjectSidebarWidth -WindowWidth ([int]$Window.Width)
    $rowY = Get-ProjectRowCenterY -WindowHeight ([int]$Window.Height)
    $hoverX = [int]($Window.Left + 70)
    $hoverY = [int]($Window.Top + $rowY)
    $actionX = [int]($Window.Left + $sidebarWidth - 12)
    $actionY = $hoverY
    $original = New-Object WinProbe.Native+POINT
    $restoreCursor = [WinProbe.Native]::GetCursorPos([ref]$original)

    $hoverPath = Join-Path $script:RunDir "codex-project-row-hover.png"
    $clickPath = Join-Path $script:RunDir "codex-project-row-action-click.png"
    $message = "Hovered the current project row only."
    try {
        [void][WinProbe.Native]::SetForegroundWindow([IntPtr]::new([int64]$Window.HwndInt64))
        Start-Sleep -Milliseconds 250
        [void][WinProbe.Native]::SetCursorPos($hoverX, $hoverY)
        Start-Sleep -Milliseconds 550
        $hoverCapture = Capture-PrintWindow -Window $Window -Path $hoverPath -Label "project-row-hover"

        $clickCapture = $null
        if ($ClickAction) {
            [void][WinProbe.Native]::SetCursorPos($actionX, $actionY)
            Start-Sleep -Milliseconds 150
            [WinProbe.Native]::mouse_event([WinProbe.Native]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero)
            [WinProbe.Native]::mouse_event([WinProbe.Native]::MOUSEEVENTF_LEFTUP, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 900
            $clickCapture = Capture-PrintWindow -Window $Window -Path $clickPath -Label "project-row-action-click"
            $message = "Hovered the current project row and clicked the revealed action control; no prompt was sent."
        }

        return [pscustomobject]@{
            Requested = $true
            ClickAction = $ClickAction
            HoverPoint = "$hoverX,$hoverY"
            ActionPoint = "$actionX,$actionY"
            HoverCapture = $hoverCapture
            ClickCapture = $clickCapture
            Message = $message
        }
    } finally {
        if ($restoreCursor) {
            [void][WinProbe.Native]::SetCursorPos($original.X, $original.Y)
        }
    }
}

function Add-UiaAssemblies {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
}

function Test-UiaPattern {
    param($Element, $Pattern)
    $patternObject = $null
    try {
        return $Element.TryGetCurrentPattern($Pattern, [ref]$patternObject)
    } catch {
        return $false
    }
}

function Format-RectPart {
    param($Value)
    try {
        $number = [double]$Value
        if ([double]::IsInfinity($number)) {
            return "inf"
        }
        if ([double]::IsNaN($number)) {
            return "nan"
        }
        return ([int][Math]::Round($number)).ToString()
    } catch {
        return "?"
    }
}

function Dump-UiaTree {
    param($Window, [string]$TreePath, [bool]$Send)
    Add-UiaAssemblies
    $root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]::new([int64]$Window.HwndInt64))
    if ($null -eq $root) {
        throw "UIA root is null"
    }

    $lines = New-Object System.Collections.Generic.List[string]
    $hits = New-Object System.Collections.Generic.List[object]
    $editCandidates = New-Object System.Collections.Generic.List[object]
    $buttonCandidates = New-Object System.Collections.Generic.List[object]
    $maxNodes = 700
    $maxDepth = 8
    $script:UiaNodeCount = 0

    function Walk-UiaNode {
        param($Element, [int]$Depth)
        if ($null -eq $Element -or $script:UiaNodeCount -ge $maxNodes -or $Depth -gt $maxDepth) {
            return
        }
        $script:UiaNodeCount += 1
        try {
            $cur = $Element.Current
            $name = [string]$cur.Name
            $automationId = [string]$cur.AutomationId
            $className = [string]$cur.ClassName
            $controlType = $cur.ControlType.ProgrammaticName
            $rect = $cur.BoundingRectangle
            $rectX = Format-RectPart $rect.X
            $rectY = Format-RectPart $rect.Y
            $rectWidth = Format-RectPart $rect.Width
            $rectHeight = Format-RectPart $rect.Height
            $patterns = New-Object System.Collections.Generic.List[string]
            if (Test-UiaPattern $Element ([System.Windows.Automation.ValuePattern]::Pattern)) { [void]$patterns.Add("Value") }
            if (Test-UiaPattern $Element ([System.Windows.Automation.InvokePattern]::Pattern)) { [void]$patterns.Add("Invoke") }
            if (Test-UiaPattern $Element ([System.Windows.Automation.TextPattern]::Pattern)) { [void]$patterns.Add("Text") }
            if (Test-UiaPattern $Element ([System.Windows.Automation.SelectionItemPattern]::Pattern)) { [void]$patterns.Add("SelectionItem") }
            $indent = "  " * $Depth
            $line = "{0}- {1} name='{2}' aid='{3}' class='{4}' rect=({5},{6},{7},{8}) patterns=[{9}]" -f `
                $indent,
                $controlType,
                (Shorten $name 90),
                (Shorten $automationId 60),
                (Shorten $className 60),
                $rectX,
                $rectY,
                $rectWidth,
                $rectHeight,
                (($patterns | ForEach-Object { $_ }) -join ",")
            [void]$lines.Add($line)

            $search = "$name $automationId $className $controlType"
            if ($search -match "(thread|chat|conversation|new|send|submit|continue|stop|failed|error|input|prompt|composer|message|!)") {
                [void]$hits.Add([pscustomobject]@{
                    Depth = $Depth
                    ControlType = $controlType
                    Name = $name
                    AutomationId = $automationId
                    ClassName = $className
                    Patterns = (($patterns | ForEach-Object { $_ }) -join ",")
                    Rect = ("{0},{1},{2},{3}" -f $rectX, $rectY, $rectWidth, $rectHeight)
                })
            }
            if ($controlType -eq "ControlType.Edit" -or $patterns.Contains("Value")) {
                [void]$editCandidates.Add([pscustomobject]@{
                    ControlType = $controlType
                    Name = $name
                    AutomationId = $automationId
                    Patterns = (($patterns | ForEach-Object { $_ }) -join ",")
                    Rect = ("{0},{1},{2},{3}" -f $rectX, $rectY, $rectWidth, $rectHeight)
                    Element = $Element
                })
            }
            if ($controlType -eq "ControlType.Button" -or $patterns.Contains("Invoke")) {
                [void]$buttonCandidates.Add([pscustomobject]@{
                    ControlType = $controlType
                    Name = $name
                    AutomationId = $automationId
                    Patterns = (($patterns | ForEach-Object { $_ }) -join ",")
                    Rect = ("{0},{1},{2},{3}" -f $rectX, $rectY, $rectWidth, $rectHeight)
                    Element = $Element
                })
            }

            $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
            $child = $walker.GetFirstChild($Element)
            while ($null -ne $child -and $script:UiaNodeCount -lt $maxNodes) {
                Walk-UiaNode -Element $child -Depth ($Depth + 1)
                $child = $walker.GetNextSibling($child)
            }
        } catch {
            [void]$lines.Add(("  " * $Depth) + "- <uia read failed: " + (Shorten $_.Exception.Message 120) + ">")
        }
    }

    Walk-UiaNode -Element $root -Depth 0
    Set-Content -LiteralPath $TreePath -Value $lines -Encoding UTF8

    $sendAttempt = [pscustomobject]@{
        Requested = $Send
        Attempted = $false
        Method = "none"
        Ok = $false
        Message = "Not requested; dry run only."
        StealsFocus = $false
        MovesMouse = $false
    }

    if ($Send) {
        $targetEdit = $editCandidates | Where-Object { $_.Patterns -match "Value" } | Select-Object -Last 1
        $targetButton = $buttonCandidates | Where-Object { $_.Patterns -match "Invoke" -and ($_.Name -match "(send|submit|continue)" -or $_.AutomationId -match "(send|submit|continue)") } | Select-Object -First 1
        if ($targetEdit) {
            $valuePatternObject = $null
            if ($targetEdit.Element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$valuePatternObject)) {
                $sendAttempt.Attempted = $true
                $sendAttempt.Method = "UIA ValuePattern"
                $valuePatternObject.SetValue($ProbePrompt)
                if ($targetButton) {
                    $invokePatternObject = $null
                    if ($targetButton.Element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invokePatternObject)) {
                        $invokePatternObject.Invoke()
                        $sendAttempt.Method = "UIA ValuePattern + InvokePattern"
                        $sendAttempt.Ok = $true
                        $sendAttempt.Message = "Prompt set and invoke requested."
                    }
                } else {
                    $sendAttempt.Ok = $true
                    $sendAttempt.Message = "Prompt set; no send button candidate was invoked."
                }
            }
        } else {
            $sendAttempt.Attempted = $true
            $sendAttempt.Method = "UIA"
            $sendAttempt.Message = "No ValuePattern edit candidate found."
        }
    }

    return [pscustomobject]@{
        NodeCount = $script:UiaNodeCount
        TreePath = $TreePath
        SearchHits = @($hits | Select-Object -First 80)
        EditCandidates = @($editCandidates | Select-Object -First 20 -Property ControlType, Name, AutomationId, Patterns, Rect)
        ButtonCandidates = @($buttonCandidates | Select-Object -First 30 -Property ControlType, Name, AutomationId, Patterns, Rect)
        SendAttempt = $sendAttempt
    }
}

function Relative-ArtifactPath {
    param([string]$Path)
    try {
        return (Resolve-Path -LiteralPath $Path).Path
    } catch {
        return $Path
    }
}

$repoRoot = (Resolve-Path (Get-Location)).Path
$gitBranch = Get-GitText -GitArgs @("branch", "--show-current")
$gitHead = Get-GitText -GitArgs @("rev-parse", "HEAD")
$gitStatus = Get-GitText -GitArgs @("status", "--short", "--branch")
$gitRemote = Get-GitText -GitArgs @("remote", "-v")

$processes = @(Get-CodexProcesses)
$cliCandidates = @(Get-CliCandidates -Processes $processes)
$latestThread = Get-LatestCodexThread
if (-not $ThreadId -and $latestThread) {
    $ThreadId = [string]$latestThread.id
}

$uriResult = [ordered]@{
    Tested = $false
    Uri = ""
    OpenError = ""
    ForegroundAfterOpen = $null
    CodexWindowFoundAfterOpen = $false
}

if ($ThreadId -and -not $SkipUriOpen) {
    $uri = "codex://threads/$ThreadId"
    $uriResult.Tested = $true
    $uriResult.Uri = $uri
    try {
        Start-Process $uri
        Start-Sleep -Milliseconds 2600
    } catch {
        $uriResult.OpenError = $_.Exception.Message
    }
    $processes = @(Get-CodexProcesses)
    $foreground = Get-ForegroundWindowInfo
    $uriResult.ForegroundAfterOpen = $foreground
}

$codexWindows = @(Get-CodexWindows -Processes $processes)
if ($uriResult.Tested) {
    $uriResult.CodexWindowFoundAfterOpen = ($codexWindows.Count -gt 0)
}

$mainWindow = $codexWindows | Where-Object { $_.Visible -and $_.Width -gt 500 -and $_.Height -gt 350 } | Select-Object -First 1
if (-not $mainWindow) {
    $mainWindow = $codexWindows | Select-Object -First 1
}

$captures = New-Object System.Collections.Generic.List[object]
$uiaResult = $null
$uiaError = ""
$hoverProbe = $null
$hoverProbeError = ""

if ($mainWindow) {
    $basePrint = Join-Path $script:RunDir "codex-window-printwindow.png"
    $baseBitBlt = Join-Path $script:RunDir "codex-window-bitblt.png"
    try { [void]$captures.Add((Capture-PrintWindow -Window $mainWindow -Path $basePrint -Label "normal")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "normal"; Method = "PrintWindow"; Ok = $false; Path = $basePrint; Stats = $null; Error = $_.Exception.Message }) }
    try { [void]$captures.Add((Capture-BitBlt -Window $mainWindow -Path $baseBitBlt -Label "normal")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "normal"; Method = "BitBlt"; Ok = $false; Path = $baseBitBlt; Stats = $null; Error = $_.Exception.Message }) }

    if (-not $SkipOcclusionTest -and -not $mainWindow.Minimized) {
        try {
            Add-Type -AssemblyName System.Windows.Forms
            Add-Type -AssemblyName System.Drawing
            $form = New-Object System.Windows.Forms.Form
            $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
            $form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
            $form.TopMost = $true
            $form.ShowInTaskbar = $false
            $form.BackColor = [System.Drawing.Color]::FromArgb(255, 30, 30, 30)
            $overlayWidth = [Math]::Min([int]$mainWindow.Width, 520)
            $form.Bounds = New-Object System.Drawing.Rectangle([int]$mainWindow.Left, [int]$mainWindow.Top, $overlayWidth, [int]$mainWindow.Height)
            $form.Show()
            [System.Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 700
            $occPrint = Join-Path $script:RunDir "codex-window-occluded-printwindow.png"
            $occBitBlt = Join-Path $script:RunDir "codex-window-occluded-bitblt.png"
            try { [void]$captures.Add((Capture-PrintWindow -Window $mainWindow -Path $occPrint -Label "occluded-left-sidebar")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "occluded-left-sidebar"; Method = "PrintWindow"; Ok = $false; Path = $occPrint; Stats = $null; Error = $_.Exception.Message }) }
            try { [void]$captures.Add((Capture-BitBlt -Window $mainWindow -Path $occBitBlt -Label "occluded-left-sidebar")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "occluded-left-sidebar"; Method = "BitBlt"; Ok = $false; Path = $occBitBlt; Stats = $null; Error = $_.Exception.Message }) }
            $form.Close()
            $form.Dispose()
        } catch {
            [void]$captures.Add([pscustomobject]@{ Label = "occluded-left-sidebar"; Method = "overlay-test"; Ok = $false; Path = ""; Stats = $null; Error = $_.Exception.Message })
        }
    }

    if (-not $SkipMinimizeTest) {
        try {
            [void][WinProbe.Native]::ShowWindow([IntPtr]::new([int64]$mainWindow.HwndInt64), 6)
            Start-Sleep -Milliseconds 700
            $minPrint = Join-Path $script:RunDir "codex-window-minimized-printwindow.png"
            $minBitBlt = Join-Path $script:RunDir "codex-window-minimized-bitblt.png"
            try { [void]$captures.Add((Capture-PrintWindow -Window $mainWindow -Path $minPrint -Label "minimized")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "minimized"; Method = "PrintWindow"; Ok = $false; Path = $minPrint; Stats = $null; Error = $_.Exception.Message }) }
            try { [void]$captures.Add((Capture-BitBlt -Window $mainWindow -Path $minBitBlt -Label "minimized")) } catch { [void]$captures.Add([pscustomobject]@{ Label = "minimized"; Method = "BitBlt"; Ok = $false; Path = $minBitBlt; Stats = $null; Error = $_.Exception.Message }) }
        } finally {
            [void][WinProbe.Native]::ShowWindow([IntPtr]::new([int64]$mainWindow.HwndInt64), 9)
            Start-Sleep -Milliseconds 700
        }
    }

    $treePath = Join-Path $script:RunDir "codex-uia-tree.txt"
    try {
        $uiaResult = Dump-UiaTree -Window $mainWindow -TreePath $treePath -Send ([bool]$SendProbePrompt)
    } catch {
        $uiaError = $_.Exception.Message
    }

    if ($HoverProjectRowProbe -or $ClickProjectRowActionProbe) {
        try {
            $hoverProbe = Invoke-ProjectRowHoverProbe -Window $mainWindow -ClickAction ([bool]$ClickProjectRowActionProbe)
        } catch {
            $hoverProbeError = $_.Exception.Message
        }
    }
}

$reportPath = Join-Path $script:RunDir "windows-probe-report.md"
$lines = New-Object System.Collections.Generic.List[string]
[void]$lines.Add("# Codex Sentinel Windows Capability Probe")
[void]$lines.Add("")
[void]$lines.Add("- Generated: $(Get-Date -Format o)")
[void]$lines.Add("- Repo: $(Md-Code $repoRoot)")
[void]$lines.Add("- Branch: $(Md-Code $gitBranch)")
[void]$lines.Add("- HEAD: $(Md-Code $gitHead)")
[void]$lines.Add("- PowerShell: $(Md-Code ($PSVersionTable.PSVersion.ToString()))")
[void]$lines.Add("- OS: $(Md-Code ([System.Environment]::OSVersion.VersionString))")
[void]$lines.Add("- Dry run send: $(Md-Code (-not [bool]$SendProbePrompt))")
[void]$lines.Add("")

[void]$lines.Add("## Git Baseline")
[void]$lines.Add("")
[void]$lines.Add('```text')
[void]$lines.Add($gitStatus)
[void]$lines.Add($gitRemote)
[void]$lines.Add('```')
[void]$lines.Add("")

[void]$lines.Add("## Codex Processes")
[void]$lines.Add("")
if ($processes.Count -eq 0) {
    [void]$lines.Add("- No Codex-related process was found.")
} else {
    [void]$lines.Add("| PID | Name | Exe | Command line |")
    [void]$lines.Add("| --- | --- | --- | --- |")
    foreach ($proc in $processes) {
        [void]$lines.Add("| $($proc.ProcessId) | $(Escape-Md $proc.Name) | $(Md-Code $proc.ExecutablePath) | $(Escape-Md (Shorten $proc.CommandLine 220)) |")
    }
}
[void]$lines.Add("")

[void]$lines.Add("## Codex CLI Candidates")
[void]$lines.Add("")
if ($cliCandidates.Count -eq 0) {
    [void]$lines.Add("- No bundled or PATH Codex CLI candidate was found.")
} else {
    [void]$lines.Add("| Source | Exists | Path |")
    [void]$lines.Add("| --- | --- | --- |")
    foreach ($cli in $cliCandidates) {
        [void]$lines.Add("| $(Escape-Md $cli.Source) | $($cli.Exists) | $(Md-Code $cli.Path) |")
    }
}
[void]$lines.Add("")

[void]$lines.Add("## Recent Thread / URI")
[void]$lines.Add("")
if ($latestThread) {
    [void]$lines.Add("- Latest thread id: $(Md-Code $latestThread.id)")
    [void]$lines.Add("- Latest thread title: $(Escape-Md (Shorten ([string]$latestThread.title) 120))")
    [void]$lines.Add("- Latest thread cwd: $(Md-Code ([string]$latestThread.cwd))")
    [void]$lines.Add("- Latest rollout: $(Md-Code ([string]$latestThread.rollout_path))")
} else {
    [void]$lines.Add('- Could not read latest thread from `%USERPROFILE%\.codex\state_5.sqlite`.')
}
if ($uriResult.Tested) {
    [void]$lines.Add("- URI tested: $(Md-Code $uriResult.Uri)")
    [void]$lines.Add("- URI open error: $(Md-Code $uriResult.OpenError)")
    [void]$lines.Add("- Codex window found after open: $(Md-Code $uriResult.CodexWindowFoundAfterOpen)")
    if ($uriResult.ForegroundAfterOpen) {
        [void]$lines.Add("- Foreground after open: hwnd $(Md-Code $uriResult.ForegroundAfterOpen.Hwnd), pid $(Md-Code $uriResult.ForegroundAfterOpen.Pid), title $(Md-Code $uriResult.ForegroundAfterOpen.Title)")
    }
} else {
    [void]$lines.Add("- URI open was skipped or no thread id was available.")
}
[void]$lines.Add("")

[void]$lines.Add("## Codex Windows")
[void]$lines.Add("")
if ($codexWindows.Count -eq 0) {
    [void]$lines.Add("- No Codex windows were found.")
} else {
    [void]$lines.Add("| HWND | PID | Visible | Minimized | Rect | Title | Class |")
    [void]$lines.Add("| --- | --- | --- | --- | --- | --- | --- |")
    foreach ($win in $codexWindows) {
        $rectText = "$($win.Left),$($win.Top),$($win.Width)x$($win.Height)"
        [void]$lines.Add("| $(Md-Code $win.Hwnd) | $($win.Pid) | $($win.Visible) | $($win.Minimized) | $(Md-Code $rectText) | $(Escape-Md (Shorten $win.Title 90)) | $(Escape-Md $win.ClassName) |")
    }
}
[void]$lines.Add("")

[void]$lines.Add("## UI Automation")
[void]$lines.Add("")
if ($uiaResult) {
    [void]$lines.Add("- UIA node count dumped: $(Md-Code $uiaResult.NodeCount)")
    [void]$lines.Add('- UIA walker: `RawViewWalker`')
    [void]$lines.Add("- Tree artifact: [codex-uia-tree.txt]($(Split-Path -Leaf $uiaResult.TreePath))")
    [void]$lines.Add("- Edit candidates: $(Md-Code (@($uiaResult.EditCandidates).Count))")
    [void]$lines.Add("- Button/invoke candidates: $(Md-Code (@($uiaResult.ButtonCandidates).Count))")
    [void]$lines.Add("- Send attempt requested: $(Md-Code $uiaResult.SendAttempt.Requested)")
    [void]$lines.Add("- Send attempt result: $(Md-Code $uiaResult.SendAttempt.Method) / ok $(Md-Code $uiaResult.SendAttempt.Ok) / $(Escape-Md $uiaResult.SendAttempt.Message)")
    [void]$lines.Add("- Focus/mouse impact: UIA dry run does not move the mouse; URI open and minimize/restore tests can change focus.")
    [void]$lines.Add("")
    [void]$lines.Add("### UIA Search Hits")
    if (@($uiaResult.SearchHits).Count -eq 0) {
        [void]$lines.Add("- No keyword hits were found in the UIA tree.")
    } else {
        [void]$lines.Add("| Type | Name | AutomationId | Class | Patterns | Rect |")
        [void]$lines.Add("| --- | --- | --- | --- | --- | --- |")
        foreach ($hit in @($uiaResult.SearchHits | Select-Object -First 25)) {
            [void]$lines.Add("| $(Escape-Md $hit.ControlType) | $(Escape-Md (Shorten $hit.Name 80)) | $(Escape-Md (Shorten $hit.AutomationId 60)) | $(Escape-Md (Shorten $hit.ClassName 60)) | $(Escape-Md $hit.Patterns) | $(Md-Code $hit.Rect) |")
        }
    }
    [void]$lines.Add("")
    [void]$lines.Add("### UIA Tree Snippet")
    [void]$lines.Add('```text')
    $snippet = Get-Content -LiteralPath $uiaResult.TreePath -TotalCount 80
    foreach ($line in $snippet) {
        [void]$lines.Add($line)
    }
    [void]$lines.Add('```')
} else {
    [void]$lines.Add("- UIA dump failed: $(Md-Code $uiaError)")
}
[void]$lines.Add("")

[void]$lines.Add("## Screenshot / Pixel Probe")
[void]$lines.Add("")
if ($captures.Count -eq 0) {
    [void]$lines.Add("- No screenshot probe was run.")
} else {
    [void]$lines.Add("| Label | Method | API ok | Mostly blank | Size | Unique sample colors | Left red samples | Left blue samples | Artifact |")
    [void]$lines.Add("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    foreach ($capture in $captures) {
        $stats = $capture.Stats
        if ($stats) {
            $sizeText = "$($stats.Width)x$($stats.Height), $($stats.FileSize) bytes"
            $mostlyBlank = $stats.MostlyBlank
            $unique = $stats.UniqueSampleColors
            $red = $stats.LeftSidebarRedSamples
            $blue = $stats.LeftSidebarBlueSamples
        } else {
            $sizeText = ""
            $mostlyBlank = ""
            $unique = ""
            $red = ""
            $blue = ""
        }
        $artifact = ""
        if ($capture.Path) {
            $artifact = "[" + (Split-Path -Leaf $capture.Path) + "](" + (Split-Path -Leaf $capture.Path) + ")"
        }
        [void]$lines.Add("| $(Escape-Md $capture.Label) | $(Escape-Md $capture.Method) | $($capture.Ok) | $mostlyBlank | $(Escape-Md $sizeText) | $unique | $red | $blue | $artifact |")
    }
}
[void]$lines.Add("")

[void]$lines.Add("## Project Row Hover Probe")
[void]$lines.Add("")
if ($hoverProbe) {
    [void]$lines.Add("- Requested: $(Md-Code $hoverProbe.Requested)")
    [void]$lines.Add("- Click revealed action: $(Md-Code $hoverProbe.ClickAction)")
    [void]$lines.Add("- Hover point: $(Md-Code $hoverProbe.HoverPoint)")
    [void]$lines.Add("- Action point: $(Md-Code $hoverProbe.ActionPoint)")
    [void]$lines.Add("- Result: $(Escape-Md $hoverProbe.Message)")
    if ($hoverProbe.HoverCapture -and $hoverProbe.HoverCapture.Path) {
        [void]$lines.Add("- Hover artifact: [" + (Split-Path -Leaf $hoverProbe.HoverCapture.Path) + "](" + (Split-Path -Leaf $hoverProbe.HoverCapture.Path) + ")")
    }
    if ($hoverProbe.ClickCapture -and $hoverProbe.ClickCapture.Path) {
        [void]$lines.Add("- Click artifact: [" + (Split-Path -Leaf $hoverProbe.ClickCapture.Path) + "](" + (Split-Path -Leaf $hoverProbe.ClickCapture.Path) + ")")
    }
} elseif ($HoverProjectRowProbe -or $ClickProjectRowActionProbe) {
    [void]$lines.Add("- Probe failed: $(Md-Code $hoverProbeError)")
} else {
    [void]$lines.Add("- Not requested. Pass `-HoverProjectRowProbe` to capture the sidebar project hover state; pass `-ClickProjectRowActionProbe` only when opening a new visible chat is acceptable.")
}
[void]$lines.Add("")

[void]$lines.Add("## Findings")
[void]$lines.Add("")
if ($processes.Count -gt 0) {
    [void]$lines.Add("- PASS: Codex Desktop processes are discoverable through Win32/CIM.")
} else {
    [void]$lines.Add("- FAIL: Codex Desktop process discovery did not find a candidate.")
}
if (@($cliCandidates | Where-Object { $_.Exists }).Count -gt 0) {
    [void]$lines.Add("- PASS: At least one Codex CLI candidate exists.")
} else {
    [void]$lines.Add("- FAIL: No existing Codex CLI candidate was verified.")
}
if ($uriResult.Tested -and $uriResult.CodexWindowFoundAfterOpen -and [string]::IsNullOrEmpty($uriResult.OpenError)) {
    [void]$lines.Add('- PASS: `codex://threads/<id>` was accepted by Windows shell and a Codex window was found after open.')
} elseif ($uriResult.Tested) {
    [void]$lines.Add("- FAIL: URI open was attempted but did not produce a clean Codex window result.")
} else {
    [void]$lines.Add("- UNKNOWN: URI open was not tested.")
}
if ($codexWindows.Count -gt 0) {
    [void]$lines.Add("- PASS: Codex HWND enumeration works.")
} else {
    [void]$lines.Add("- FAIL: No Codex HWND was found.")
}
if ($uiaResult -and $uiaResult.NodeCount -gt 1) {
    [void]$lines.Add("- PASS: UI Automation can attach to the Codex main window and dump a tree.")
} else {
    [void]$lines.Add("- FAIL: UI Automation tree was unavailable or empty.")
}
if (@($captures | Where-Object { $_.Method -eq "PrintWindow" -and $_.Ok -and $_.Stats -and -not $_.Stats.MostlyBlank }).Count -gt 0) {
    [void]$lines.Add("- PASS: PrintWindow produced at least one nonblank Codex capture.")
} else {
    [void]$lines.Add("- RISK: PrintWindow did not verify as a reliable nonblank capture path.")
}
if (@($captures | Where-Object { $_.Method -eq "BitBlt" -and $_.Ok -and $_.Stats -and -not $_.Stats.MostlyBlank }).Count -gt 0) {
    [void]$lines.Add("- PASS: BitBlt produced at least one nonblank Codex capture.")
} else {
    [void]$lines.Add("- RISK: BitBlt did not verify as a reliable nonblank capture path.")
}
if ($uiaResult -and @($uiaResult.EditCandidates).Count -gt 0) {
    [void]$lines.Add('- PARTIAL: UIA exposed edit/value candidates; actual safe send remains gated behind `-SendProbePrompt`.')
} else {
    [void]$lines.Add("- RISK: UIA did not expose an obvious edit/value candidate.")
}
[void]$lines.Add("")

[void]$lines.Add("## Next Steps")
[void]$lines.Add("")
[void]$lines.Add("- Keep Windows visible control behind the Rust platform layer (`src/desktop_control/windows.rs`) and avoid moving Win32/UIA assumptions into macOS code.")
[void]$lines.Add("- Use UIA first if a future Codex build exposes stable edit/button/list semantics; current verified fallback is Win32 HWND + constrained left-sidebar screenshot analysis.")
[void]$lines.Add("- For real send probes, use disposable threads or explicit user-approved targets only; visible send uses focus, clipboard, and keyboard input.")
[void]$lines.Add("- Collect real Failed/StoppedMarker sidebar samples and tune the selected-row marker thresholds against those fixtures.")

Set-Content -LiteralPath $reportPath -Value $lines -Encoding UTF8

$summary = [pscustomobject]@{
    Report = $reportPath
    RunDir = $script:RunDir
    ThreadId = $ThreadId
    ProcessCount = $processes.Count
    WindowCount = $codexWindows.Count
    UiaNodeCount = if ($uiaResult) { $uiaResult.NodeCount } else { 0 }
    CaptureCount = $captures.Count
    SendAttempted = if ($uiaResult) { $uiaResult.SendAttempt.Attempted } else { $false }
    HoverProjectRowProbe = [bool]$hoverProbe
}

$summary | ConvertTo-Json -Depth 6
