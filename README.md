# SingSwitch

Switch [sing-box](https://sing-box.sagernet.org/) TUN profiles on macOS and Windows. SingSwitch
is a desktop app, a menu bar or system tray controller, and a CLI, all in Rust and GPUI.

The point of it is the switch itself. SingSwitch checks the profile with `sing-box check` before
touching anything, swaps the config, restarts the service, and watches it for 2.5 seconds. If the
daemon exits, changes PID, or restarts, the previous profile and service state come back.

## Install

macOS:

```bash
make bundle-macos
./scripts/install-macos.sh
```

Windows. Cross-compile or build natively, stage the managed binary, then run PowerShell as
administrator:

```powershell
cargo build --release --workspace --target x86_64-pc-windows-msvc
./scripts/fetch-sing-box.sh windows-amd64 target/x86_64-pc-windows-msvc/release
.\scripts\install.ps1
```

Uninstall with `./scripts/uninstall-macos.sh` or `.\scripts\uninstall.ps1`. Both take a purge
flag (`--purge` / `-Purge`) that also deletes profiles and logs.

The installer copies a pinned, checksum-verified sing-box to a root-owned path, so the daemon
never runs a binary from Homebrew, WinGet, or your home directory. You do not need sing-box
installed.

## CLI

```
singswitch                     status panel and interactive profile picker
singswitch use <name>          switch to a profile, roll back if the service dies
singswitch off                 stop sing-box
singswitch status              what sing-box is doing right now
singswitch list                profiles, plus which fields are still empty
singswitch add <name>          create from the template, then edit
singswitch pull <name> <url>   download a profile and validate it
singswitch sync [name]         re-fetch one or all remote profiles
singswitch edit <name>         edit and validate, reloading if it is active
singswitch rm <name>           delete a profile
singswitch check [name]        validate the named or active profile
singswitch logs [-n 80] [-f]   follow sing-box output
singswitch ip                  public address traffic is leaving from
singswitch completion <shell>  shell completions
```

Flags are `--json`, `--plain`, `--no-color`, `-q`, and `-v`.

Every command takes `--json` and prints one document, with errors shaped as
`{"schema":1,"code":N,"error":"...","hints":[...]}`. Exit codes: `0` success, `1` general,
`2` validation, `3` service failure, `4` missing privileges. Nothing prompts when stdin is not
a terminal, so the CLI works in scripts without flags. The full contract, including why `--set`
refuses to invent sections, is in [docs/cli-json.md](docs/cli-json.md).

## Where things live

| | macOS | Windows |
| --- | --- | --- |
| Service | launchd `app.lexiflix.singswitch-daemon` | WinSW `sing-box`, plus `singswitch-agent` |
| sing-box | `/usr/local/libexec/singswitch/sing-box` | `%ProgramData%\singswitch\bin\sing-box.exe` |
| Active config | symlink at `/usr/local/etc/sing-box/config.json` | copy at `%ProgramData%\sing-box\config.json` |
| Logs | `/var/log/sing-box/{error,access}.log` | `%ProgramData%\sing-box\logs\` |

The GUI and the CLI run as your user. Service control goes through four fixed `NOPASSWD` sudo
rules on macOS, and through a named pipe or UAC on Windows. Nothing runs as root for longer than
one of those commands. [SECURITY.md](SECURITY.md) has the rules and what they do not protect
against.

## When the window does not appear

A bundled app has no terminal, so startup writes to a log file:
`~/Library/Logs/singswitch/app.log` on macOS, `%LOCALAPPDATA%\singswitch\logs\app.log` on Windows.
Each line has a UTC timestamp, including tray setup, window creation, and panic messages.

## Development

```bash
make check      # format check and clippy, with warnings denied
make cross      # macOS and Windows cross-compilation checks
```

Building the macOS bundle from Linux needs `MAC_SDK=/path/to/MacOSX14.5.sdk` and shader stitching
first; `scripts/cross-env.sh` and `scripts/stitch-macos-shaders.sh` do that.
[docs/ui-plan.md](docs/ui-plan.md) covers the interface design and its open questions.

## License

MIT. See [LICENSE](LICENSE).
