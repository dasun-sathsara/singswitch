# Desktop UI framework research for `sbctl-rs`

Before rewriting `sbctl` in Rust, we evaluated the candidate desktop toolkits with focus on native macOS menu bar (`NSStatusItem`) support, Windows system tray (`Shell_NotifyIconW`) support, resident memory footprint for an always-running background tray app, and build toolchain simplicity.

## Why `anomalyco/rune` was rejected

We inspected `anomalyco/rune` (on the `dev` branch) directly and found three hard blockers for `sbctl`:

1. **No macOS tray support**: `crates/rune/src/tray.rs` explicitly documents *"macOS isn't supported yet"* and returns `Err(TrayError::Unsupported)` on macOS.
2. **Limited Windows tray menus**: Windows tray menus support only flat `Action` and `Separator` items without checkmarks or nested submenus needed for showing the active profile cleanly.
3. **Heavy forked dependency graph**: `rune` patches five upstream crates via `[patch.crates-io]` (`wgpu-hal`, `winit`, `parley_engine`, `accesskit_macos`, `accesskit_windows`) and maintains an internal upstream mirror rather than clean crates.io releases.

## Selected stack: `gpui-kit` 0.10.0 + `gpui-tray` 0.3.0

`gpui-kit` (packaging `gpui-pre 0.3.12`, `gpui-component`, and `gpui-kit-assets`) paired with `gpui-tray 0.3.0` (`features = ["gpui-kit", "menu-state"]`) satisfies every functional and footprint requirement:

- **Native macOS menu bar**: `gpui-tray` creates a real `NSStatusItem` and `NSMenu` with SF Symbol support, `@2x` retina template icons, menu bar profile title, checkmarks, disabled items, and submenus.
- **Background lifecycle**: `QuitMode::Explicit` keeps the event loop alive with zero open windows; `ActivationPolicy` toggles dynamically between `Regular` (Dock icon visible when the Control Center is open) and `Accessory` (menu bar only when closed).
- **Native Windows tray**: `gpui-tray` uses `Shell_NotifyIconW` and native Win32 menus with left-click activation, tooltip state, and RGBA icons.
- **Included UI controls**: multi-line code editor with `syntect` syntax highlighting (`features = ["syntect"]`, no C tree-sitter dependencies), virtualized list (`v_virtual_list`) for multi-thousand-line logs, dialogs, alerts, switches, tabs, collapsible sidebar, and custom titlebar.
- **TLS without C cross-compilation pain**: `sbctl-core` uses `ureq` with `native-tls` (Security.framework on macOS, Schannel on Windows, OpenSSL on Linux).

## Benchmark comparison across Rust UI frameworks

Measured idle and active resource figures from `kkRelation/rust-ui-benchmark-windows` (`comparison-data.json`, Windows 11 desktop benchmark measuring a tray + toggleable window application):

| Framework | Hidden Tray RAM (MB) | Active Window RAM (MB) | Peak VRAM (MB) | Shared GPU (MB) | Avg CPU (%) |
| --- | ---: | ---: | ---: | ---: | ---: |
| `gpui-kit` | 19.0 | 24.8 | 49.3 | 2.0 | 0.656 |
| `gpui-fast` | 20.4 | 25.9 | 49.4 | 1.8 | 0.325 |
| `gpui-ce` | 23.7 | 36.6 | 49.4 | 2.1 | 0.658 |
| Slint | 33.2 | 61.5 | 63.1 | 23.2 | 1.304 |
| FerroUI | 45.7 | 54.4 | 72.7 | 4.6 | 0.329 |
| zgui | 87.0 | 96.6 | 133.9 | 37.3 | 0.328 |
| Tauri / WebView2 | 94.6 | 98.2 | 77.3 | 2.1 | 0.328 |
| Dioxus Desktop / WebView2 | 97.2 | 123.1 | 76.1 | 3.3 | 1.313 |
| Makepad | 124.9 | 133.9 | 62.0 | 10.6 | 0.328 |
| Iced | 133.3 | 152.0 | 79.5 | 78.4 | 0.327 |
| KAS | 151.7 | 188.4 | 241.9 | 195.6 | 0.328 |
| egui | 184.9 | 189.0 | 281.9 | 81.1 | 0.325 |
| Dioxus Native / Blitz | 189.5 | 189.9 | 119.5 | 74.2 | 0.325 |

### Takeaways

- **Lowest background memory**: `gpui-kit` stays around `19.0 MB` while sitting minimized in the tray, roughly **5x lighter than Tauri / WebView2 (`94.6 MB`)** and nearly **10x lighter than egui (`184.9 MB`)** or **Dioxus Native (`189.5 MB`)**.
- **GPU efficiency**: unlike `egui` (`281.9 MB` VRAM) or `Iced` (`78.4 MB` shared GPU memory), GPUI uses `49.3 MB` VRAM and `2.0 MB` shared GPU memory, avoiding driver allocation spikes when toggling windows.
- **Why not `Slint`**: `Slint` (`33.2 MB` hidden, `61.5 MB` active) is also compact, but requires `.slint` DSL codegen and offers fewer out-of-the-box developer controls (such as a syntax-highlighted JSON editor and built-in dialog/tab/sidebar primitives) than `gpui-kit`.
