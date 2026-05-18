param(
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
Set-Location $Root

if ([string]::IsNullOrWhiteSpace($Version)) {
    $PackageJson = Get-Content -Raw -Path (Join-Path $Root "package.json") | ConvertFrom-Json
    $Version = [string]$PackageJson.version
}

npm run tauri:build:windows

# Do not rebuild the main Tauri binary with plain `cargo build` here.
# Tauri CLI sets the production context that loads bundled `ui-dist`; a plain
# cargo rebuild makes the WebView try the dev server at 127.0.0.1:5173.
cargo build --release --bin codex-sentinel-gui

$HelperSource = Join-Path $Root "target\release\codex-sentinel.exe"
if (-not (Test-Path -LiteralPath $HelperSource)) {
    throw "Windows release helper exe not found: $HelperSource"
}

$LauncherSource = Join-Path $Root "target\release\codex-sentinel-gui.exe"
if (-not (Test-Path -LiteralPath $LauncherSource)) {
    throw "Windows release GUI launcher exe not found: $LauncherSource"
}

$Dist = Join-Path $Root "dist"
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$Target = Join-Path $Dist "Codex Sentinel_${Version}_windows_x64.exe"
$HelperTarget = Join-Path $Dist "codex-sentinel-cli.exe"

$PackagedExePaths = @($Target, $HelperTarget)
Get-CimInstance Win32_Process |
    Where-Object {
        $exe = $_.ExecutablePath
        -not [string]::IsNullOrWhiteSpace($exe) -and
            ($PackagedExePaths | Where-Object { [string]::Equals($_, $exe, [System.StringComparison]::OrdinalIgnoreCase) })
    } |
    ForEach-Object {
        Write-Host "Stopping running packaged process $($_.ProcessId): $($_.ExecutablePath)"
        Stop-Process -Id $_.ProcessId -Force
        try {
            Wait-Process -Id $_.ProcessId -Timeout 10 -ErrorAction SilentlyContinue
        } catch {
            Write-Warning "Timed out waiting for process $($_.ProcessId) to exit"
        }
    }

Copy-Item -LiteralPath $LauncherSource -Destination $Target -Force
Copy-Item -LiteralPath $HelperSource -Destination $HelperTarget -Force

Write-Host "Packaged $Target"
Write-Host "Packaged $HelperTarget"
