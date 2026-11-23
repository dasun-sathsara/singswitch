# Security

`singswitch` controls `sing-box`, which opens a TUN interface and routes host traffic as `root` on macOS or `LocalSystem` on Windows. The security boundary keeps a compromised unprivileged user session or malformed input from turning `singswitch` into an arbitrary privileged command runner.

## Privilege boundaries

### macOS

No persistent root helper runs for `singswitch`. Both `singswitch` and `singswitch-app` run as the logged-in console user and invoke `sudo -n` only for four exact commands in `/etc/sudoers.d/singswitch`:

```text
<user> ALL=(root) NOPASSWD: /bin/launchctl print system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl kickstart -k system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl bootout system/app.lexiflix.singswitch-daemon
<user> ALL=(root) NOPASSWD: /bin/launchctl bootstrap system /Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist
```

`Layout::sudo_commands` generates both `singswitch print-sudoers` output and runtime `sudo` arguments from one table. Staged installs validate the temporary rules file with `visudo -cf` before installing it as `0440 root:wheel`, and activation verifies the rule with `sudo -n -l` before relying on it.

### Windows

`scripts/install.ps1` registers `singswitch-agent` as a `LocalSystem` service listening on `\\.\pipe\singswitch-control` with SDDL `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`. It also removes inheritance on `%ProgramData%\sing-box`, limiting writes to `SYSTEM` and `Administrators` while granting `Users` modify rights only inside `profiles\`. The managed sing-box under `%ProgramData%\singswitch\bin` is writable only by `SYSTEM` and `Administrators`; `Users` get read and execute because the unprivileged tray app shells out to it for `check` and `version`.

Both `singswitch-agent` and the `--elevated-op` UAC fallback deserialize typed JSON `PipeRequest` variants (`Probe`, `Activate`, `Stop`, `CreateProfile`, `PullProfile`, `SyncOne`, `SyncAll`, `SaveProfile`, `DeleteProfile`, `EnsureProfilesDir`, `RepairWindowsHelper`) and never execute arbitrary shell commands.

## Input and runtime checks

| Area | Check |
| --- | --- |
| Profile names | Reject `/`, `\`, `..`, leading `-`, whitespace, control characters, DOS device names, and names over 64 bytes; verify symlinks stay inside `profiles/`. |
| Activation and rollback | Refuse activation while the outbound is missing fields it needs (named in the error), run `sing-box check` before swapping config, and revert if the service exits, restarts, or changes PID within 2.5 seconds. |
| Remote pulls | Accept `http://` and `https://` URLs only, cap bodies at 8 MiB, require valid JSON, and pass `sing-box check` on a temporary file before saving. |
| Notifications | Strip control characters and escape quotes and backslashes for `osascript`; strip control characters and XML-escape WinRT toast payloads. |
| Concurrency | Serialize switches, pulls, syncs, and saves with an exclusive `singswitch.lock` file lock. |

## Privileged binaries

Both platforms run a sing-box that ordinary users cannot replace. macOS installs a root-owned copy at `/usr/local/libexec/singswitch/sing-box`; Windows installs one at `%ProgramData%\singswitch\bin\sing-box.exe` (plus `libcronet.dll`). The installer takes both from the checksum-verified release pinned in `scripts/sing-box.lock`, and the LaunchDaemon and WinSW service point at that path. A sing-box from Homebrew, WinGet, or `PATH` is only used as a fallback when no managed copy exists.

## Known limitation: user-writable profiles

Profile files stay writable by the console user so you can edit configs without elevation, while `sing-box` reads the active file as `root` or `LocalSystem`. Directives such as `log.output` or `cache_file` inside a profile make `sing-box` write to paths chosen in that JSON, Treat write access to `profiles/` as privileged on the machine. Activating a profile is an ordinary symlink swap performed as the console user, not a privileged command: the installer gives that user the config directory. No sudoers rule names a wildcard or a user-writable path.

Scoped sudoers rules and typed Windows RPC stop `singswitch` from executing arbitrary commands, not privileged file writes performed by `sing-box`.
