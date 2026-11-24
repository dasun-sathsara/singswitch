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
# `sc delete` only marks a service for deletion; it disappears when the last
# handle closes. Removing the install directory underneath a still-terminating
# service is how an uninstall leaves a locked, half-deleted tree behind.
foreach ($svc in @("singswitch-agent", "sing-box")) {
    $found = Get-Service -Name $svc -ErrorAction SilentlyContinue
    if (-not $found) { continue }
    if ($found.Status -ne 'Stopped') {
        Stop-Service -Name $svc -Force -ErrorAction SilentlyContinue
        $found.WaitForStatus('Stopped', '00:00:30')
    }
    sc.exe delete $svc | Out-Null
    for ($i = 0; $i -lt 60; $i++) {
        if (-not (Get-Service -Name $svc -ErrorAction SilentlyContinue)) { break }
        Start-Sleep -Milliseconds 500
    }
}

Write-Host "==> Removing Start Menu shortcut, HKCU Run entry, and binaries..."
Remove-Item -Path "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\singswitch.lnk" -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "singswitch" -ErrorAction SilentlyContinue
Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
# install.ps1 appended $InstallDir to the machine PATH. Leave it behind and
# every shell keeps a directory that no longer exists.
$machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
if ($machinePath) {
    $cleaned = ($machinePath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $InstallDir.TrimEnd('\') }) -join ';'
    if ($cleaned -ne $machinePath) {
        [Environment]::SetEnvironmentVariable("Path", $cleaned, "Machine")
    }
}
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
