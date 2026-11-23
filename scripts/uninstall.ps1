#Requires -Version 5.1
#Requires -RunAsAdministrator
param(
    [switch]$Purge
)

$ErrorActionPreference = "Stop"

$InstallDir = "$env:ProgramFiles\singswitch"
$SingBoxData = "$env:ProgramData\sing-box"
$SingSwitchState = "$env:ProgramData\singswitch"

Write-Host "==> Stopping and removing singswitch-agent and sing-box services..."
if (Get-Service -Name "singswitch-agent" -ErrorAction SilentlyContinue) {
    Stop-Service -Name "singswitch-agent" -Force -ErrorAction SilentlyContinue
    sc.exe delete "singswitch-agent" | Out-Null
}
if (Get-Service -Name "sing-box" -ErrorAction SilentlyContinue) {
    Stop-Service -Name "sing-box" -Force -ErrorAction SilentlyContinue
    sc.exe delete "sing-box" | Out-Null
}

Write-Host "==> Removing Start Menu shortcut, HKCU Run entry, and binaries..."
Remove-Item -Path "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\singswitch.lnk" -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "singswitch" -ErrorAction SilentlyContinue
Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
# The managed sing-box copy is ours, unlike profiles and logs, so it always goes.
Remove-Item -Path "$SingSwitchState\bin" -Recurse -Force -ErrorAction SilentlyContinue

if ($Purge) {
    Write-Host "==> Removing $SingBoxData and $SingSwitchState..."
    Remove-Item -Path $SingBoxData -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $SingSwitchState  -Recurse -Force -ErrorAction SilentlyContinue
} else {
    Write-Host "==> Keeping $SingBoxData\profiles (pass -Purge to remove)."
}

Write-Host "==> Uninstall complete."
