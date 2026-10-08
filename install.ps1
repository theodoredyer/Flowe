# Copies the release build to %LOCALAPPDATA%\Flowe and adds a Start Menu entry, so "Flowe"
# shows up when you press the Windows key and type. Run again after `cargo build --release` to update.
#   -Startup    also launch it automatically at sign-in
#   -Uninstall  remove the shortcuts (exe and model stay in %LOCALAPPDATA%\Flowe)
param([switch]$Startup, [switch]$Uninstall)
$ErrorActionPreference = "Stop"
$dest = Join-Path $env:LOCALAPPDATA "Flowe"
$exe = Join-Path $dest "flowe.exe"
$menuLink = Join-Path ([Environment]::GetFolderPath("Programs")) "Flowe.lnk"
$startupLink = Join-Path ([Environment]::GetFolderPath("Startup")) "Flowe.lnk"

if ($Uninstall) {
    Get-Process flowe -ErrorAction SilentlyContinue | Stop-Process
    Remove-Item $menuLink, $startupLink -ErrorAction SilentlyContinue
    "removed shortcuts (exe and model left in $dest)"
    return
}

$src = Join-Path $PSScriptRoot "target\release"
if (-not (Test-Path "$src\flowe.exe")) { throw "build first: cargo build --release" }
New-Item -ItemType Directory -Force $dest | Out-Null
Get-Process flowe -ErrorAction SilentlyContinue | Stop-Process
Start-Sleep -Milliseconds 300
Copy-Item "$src\flowe.exe", "$src\DirectML.dll" $dest -Force

function New-Link($path, $arguments) {
    $s = (New-Object -ComObject WScript.Shell).CreateShortcut($path)
    $s.TargetPath = $exe
    $s.Arguments = $arguments
    $s.WorkingDirectory = $dest
    $s.Description = "Local push-to-talk dictation (Ctrl+Win)"
    $s.Save()
}
New-Link $menuLink ""
# At sign-in, start quietly in the tray instead of opening the dashboard.
if ($Startup) { New-Link $startupLink "--tray" } else { Remove-Item $startupLink -ErrorAction SilentlyContinue }

if (-not (Test-Path (Join-Path $dest "model\vocab.txt"))) { "model missing: run .\download-model.ps1" }
Start-Process $exe -ArgumentList "--tray"   # quiet start; the Start Menu entry opens the dashboard
"installed to $dest; 'Flowe' is in the Start Menu$(if ($Startup) { ' and starts at sign-in' }); running now (F badge in the tray)"
