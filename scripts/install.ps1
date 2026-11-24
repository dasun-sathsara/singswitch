#Requires -Version 5.1
#Requires -RunAsAdministrator
param(
    [string]$BinDir = "$PSScriptRoot\..\target\x86_64-pc-windows-msvc\release",
    [string]$SingBoxExe = "$env:ProgramData\singswitch\bin\sing-box.exe",
    [string]$WinSwExe = "$env:ProgramFiles\sing-box\winsw.exe",
    # Install the app without registering the sing-box service. Documented in
    # the failure path above, so it has to exist.
    [switch]$SkipService
)

$ErrorActionPreference = "Stop"

# Both service registrations below are re-entrant for the same reason: install
# over a live install must work, and Windows refuses to create a service whose
# name is still registered, or to delete one while a process holds it.
function Remove-SingSwitchService {
    param([string]$Name)
    if (-not (Get-Service -Name $Name -ErrorAction SilentlyContinue)) { return }
    # `sc delete` only marks the service; it disappears when the last handle
    # closes, so wait for the name to be free instead of racing the `create`.
    sc.exe delete $Name | Out-Null
    for ($i = 0; $i -lt 60; $i++) {
        if (-not (Get-Service -Name $Name -ErrorAction SilentlyContinue)) { return }
        Start-Sleep -Milliseconds 500
    }
    throw "Service $Name is still registered after deletion; reboot or stop the process holding it."
}

$InstallDir = "$env:ProgramFiles\singswitch"
$SingBoxData = "$env:ProgramData\sing-box"
$ProfilesDir = "$SingBoxData\profiles"
$LogsDir = "$SingBoxData\logs"
$SingSwitchState = "$env:ProgramData\singswitch"

Write-Host "==> Creating directories..."
New-Item -ItemType Directory -Force -Path $InstallDir, $ProfilesDir, $LogsDir, $SingSwitchState | Out-Null

$singswitchSrc = Join-Path $BinDir "singswitch.exe"
$appSrc = Join-Path $BinDir "singswitch-app.exe"
if (-not (Test-Path $singswitchSrc)) { throw "Missing $singswitchSrc. Build with: cargo build --release --workspace --target x86_64-pc-windows-msvc" }
if (-not (Test-Path $appSrc))   { throw "Missing $appSrc. Build with: cargo build --release --workspace --target x86_64-pc-windows-msvc" }

Write-Host "==> Copying binaries to $InstallDir..."
# Windows keeps a running image locked, so overwriting the helper binary while
# its service is up fails. Re-running the installer over an existing install is
# the documented repair step, so stop the services before the copies, not after.
$AgentService = "singswitch-agent"
foreach ($svc in @($AgentService, "sing-box")) {
    $found = Get-Service -Name $svc -ErrorAction SilentlyContinue
    if ($found -and $found.Status -ne 'Stopped') {
        Stop-Service -Name $svc -Force -ErrorAction SilentlyContinue
        # Stop-Service returns once the stop is requested; the image stays
        # locked until the process is actually gone.
        $found.WaitForStatus('Stopped', '00:00:30')
    }
}
Copy-Item $singswitchSrc "$InstallDir\singswitch.exe" -Force
Copy-Item $appSrc   "$InstallDir\singswitch-app.exe" -Force

Write-Host "==> Installing the managed sing-box binary..."
$ManagedBinDir = "$SingSwitchState\bin"
New-Item -ItemType Directory -Force -Path $ManagedBinDir | Out-Null
$BundledSingBox = Join-Path $BinDir "sing-box\sing-box.exe"
if (Test-Path $BundledSingBox) {
    Copy-Item $BundledSingBox $SingBoxExe -Force
    $BundledDll = Join-Path $BinDir "sing-box\libcronet.dll"
    if (Test-Path $BundledDll) {
        Copy-Item $BundledDll (Join-Path $ManagedBinDir "libcronet.dll") -Force
    }
} elseif (-not (Test-Path $SingBoxExe)) {
    Write-Warning "No managed sing-box found at $SingBoxExe; stage one with scripts/fetch-sing-box.sh windows-amd64 into $BinDir."
}

