# Security

SingSwitch runs sing-box, which opens a TUN interface and routes all host traffic as `root` on
macOS or `LocalSystem` on Windows. The boundary below is about one thing: a compromised user
session, or a malformed profile, must not turn SingSwitch into a way to run any command as root.

## macOS

No root helper stays resident. `singswitch` and `singswitch-app` run as the logged-in user and
call `sudo -n` for exactly four commands, listed in `/etc/sudoers.d/singswitch`:

```text
<user> ALL=(root) NOPASSWD: /bin/launchctl print system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl kickstart -k system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl bootout system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl bootstrap system /Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist
```

`Layout::sudo_commands` builds both the `print-sudoers` output and the arguments used at runtime
from that one table, so the two cannot drift. The installer validates the staged file with
`visudo -cf` and installs it `0440 root:wheel`. Before relying on a rule, activation confirms it
with `sudo -n -l`.

## Windows

`scripts/install.ps1` registers `singswitch-agent` as a `LocalSystem` service on
`\\.\pipe\singswitch-control`, with SDDL `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`. It removes
inheritance on `%ProgramData%\sing-box` so only `SYSTEM` and `Administrators` can write there,
granting `Users` modify rights inside `profiles\` alone. The managed binary under
`%ProgramData%\singswitch\bin` is writable by `SYSTEM` and `Administrators` only; `Users` get read
and execute because the tray app shells out to it for `check` and `version`.

Both `singswitch-agent` and the `--elevated-op` UAC fallback deserialize a fixed set of typed
`PipeRequest` variants (`Probe`, `Activate`, `Stop`, `CreateProfile`, `PullProfile`, `SyncOne`,
`SyncAll`, `SaveProfile`, `DeleteProfile`, `EnsureProfilesDir`, `RepairWindowsHelper`). No variant
carries a shell command, so there is nothing to inject.

## Input checks

| Area | Check |
| --- | --- |
| Profile names | Rejects `/`, `\`, `..`, a leading `-`, whitespace, control characters, DOS device names, and names over 64 bytes. Symlinks must resolve inside `profiles/`. |
| Activation | Refuses to activate while required fields are empty, runs `sing-box check` before the swap, and reverts if the service exits, restarts, or changes PID within 2.5 seconds. |
| Remote pulls | `http://` and `https://` only, 8 MiB cap, valid JSON required, and a `sing-box check` on a temporary file before saving. |
| Notifications | Control characters stripped; quotes and backslashes escaped for `osascript`; XML-escaped for WinRT toasts. |
| Concurrency | Switches, pulls, syncs, and saves serialize on an exclusive `.singswitch.lock`. |

## Privileged binaries

On both platforms the daemon runs a sing-box that an ordinary user cannot replace. macOS gets a
root-owned copy at `/usr/local/libexec/singswitch/sing-box`; Windows gets
`%ProgramData%\singswitch\bin\sing-box.exe` with `libcronet.dll`. Both come from the
checksum-verified release pinned in `scripts/sing-box.lock`. A Homebrew, WinGet, or `PATH` sing-box
is used only when no managed copy exists.

## What this does not protect

Profiles stay writable by the console user so you can edit configs without elevation, but sing-box
reads the active file as root. A directive like `log.output` or `cache_file` makes sing-box write
wherever that JSON points, as root. Treat write access to `profiles/` as a privileged capability
on that machine.

Activation is a symlink swap performed as your user, not a privileged command; the installer gives
that user the config directory. No sudoers rule names a wildcard or a user-writable path.

The scoped rules and typed Windows RPC stop SingSwitch from executing arbitrary commands. They do
not constrain what sing-box itself writes once it holds root.
