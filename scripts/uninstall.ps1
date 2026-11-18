#Requires -Version 5.1
#Requires -RunAsAdministrator
param(
    [switch]$Purge
)

$ErrorActionPreference = "Stop"

$InstallDir = "$env:ProgramFiles\sbctl"
$SingBoxData = "$env:ProgramData\sing-box"
$SbctlState = "$env:ProgramData\sbctl"

Write-Host "==> Stopping and removing sbctl-agent and sing-box services..."
if (Get-Service -Name "sbctl-agent" -ErrorAction SilentlyContinue) {
    Stop-Service -Name "sbctl-agent" -Force -ErrorAction SilentlyContinue
    sc.exe delete "sbctl-agent" | Out-Null
}
if (Get-Service -Name "sing-box" -ErrorAction SilentlyContinue) {
    Stop-Service -Name "sing-box" -Force -ErrorAction SilentlyContinue
    sc.exe delete "sing-box" | Out-Null
}

Write-Host "==> Removing Start Menu shortcut, HKCU Run entry, and binaries..."
Remove-Item -Path "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\sbctl.lnk" -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "sbctl" -ErrorAction SilentlyContinue
Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction SilentlyContinue

if ($Purge) {
    Write-Host "==> Removing $SingBoxData and $SbctlState..."
    Remove-Item -Path $SingBoxData -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $SbctlState  -Recurse -Force -ErrorAction SilentlyContinue
} else {
    Write-Host "==> Keeping $SingBoxData\profiles (pass -Purge to remove)."
}

Write-Host "==> Uninstall complete."
