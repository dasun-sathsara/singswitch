# SingSwitch

`singswitch` is a desktop app, tray controller, and CLI for switching [sing-box](https://sing-box.sagernet.org/) TUN profiles on macOS and Windows. Written in Rust (`singswitch-core`, `singswitch-cli`, `singswitch-app`) on GPUI, it preserves the on-disk paths, scoped sudoers rules, verified switches, and `--json` schema from the Go CLI.

## How it works

- **Verified switches with rollback.** `singswitch` validates profiles with `sing-box check`, refuses to activate a profile whose required fields are still empty (naming them), updates the active config atomically, restarts the service, and watches it for 2.5 seconds. It restores the prior profile and state if the daemon exits, changes PID, or restarts.
- **Tray and desktop app.** `singswitch-app` runs in the macOS menu bar or Windows system tray and hides the Dock icon when its window closes. Switch profiles from the menu, edit VLESS and Reality fields without dropping custom DNS or routing JSON, and tail logs.
- **Scoped privileges.** Profile editing and the GUI run unprivileged. Service control uses four fixed `NOPASSWD` sudo rules on macOS and the `singswitch-agent` named pipe or UAC on Windows. See [SECURITY.md](SECURITY.md).
- **Bundled sing-box.** The release carries a pinned, checksum-verified sing-box (`scripts/sing-box.lock`). The installer copies it to a root-owned path, so the daemon never execs a Homebrew, WinGet, or user-writable binary, and no separate sing-box install is needed.

## Platform layout

| Platform | Service | sing-box binary | Active config | Logs |
| --- | --- | --- | --- | --- |
| macOS (Apple Silicon) | launchd daemon (`app.lexiflix.singswitch-daemon`) | `/usr/local/libexec/singswitch/sing-box` | symlink at `/usr/local/etc/sing-box/config.json` | `/var/log/sing-box/{error,access}.log` |
| Windows (x86_64) | WinSW (`sing-box`) and `singswitch-agent` | `%ProgramData%\singswitch\bin\sing-box.exe` | copy at `%ProgramData%\sing-box\config.json` | `%ProgramData%\sing-box\logs\sing-box.{err,out}.log` |

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

Remove with `.\scripts\uninstall.ps1`, or pass `-Purge` to remove `%ProgramData%\sing-box` and `%ProgramData%\singswitch`.

## CLI

```bash
singswitch                     # status panel and interactive profile picker
singswitch list                # list profiles and readiness state
singswitch use <name>          # validate, switch, and roll back on crash
singswitch off                 # stop sing-box
singswitch status              # service state, profile, server, TUN, and PID
singswitch logs [-n 80] [-f]   # tail or follow log output
singswitch add <name>          # create a profile from template and edit it
singswitch edit <name>         # edit, validate, and reload if active
singswitch edit <name> --set server=203.0.113.9 --set uuid=...   # no editor
singswitch edit <name> --from - < new.json                       # replace from stdin
singswitch pull <name> <url>   # fetch and validate a remote profile JSON
singswitch sync [name]         # re-fetch one or all remote profiles
singswitch rm <name> [--force] # delete a profile
singswitch check [name]        # validate a profile with sing-box check
singswitch ip                  # show public egress IP and location
singswitch version             # print build and schema version
singswitch completion <shell>  # generate shell completions
```

Global flags: `--json` (schema version `1`), `--plain`, `--no-color`, `-q`, and `-v`.

### Scripting

Every command accepts `--json` and prints one JSON document to stdout, with errors as
`{"schema":1,"code":N,"error":"...","hints":[...]}`. Exit codes are stable: `0` success,
`1` general (usage, missing profile, I/O, network), `2` validation, `3` service failure,
`4` missing privileges. No command prompts when
stdin is not a terminal, and `add`/`edit` never require `$EDITOR` when `--from` or `--set`
is given. Profiles report `ready` and a `missing` list naming the fields still to fill, so a
script can tell "not configured yet" from "configured wrongly". `logs --json --follow`
emits JSON Lines — one object per line, plus a `{"gap":...}` line when the log rotated
mid-stream.

`--set` rejects an unknown key, an out-of-range port, and a malformed value before anything
is written. It also fails when a value has nowhere to go: setting `tun_mtu` on a config with
no tunnel is an error naming the missing section, not a silent no-op. Values are written only
into sections the document already has — a one-field edit never adds a `log` section or a tag
to an untagged outbound.

## Development

```bash
make check    # format check, clippy (-D warnings), and workspace tests
make cross    # macOS and Windows cross-compilation checks
```

Set `MAC_SDK=/path/to/MacOSX14.5.sdk` and run `make bundle-macos` to cross-build `dist/singswitch.app` from Linux.

A bundled app has no terminal, so startup records go to a file. If the window never appears, read `~/Library/Logs/singswitch/app.log` on macOS or `%LOCALAPPDATA%\singswitch\logs\app.log` on Windows. Each line carries a UTC timestamp, and the file includes tray availability, window creation results, and panic messages.

## License

MIT. See [LICENSE](LICENSE).
