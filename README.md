# sbctl-rs

`sbctl-rs` is a native desktop GUI, menu bar / system tray controller, and CLI for managing `sing-box` profiles and the privileged service that runs `sing-box` in TUN mode on **macOS** (Apple Silicon) and **Windows**.

It rewrites the original Go `sbctl` CLI into a shared Rust workspace (`sbctl-core`, `sbctl-cli`, and `sbctl-app`) built on GPUI (`gpui-kit`) and `gpui-tray`, preserving the canonical OS paths, sudoers model, verified activation with crash-loop rollback, and JSON CLI contract.

## Architecture and workspace layout

```text
sbctl-rs/
├── assets/
│   ├── icons/                # App icons (.png, .ico, .icns)
│   └── skeleton.json         # Embedded VLESS + Reality TUN seed config
├── crates/
│   ├── sbctl-core/           # Core logic: layout, sudoers, activator, verifier, service, logs, doctor
│   ├── sbctl-cli/            # `sbctl` CLI and `sbctl agent` Windows SCM named-pipe helper
│   └── sbctl-app/            # Native desktop GUI and macOS menu bar / Windows tray app
├── docs/
│   └── framework-research.md # Rust desktop framework evaluation and benchmark measurements
├── resources/
│   ├── macos/Info.plist      # LSUIElement bundle metadata (`app.lexiflix.sbctl`)
│   └── windows/app.manifest  # PerMonitorV2 DPI and UTF-8 manifest
└── scripts/                  # Cross-checks, .app bundling, and macOS/Windows installers
```

### Shared behavior preserved from the Go implementation

- **Canonical macOS paths**: `/usr/local/etc/sing-box/profiles/*.json`, `/usr/local/etc/sing-box/config.json` (symlink to active profile), LaunchDaemon `system/app.lexiflix.singbox` (`/Library/LaunchDaemons/app.lexiflix.singbox.plist`), and `/var/log/sing-box/{error,access}.log`.
- **Canonical Windows paths**: `%ProgramData%\sing-box\profiles\*.json`, `%ProgramData%\sing-box\config.json` (atomic copy), `%ProgramData%\sbctl\active-profile`, `%ProgramData%\sing-box\logs`, and WinSW service `sing-box`.
- **Verified activation and crash-loop rollback**: captures a pre-restart sample, updates the active config atomically, restarts the service, polls for startup, waits `2500ms` to catch `KeepAlive` / WinSW crash loops (restarts, PID changes, exit codes), and automatically rolls back to the previous config and state if the tunnel does not stay healthy.
- **Generic placeholder guard**: blocks activation whenever any `TODO_[A-Z0-9_]+` token remains in the profile JSON.
- **Remote profiles**: `.url` sidecars stored alongside pulled `.json` profiles with `sbctl pull` and `sbctl sync`.

## Features in `sbctl-app`

- **macOS menu bar (`NSStatusItem`) & Windows system tray (`Shell_NotifyIconW`)**:
  - Dynamic `ActivationPolicy` on macOS: starts with `LSUIElement` / `Accessory` mode when launched with `--background`, switches to `Regular` (Dock icon visible) when windows are open, and returns to `Accessory` when closed.
  - Native menus with live tunnel status, public egress IP, one-click Connect / Disconnect, checkmarked ready profiles, disabled `(setup required)` profiles, and remote sync.
- **Dashboard**: connection hero card with live phase progress spinner, public IP check (`ipinfo.io/json`), and quick-switch profile cards.
- **Guided setup & JSON editor**:
  - **Guided setup**: lossless `ProfileFormModel` editing server, port, UUID/password, flow, SNI, uTLS fingerprint, Reality keys, TUN interface/MTU/stack, and IPv6/route toggles without dropping custom DNS, route rules, or `endpoints` (such as Tailscale `tsnet`).
  - **JSON editor**: multi-line syntax-highlighted editor (`syntect`), JSON formatting, live syntax status bar, external disk-change warning without clobbering unsaved edits, inline `TODO_` placeholder badges, `sing-box check` validation on the live buffer (`Validate`), `Save` (`Cmd/Ctrl+S`), and one-click `Save & Connect` (`Cmd/Ctrl+Shift+S`).
