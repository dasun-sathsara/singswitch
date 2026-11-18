# Security model and privilege boundaries

`sbctl-rs` controls a VPN daemon (`sing-box`) that opens a system TUN interface and routes host network traffic as `root` / `LocalSystem`, while keeping profile editing and the desktop UI unprivileged.

## Privilege architecture

### macOS (`Darwin`)

- **No persistent root daemon for control**: the desktop app (`sbctl-app`) and `sbctl` CLI run as the unprivileged console user.
- **Scoped sudoers rules**: `sbctl` writes a dedicated `/etc/sudoers.d/sbctl` file containing five explicit `NOPASSWD` command patterns limited to `/bin/ln` and `/bin/launchctl` against `system/app.lexiflix.singbox` and `/usr/local/etc/sing-box`:
  - `/bin/ln -sfn /usr/local/etc/sing-box/profiles/*.json /usr/local/etc/sing-box/config.json`
  - `/bin/launchctl bootstrap system /Library/LaunchDaemons/app.lexiflix.singbox.plist`
  - `/bin/launchctl bootout system/app.lexiflix.singbox`
  - `/bin/launchctl kickstart -k system/app.lexiflix.singbox`
  - `/bin/launchctl print system/app.lexiflix.singbox`
- **Hardening**:
  - Profile names strictly disallow `/`, `\`, `..`, leading dashes `-`, empty strings, whitespace control characters, Windows DOS device names, and names exceeding 64 characters.
  - Even with wildcards in `/etc/sudoers.d/sbctl`, `sbctl-core` enforces canonical filename validation and verifies the resolved target stays directly inside `/usr/local/etc/sing-box/profiles/`.
  - `visudo -cf` validates every sudoers file on a temporary staged file (`0440` root:wheel) before atomic installation into `/etc/sudoers.d/sbctl`.
  - Username validation rejects spaces, colons, commas, equals, newlines, and non-ASCII/control characters before rendering sudoers rules.

### Windows

- **Two supported privilege paths for GUI / CLI parity**:
  1. **Privileged helper service (`sbctl-agent`)**: installed by `scripts/install.ps1` as a Windows SCM service exposing `\\.\pipe\sbctl-agent` with an explicit security descriptor (SDDL allowing authenticated local administrators and interactive users only as configured). Only structured `PipeRequest` enum variants (`Activate { name }`, `Stop`, `SaveProfile`, `DeleteProfile`) are accepted, never arbitrary command strings.
  2. **UAC fallback (`ShellExecuteExW`)**: if the named pipe helper is unavailable, `sbctl-core` serializes the structured `PipeRequest` to a restrictive private temporary file, prompts via `runas` (`--elevated-op`), waits cleanly, and reads the JSON `PipeResponse`.
- **Filesystem ACLs**: `scripts/install.ps1` locks `%ProgramData%\sing-box` and `%ProgramData%\sbctl` inheritance so unprivileged users can inspect logs and profiles without tampering with service executables.

## Profile and network safety

- **Placeholder guard**: every `TODO_[A-Z0-9_]+` match inside a profile JSON blocks service activation and active-profile saves, preventing broken TUN routing or invalid credentials from tearing down host connectivity.
- **Validation before switch or reload**: every activation (`use`, GUI `Activate`, `pull`, `sync`, `edit`, and active `save`) runs `sing-box check -c <path>` and keeps the prior profile untouched if validation fails.
- **Crash-loop rollback**: if `sing-box` exits cleanly/uncleanly, changes PID, or increments restart counters during the `2500ms` settle window after activation, `sbctl-core` automatically restores the previous `config.json` target and restarts or stops the service to match pre-change state.
- **Remote pull constraints**:
  - Only `http://` and `https://` URLs are accepted (`file://` and local schemes are rejected).
  - Response bodies are size-capped (`10 MiB`), stripped of unexpected binary/HTML payloads by strict JSON parsing plus `sing-box check` on a temporary file, and written atomically without shell interpolation.
- **AppleScript / OS notification safety**: macOS notification strings strip non-printable control characters and escape quotes and backslashes before passing to `osascript`.
- **Advisory concurrency locking**: profile switches, pulls, syncs, and saves acquire an exclusive non-blocking/timed file lock (`sbctl.lock`) to prevent interleaved symlink swaps or partial copies.
