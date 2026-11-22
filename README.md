# sbctl-rs

`sbctl` is a desktop app, tray controller, and CLI for switching [sing-box](https://sing-box.sagernet.org/) TUN profiles on macOS and Windows. Written in Rust (`sbctl-core`, `sbctl-cli`, `sbctl-app`) on GPUI, it preserves the on-disk paths, scoped sudoers rules, verified switches, and `--json` schema from the Go CLI.

## How it works

- **Verified switches with rollback.** `sbctl` validates profiles with `sing-box check`, blocks remaining `TODO_` placeholders, updates the active config atomically, restarts the service, and watches it for 2.5 seconds. It restores the prior profile and state if the daemon exits, changes PID, or restarts.
- **Tray and desktop app.** `sbctl-app` runs in the macOS menu bar or Windows system tray and hides the Dock icon when its window closes. Switch profiles from the menu, edit VLESS and Reality fields without dropping custom DNS or routing JSON, and tail logs.
- **Scoped privileges.** Profile editing and the GUI run unprivileged. Service control uses five fixed `NOPASSWD` sudo rules on macOS and the `sbctl-agent` named pipe or UAC on Windows. See [SECURITY.md](SECURITY.md).
- **Bundled sing-box.** The release carries a pinned, checksum-verified sing-box (`scripts/sing-box.lock`). The installer copies it to a root-owned path, so the daemon never execs a Homebrew, WinGet, or user-writable binary, and no separate sing-box install is needed.

## Platform layout

| Platform | Service | sing-box binary | Active config | Logs |
| --- | --- | --- | --- | --- |
| macOS (Apple Silicon) | launchd daemon (`app.lexiflix.singbox`) | `/usr/local/libexec/sbctl/sing-box` | symlink at `/usr/local/etc/sing-box/config.json` | `/var/log/sing-box/{error,access}.log` |
| Windows (x86_64) | WinSW (`sing-box`) and `sbctl-agent` | `%ProgramData%\sbctl\bin\sing-box.exe` | copy at `%ProgramData%\sing-box\config.json` | `%ProgramData%\sing-box\logs\sing-box.{err,out}.log` |

## Install

### macOS

```bash
make bundle-macos
./scripts/install-macos.sh
```

Remove with `./scripts/uninstall-macos.sh`, or pass `--purge` to delete profiles and logs.

### Windows

Build on Windows or cross-compile (`cargo build --release --workspace --target x86_64-pc-windows-msvc`), stage the managed binary with `./scripts/fetch-sing-box.sh windows-amd64 target/x86_64-pc-windows-msvc/release`, then run from an elevated PowerShell prompt:

```powershell
.\scripts\install.ps1
```

Remove with `.\scripts\uninstall.ps1`, or pass `-Purge` to remove `%ProgramData%\sing-box` and `%ProgramData%\sbctl`.

## CLI

```bash
sbctl                     # status panel and interactive profile picker
sbctl list                # list profiles and readiness state
sbctl use <name>          # validate, switch, and roll back on crash
sbctl off                 # stop sing-box
sbctl status              # service state, profile, server, TUN, and PID
sbctl logs [-n 80] [-f]   # tail or follow log output
sbctl add <name>          # create a profile from template and edit it
sbctl edit <name>         # edit, validate, and reload if active
sbctl pull <name> <url>   # fetch and validate a remote profile JSON
sbctl sync [name]         # re-fetch one or all remote profiles
sbctl rm <name> [--force] # delete a profile
sbctl check [name]        # validate a profile with sing-box check
sbctl ip                  # show public egress IP and location
sbctl version             # print build and schema version
sbctl completion <shell>  # generate shell completions
```

Global flags: `--json` (schema version `1`), `--plain`, `--no-color`, `-q`, and `-v`.

## Development

```bash
make check    # format check, clippy (-D warnings), and workspace tests
make cross    # macOS and Windows cross-compilation checks
```

Set `MAC_SDK=/path/to/MacOSX14.5.sdk` and run `make bundle-macos` to cross-build `dist/sbctl.app` from Linux.

A bundled app has no terminal, so startup records go to a file. If the window never appears, read `~/Library/Logs/sbctl/app.log` on macOS or `%LOCALAPPDATA%\sbctl\logs\app.log` on Windows. Each line carries a UTC timestamp, and the file includes tray availability, window creation results, and panic messages.

## License

MIT. See [LICENSE](LICENSE).