- **Logs viewer**: virtualized scrolling (`v_virtual_list`) with zero-copy cached filtering over `/var/log/sing-box/{error,access}.log` (or `%ProgramData%\sing-box\logs`), incremental background tailing, severity filters, text search, full-line detail inspector, `Clear`, and clipboard copy.
- **Doctor & settings**: full diagnostic checklist, copyable remediation hints, macOS sudoers repair staged to a temp file and checked via `visudo -cf` before installation, restrained light/dark/system theme switcher with automatic OS appearance sync, persisted window geometry, adaptive idle background polling (`5s` open / `30s` tray-only), keyboard shortcuts (`Cmd/Ctrl+1..5`, `N`, `I`, `S`, `R`, `W`, `Q`), and per-user launch-at-login registration (`~/Library/LaunchAgents/app.lexiflix.sbctl.plist` or `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).

## Building and verifying

### Host tests and lints (`sbctl-core` and `sbctl-cli`)

```bash
cargo test --workspace --exclude sbctl-app
cargo clippy --workspace --exclude sbctl-app --all-targets -- -D warnings
```

Note: `gpui-kit` targets Apple Silicon / macOS AppKit and Windows Win32/DirectX inside this project, so full `sbctl-app` compilation and linting are checked against `x86_64-pc-windows-msvc` and `aarch64-apple-darwin`.

### Cross-checking on Linux

Requires `x86_64-pc-windows-msvc` and `aarch64-apple-darwin` rustup targets plus `llvm-18` (`llvm-rc-18`, `llvm-cvtres-18`) for Windows resources:

```bash
./scripts/check-windows.sh
./scripts/check-macos.sh
```

### Native builds and installation

On **macOS** (Apple Silicon):

```bash
cargo build --release -p sbctl-cli -p sbctl-app
./scripts/bundle-macos.sh aarch64-apple-darwin release
./scripts/install-macos.sh
```

To build a macOS `.app` bundle from Linux using an extracted Xcode SDK (`MacOSX14.5.sdk`), set `MAC_SDK=/path/to/MacOSX14.5.sdk` and run `./scripts/build-mac.sh`.

To uninstall on macOS (add `--purge` to delete profiles and logs):

```bash
./scripts/uninstall-macos.sh
```

On **Windows** (from an elevated Administrator PowerShell prompt after `cargo build --release --workspace --target x86_64-pc-windows-msvc`):

```powershell
.\scripts\install.ps1
```

To uninstall on Windows (add `-Purge` to remove `%ProgramData%\sing-box` and `%ProgramData%\sbctl`):

```powershell
.\scripts\uninstall.ps1
```

## CLI reference

Run `sbctl --help` for full flags (`--json` with schema version `1`, `--plain`, `--no-color`, `-q`, `-v`):

- `sbctl` / `sbctl use [name]` - interactive picker or verified switch
- `sbctl list` (`ls`) - list profiles and readiness state
- `sbctl off` - stop `sing-box`
- `sbctl status` - show active profile, endpoint, TUN interface, PID, and health
- `sbctl logs [-n 80] [-f]` - stream log output
- `sbctl add <name>` / `sbctl edit <name>` - scaffold or edit and validate a profile
- `sbctl pull <name> <url>` / `sbctl sync [name]` - download and update remote profiles
- `sbctl check [name]` / `sbctl rm <name> [--force]` / `sbctl ip` / `sbctl doctor`
- `sbctl version` / `sbctl completion <shell>`

## Roadmap and future work

- **Linux desktop support**: future work will add `systemd` unit management, polkit / scoped sudoers rules, and Linux StatusNotifierItem (`D-Bus`) tray support.
- **Signed installers**: code signing and notarization for macOS `.dmg` releases and Authenticode signing for Windows `.msi` / Inno Setup packages.

## License

MIT. See [LICENSE](LICENSE).
