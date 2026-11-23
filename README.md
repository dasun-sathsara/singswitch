# SingSwitch

[sing-box](https://sing-box.sagernet.org/) opens a TUN interface and routes your traffic through one
proxy. Switching proxy means swapping the config and restarting the daemon, and when that goes
wrong you lose the network you were using to fix it.

SingSwitch does that swap and takes responsibility for the result. It validates the new profile
with `sing-box check` before touching anything, restarts the service, and watches it for 2.5
seconds. If the daemon exits, changes PID, or restarts, the previous profile comes back and you
are still online.

It ships as a menu bar app on macOS, a system tray app on Windows, and a CLI that does everything
the window does.

## Install

macOS:

```bash
make bundle-macos
./scripts/install-macos.sh
```

Windows. Build on Windows or cross-compile, stage the managed binary, then run PowerShell as
administrator:

```powershell
cargo build --release --workspace --target x86_64-pc-windows-msvc
./scripts/fetch-sing-box.sh windows-amd64 target/x86_64-pc-windows-msvc/release
.\scripts\install.ps1
```

Uninstall with `./scripts/uninstall-macos.sh` or `.\scripts\uninstall.ps1`, adding `--purge` or
`-Purge` to delete profiles and logs too.

The installer puts a pinned, checksum-verified sing-box at a root-owned path, so the daemon execs a
binary an ordinary user cannot replace. You do not need sing-box installed. A copy bundled with
the app is used next, and a `PATH` binary only when neither exists, which means before install or
on a machine where the managed copy was removed.

## CLI

```
singswitch                     status panel and interactive profile picker
singswitch use <name>          switch, with rollback if the service dies
singswitch off                 stop sing-box
singswitch status              what sing-box is doing right now
singswitch list                profiles, and which fields are still empty
singswitch add <name>          create from the template, then edit
singswitch pull <name> <url>   download a profile and validate it
singswitch sync [name]         re-fetch one or all remote profiles
singswitch edit <name>         edit and validate, reloading if it is active
singswitch rm <name>           delete a profile
singswitch check [name]        validate the named or active profile
singswitch logs [-n 80] [-f]   follow sing-box output
singswitch ip                  the public address traffic leaves from
singswitch completion <shell>  shell completions
```

Flags are `--json`, `--plain`, `--no-color`, `-q`, and `-v`.

Every command takes `--json` and prints a single document, errors included:
`{"schema":1,"code":N,"error":"...","hints":[...]}`. Exit codes stay fixed at `0` success, `1`
general, `2` validation, `3` service failure, `4` missing privileges. No command prompts when stdin
is not a terminal, so scripts need no special flags. [docs/cli-json.md](docs/cli-json.md) has the
full contract.

## Where things live

| | macOS | Windows |
| --- | --- | --- |
| Service | launchd `app.lexiflix.singswitch-daemon` | WinSW `sing-box`, plus `singswitch-agent` |
| sing-box | `/usr/local/libexec/singswitch/sing-box` | `%ProgramData%\singswitch\bin\sing-box.exe` |
| Active config | symlink at `/usr/local/etc/sing-box/config.json` | copy at `%ProgramData%\sing-box\config.json` |
| Logs | `/var/log/sing-box/{error,access}.log` | `%ProgramData%\sing-box\logs\` |

The window and the CLI run as you. Service control goes through four fixed `NOPASSWD` sudo rules on
macOS, or a named pipe and UAC on Windows, so nothing holds root between commands.
[SECURITY.md](SECURITY.md) lists the rules and, more usefully, what they do not protect against.

## When the window does not appear

A bundled app has no terminal, so startup writes to a file:
`~/Library/Logs/singswitch/app.log` on macOS, `%LOCALAPPDATA%\singswitch\logs\app.log` on Windows.
Every line carries a UTC timestamp, including tray setup, window creation, and panics.

## Development

```bash
make check      # format and clippy, warnings denied
make cross      # macOS and Windows cross-compilation
```

Cross-building the macOS bundle from Linux needs `MAC_SDK=/path/to/MacOSX14.5.sdk` and a shader
stitch first; `scripts/cross-env.sh` and `scripts/stitch-macos-shaders.sh` handle both.

## License

MIT. See [LICENSE](LICENSE).
