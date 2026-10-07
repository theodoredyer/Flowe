# Copies the release build to %LOCALAPPDATA%\parakey and makes it start at login.
# Run again after `cargo build --release` to update. Pass -Uninstall to remove the startup entry.
param([switch]$Uninstall)
$ErrorActionPreference = "Stop"
$dest = Join-Path $env:LOCALAPPDATA "parakey"
$link = Join-Path ([Environment]::GetFolderPath("Startup")) "parakey.lnk"

if ($Uninstall) {
    Get-Process parakey -ErrorAction SilentlyContinue | Stop-Process
    Remove-Item $link -ErrorAction SilentlyContinue
    "removed startup entry (model and exe left in $dest)"
    return
}

$src = Join-Path $PSScriptRoot "target\release"
if (-not (Test-Path "$src\parakey.exe")) { throw "build first: cargo build --release" }
New-Item -ItemType Directory -Force $dest | Out-Null
Get-Process parakey -ErrorAction SilentlyContinue | Stop-Process; Start-Sleep -Milliseconds 300
Copy-Item "$src\parakey.exe", "$src\DirectML.dll" $dest -Force

$sh = New-Object -ComObject WScript.Shell
$s = $sh.CreateShortcut($link)
$s.TargetPath = Join-Path $dest "parakey.exe"
$s.WorkingDirectory = $dest
$s.Description = "parakey - local dictation"
$s.Save()

if (-not (Test-Path (Join-Path $dest "model\vocab.txt"))) { "model missing: run .\download-model.ps1" }
Start-Process (Join-Path $dest "parakey.exe")
"installed to $dest and added to startup; parakey is running (look for the mic icon in the tray)"