# Harden ProgramData ACLs so unprivileged users can read logs/profiles while service updates remain protected.
icacls $SingSwitchState /inheritance:r /grant "*S-1-5-18:(OI)(CI)F" /grant "*S-1-5-32-544:(OI)(CI)F" /grant "*S-1-5-32-545:(OI)(CI)R" | Out-Null
# The tray app runs unprivileged and still has to exec the managed sing-box for
# `check` and `version`, so the binary directory needs execute rights for Users.
icacls $ManagedBinDir /grant "*S-1-5-32-545:(OI)(CI)RX" | Out-Null
icacls $SingBoxData /inheritance:r /grant "*S-1-5-18:(OI)(CI)F" /grant "*S-1-5-32-544:(OI)(CI)F" /grant "*S-1-5-32-545:(OI)(CI)RX" | Out-Null
icacls $ProfilesDir /grant "*S-1-5-32-545:(OI)(CI)M" | Out-Null
icacls $LogsDir     /grant "*S-1-5-32-545:(OI)(CI)R" | Out-Null

# Add install directory to machine PATH if absent.
$machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
$pathParts = @($machinePath -split ';' | Where-Object { $_ })
if ($pathParts -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", (@($pathParts) + $InstallDir) -join ';', "Machine")
}

# Register WinSW sing-box service when WinSW and sing-box binaries are present.
if ($SkipService) {
    Write-Host "Skipping sing-box service registration (-SkipService)."
} elseif ((Test-Path $WinSwExe) -and (Test-Path $SingBoxExe)) {
    $ServiceDir = Split-Path -Parent $WinSwExe
    $WinSwXml = Join-Path $ServiceDir "sing-box.xml"
    # WinSW `install` fails outright when the service already exists, and the
    # XML is only read at install time, so re-running the installer needs the
    # old registration gone before the rewritten config can take effect.
    Remove-SingSwitchService "sing-box"
    $EscSingBoxExe = [System.Security.SecurityElement]::Escape($SingBoxExe)
    $EscArguments = [System.Security.SecurityElement]::Escape("run -c `"$SingBoxData\config.json`"")
    $EscLogsDir = [System.Security.SecurityElement]::Escape($LogsDir)
    @"
<service>
  <id>sing-box</id>
  <name>sing-box</name>
  <description>sing-box universal proxy service managed by singswitch</description>
  <executable>$EscSingBoxExe</executable>
  <arguments>$EscArguments</arguments>
  <logpath>$EscLogsDir</logpath>
  <log mode="roll-by-size">
    <sizeThreshold>10240</sizeThreshold>
    <keepFiles>4</keepFiles>
  </log>
  <startmode>Manual</startmode>
  <onfailure action="restart" delay="3 sec"/>
</service>
"@ | Set-Content -Encoding UTF8 $WinSwXml

    & $WinSwExe install "$WinSwXml" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "WinSW failed to install the sing-box service (exit $LASTEXITCODE)."
    }
} else {
    # Reporting success over a missing service is worse than stopping: Doctor
    # then advises re-running this script, which would "succeed" again.
    throw "WinSW ($WinSwExe) or sing-box ($SingBoxExe) not found. Install both, or run with -SkipService."
}

# Register privileged singswitch named-pipe helper service (allows unprivileged tray/GUI control without UAC prompts).
Remove-SingSwitchService $AgentService
sc.exe create $AgentService binPath= "`"$InstallDir\singswitch.exe`" agent" start= auto DisplayName= "SingSwitch Helper" | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "sc.exe could not create the $AgentService service (exit $LASTEXITCODE)."
}
sc.exe start $AgentService | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "sc.exe could not start the $AgentService service (exit $LASTEXITCODE)."
}

# Create Start Menu shortcut.
$ShortcutPath = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\singswitch.lnk"
$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = "$InstallDir\singswitch-app.exe"
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.Description = "Manage sing-box profiles and service"
$Shortcut.Save()

Write-Host "==> Installation complete. Launch SingSwitch from the Start Menu."
