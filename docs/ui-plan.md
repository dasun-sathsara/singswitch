# SingSwitch desktop UI: design plan (v3)

> **Historical.** This is the design record for the v3 shell, kept because the
> reasoning still explains the layout. It is not current: the Doctor feature
> described below was removed after the fact, and the `TODO_` placeholder scheme
> was replaced by typed readiness (`crates/singswitch-core/src/readiness.rs`). Read
> `README.md` for the shipped behaviour.

Status: revised after two adversarial reviews. v3 is a behaviour spec, not a layout sketch: every
state below names the code that produces it and what the user sees when it holds.
Mode: **Operate**. macOS primary, Windows parity.

## 0. Corrections carried from review

| Wrong in v2 | Correction | Now |
|---|---|---|
| Observed IP shown under the inspected profile | The lookup is machine-wide, has no profile identity, and keeps its last value on failure. Attributing it to one profile is a lie. | Observed IP lives in global service status (§5.1). The stage shows only what the profile's own JSON says. |
| `Configured: vless://13.215.132.32:443` | `extract_server_and_tun` takes the first non-`direct`/`block`/`dns` outbound with a `server` field. That outbound may not be the one rules select, and the scheme is not necessarily vless. | `Configured outbound` with the actual type, tag, and `host:port` read from the same object; unknown states named. |
| Two live save actions (`Save`, `Save & Reconnect`) | Both hit the service; the difference is invisible and the choice is meaningless. | One live action: `Save & Reconnect`. |
| Rail rows switch on hover | Hover-only actions are unreachable by keyboard and touch, and reserving space for them shrinks names. | The rail inspects. Switching is the stage's primary action, `Return`, and the tray. |
| Active profile pinned when scrolling | The list reorders while the user scrolls and can duplicate a row. | Stable order by name. The running profile is named in the status strip with a `Reveal` action. |
| "Last active" | `Snapshot.active` is the persisted selection, not a record of a successful connection. `connected_since` starts when the app *observes* running. | `Selected`. Uptime reads `up since SingSwitch started watching` when the app did not watch the start. |
| Rollback assumed to succeed | `recover_inner` has four outcomes: no previous profile, rollback failed, reverted but restart failed, reverted and kept stopped. | Four distinct messages, verbatim from the error's hints. |
| Setup suppressed for users who dismissed the old guide | Dismissal must never hide a failure. `needs_onboarding` already fires on a blocking Doctor check. | Dismissal hides the welcome copy only. Empty library, unfinished TODOs, permission blocks, init failure, and rollback always show. |
| `subscription URL` | Import fetches a complete sing-box JSON document over HTTP(S) (`pull_profile`). | `Import from URL`. |
| Logs 20px in §6.2, 22px in §5.3 | Inconsistency. | 20px everywhere below the hero screen. |
| Shortcuts silently renumbered | v1 had Dashboard 1, Profiles 2, Logs 3, Doctor 4, Settings 5. | Documented migration, §7. |
| Native menus unmentioned | `shell.rs` installs SingSwitch, File, Connection, Edit, Window menus with Undo/Cut/Copy/Paste. | Kept; §7.2 states what changes. |
| Windows notifications under "Menu bar (macOS only)" | `notifier.rs` posts on both platforms. | Cross-platform `Notifications` row. |

## 1. What it is

SingSwitch points a privileged TUN proxy at one of N saved configs and keeps it alive. The window is a
profile switcher with a diagnostic bench attached.

Frequency of use: once at setup; daily from the menu bar (confirm state, switch, stop); weekly from
the window (add or edit a profile, check the public IP); when it breaks (read state, read logs, run
Doctor).

## 2. Thesis

> **Inspect on the left, act in the middle, never lose the state.**

1. **Separate what is configured from what is observed.** Config facts belong to the inspected
   profile. Service health and the public IP belong to the machine. They never share a block.
2. **One service headline.** Dot plus words, one vocabulary, at every altitude. Readiness and
   diagnostics are subordinate text, never competing headlines.
3. **Elevation is spent once, usually never.** Grounds, hairlines, weight, space. No cards.
4. **The switch is always one move away.** Stage primary, `Return`, tray.

## 3. Structure

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ ○ ○ ○        (native title bar, 34px)                                         │
├───────────────┬───────────────────────────────────────────────────────────────┤
│ SingSwitch · Connections                    ● Connected: home-vless    ⟳   ⋯      │ 44 toolbar
├───────────────┼───────────────────────────────────────────────────────────────┤
│ Profiles  4   │  home-vless                              [Edit] [Save & Reconnect]│
│ ⌕ Filter      │  13.215.132.32:443 · VLESS + Reality · source saved            │
│ ───────────── │ ─────────────────────────────────────────────────────────────  │
│ ● home-vless  │  Configured outbound   vless · proxy · 13.215.132.32:443        │
│   sg-direct   │  Tunnel                utun123 · from the profile              │
│   aws-sg-ads  │  Egress                unset in this profile                   │
│   work-tokyo  │                                                                │
│   3 TODO      │  Up since SingSwitch started watching · 2h 14m   PID 49398  1 restart│
│               │                                                                │
│               │  Transport   VLESS + Reality        SNI      www.microsoft.com │
│ ───────────── │  Flow        xtls-rprx-vision       uTLS     chrome            │
│ Logs       2  │  TUN stack   gvisor · MTU 9000      Log      info              │
│ Doctor        │  Config      …/profiles/home-vless.json                        │
│ Settings      │  Source      https://sub.example.com/vless/home                │
├───────────────┴───────────────────────────────────────────────────────────────┤
│ ● Connected · home-vless   utun123 · PID 49398   103.24.9.17 Singapore 2m  ○0 │ 30
└───────────────────────────────────────────────────────────────────────────────┘
```

- **Title bar**: stock `TitleBar`, 34px, native controls and drag.
- **Toolbar** (44px): mark, screen name, then global service state and `Disconnect`/`Connect`,
  refresh, overflow. The connection control exists on every screen.
- **Rail** (240-320, default 264): inspect-only list plus utility nav.
- **Stage**: one screen.
- **Status strip** (30px, `StatusBar`): service headline, tunnel and PID, the observed IP sample,
  Doctor failure count.

## 4. State

### 4.1 Four states, one headline

| State | Source | Shown as |
|---|---|---|
| **Service** | `health.state`, `running`, `pid`, `busy_phase` | The only headline: `Connected: home-vless` / `Stopped` / `Needs attention` / `Stopping…` |
| **Owner** | `snapshot.active` | `Selected: sg-direct` under the headline, and the rail's remembered row marked `Selected` |
| **Readiness** | `Profile.placeholders` | Subordinate: `3 values still TODO` |
| **Diagnostics** | `health.restarts`, `last_exit`, `broken`, Doctor | Subordinate: `Restarted 4 times`, `Last exit 1`, `Config broken: <reason>` |

Rules:

- Nothing else is a headline. `Selected` and `3 TODO` are quiet text, not statuses competing with
  the service.
- `Connected: name` requires `running && active == name`. Running with no active profile reads
  `Connected: unknown profile`, which is what the daemon state actually says.
- Uptime: `connected_since` is set when the app observes running, so the label is
  `up since SingSwitch started watching` for any sample older than one poll. `Connected for 2h 14m` is
  only used when the app watched the transition itself.
- Restart count is a total, shown whenever non-zero, running or stopped. `Restarting repeatedly` is
  never asserted from a single sample; it requires the count to rise across two or more snapshots.
- `broken` is displayed with its exact reason from `Snapshot.broken` plus `Run Doctor`, on the
  Connections stage and in the tray. It is never summarised as "something went wrong".

### 4.2 Connect targets explicitly

`connect_active` today prefers `snapshot.active` even when that profile is not `ready()`. The
redesign makes the target explicit and never connects an unready profile:

- Toolbar `Connect` acts on the **selected profile** when it is ready. If the selected profile is
  not ready, the button reads `Connect` and is disabled with the reason in its tooltip
  (`home-vless has 3 TODO values`).
- If nothing is selected, it acts on the remembered active profile when ready, otherwise the first
  ready profile, and the toolbar label names it: `Connect sg-direct`.
- With no ready profile at all: label `Connect`, disabled, tooltip `No ready profile. Finish one
  first.`, plus `Open Connections` in the tooltip's action row.
- The tray mirrors exactly this. No silent fallback to an unready profile anywhere.
- Windows: a stopped helper is not a failure. Activation falls back to UAC elevation; copy says
  `A Windows permission prompt will appear`, never "blocked".

## 5. Screens

### 5.1 Connections (`⌘1`)

**Rail.** inspect only.

1. `Profiles` 15/700, count in mono `--muted`; `+` new, `↧` import.
2. Filter, 26px, `--surface` with hairline, `⌘F`. With no matches: `No profile matches "wifi".` +
   `Clear filter`.
3. Rows 52px, stable order by name:
   - name 15 (700 when selected), `Selected` marker on the remembered active row, `Needs setup`
     when unready;
   - line 2: `host:port` mono 12 `--muted`, or `3 values still TODO` in `--warn`, or `direct`.
   - selected: accent wash on `--surface` + 700 name. No accent bar, no hover action.
4. Foot: `Logs` (error count), `Doctor` (failed count), `Settings`.
5. Keyboard: `↑`/`↓` move selection, `Return` switches to the selected profile (if ready),
   `⌘F` filter, `Esc` clears the filter and returns focus to the list.

**Stage**

1. **Head**: name 24/700, tracking -0.02em. Meta 12.5: primary endpoint (mono) · transport ·
   source state (`source saved` / `no source`).
   Actions: `Switch` / `Reconnect` (ink fill), `Edit` (hairline), `⋯`.
2. **Provenance** (three rows, 36px, hairline-separated). Everything here comes from the profile's
   own JSON, nothing from the daemon:
   - `Configured outbound`: `<type> · <tag> · <server>:<port>`, e.g. `vless · proxy ·
     13.215.132.32:443`. If the first eligible outbound is not the one routing selects, the row
     says `first outbound in the config` in `--muted`. If the JSON can't be parsed:
     `unavailable · config could not be parsed`.
   - `Tunnel`: three distinct states: `utun123 · from the profile`; `OS default · no interface
     set`; `none · no TUN inbound in this config`.
   - `Egress`: from the profile's own `route`/`outbounds` when it sets one, else
     `unset in this profile`. Never the machine's public IP. (The observed IP is machine-wide and
     lives in the status strip.)
3. **Runtime** (only when running): uptime with its honest label, `PID`, restart total,
   `last exit`. Mono 12, labels `--muted`, values `--ink2`, 24px gaps.
4. **Config facts**, two columns at ≥1000px, one below: `Transport`, `Flow`, `SNI`, `uTLS`,
   `TUN stack`, `TUN MTU`, `Log level`, `Config` (full path, copyable, middle-truncated in display
   only), `Source` (URL + inline `Sync` when the sidecar exists). Missing values show `n/a` in `--muted`.
5. **Overflow `⋯`**: `Restart service`, `Refresh status`, `Sync from source`, `Sync all`,
   `Copy public IP`, `Copy config path`, `Reveal config`, `Delete…`.
6. **Broken config**: a row above the facts with `Snapshot.broken` verbatim and `Run Doctor`.
7. **Empty library**: setup §5.5.
8. **Snapshot failure at startup**: when `Controller::snapshot()` fails, `AppStore` currently
   stores an empty snapshot and the window looks like a fresh install. The redesign keeps
   `init_error`/snapshot error distinct and shows `SingSwitch could not read the service state` with the
   error and `Retry`, never the empty-library guide.

`Delete` states that a running profile's service is stopped first, then confirms.

### 5.2 Editor (`⌘E`)

- **Pinned toolbar**: back chevron, `Editing home-vless` 15/700, then the service headline in the
  same vocabulary (`Connected: home-vless` / `Stopped`), then `Revert`, `Save` (hairline, inactive
  profiles only), `Save & Reconnect` (ink fill, live profiles only). Never both save buttons.
  `Format` and `Validate` in `⋯`. The action row does not scroll; only the body does.
- **Body**: form or JSON, `Form | JSON` segmented in the toolbar. Invalid JSON blocks the switch to
  Form with the parse error inline.
- **Form**: 14 inputs + 6 switches (`ProfileFormModel`); groups `Server`, `Security`, `Tunnel`,
  `Logging`; 30px fields; labels above at 12 `--ink2`; identifiers mono.
- **Validation**: sing-box's message verbatim in mono, `Copy`, field highlighted. TODO tokens shown
  as mono coloured words.
- **External change**: `Changed on disk` + `Reload` / `Keep mine`. A clean draft reloads
  automatically; `Keep mine` is refused if the file's revision moved again, with the reason.
- **Draft**: `Save Draft…` in `⋯` for inactive profiles. Invalid drafts confirm with
  `Keep editing` / `Discard`.
- **Draft guard**, one pending intent, all exits: editor Back, rail selection, screen shortcuts,
  New, Import, tray navigation, tray `Quit`, `⌘Q`, and the native window close. Native close
  returns `false` immediately, then resumes the pending exit once after `Discard` or a successful
  save; the draft and focus survive a failed save. Writes are checked against the specific file's
  revision at write time, under the save lock, not the global disk revision.

### 5.3 Logs (`⌘2`)

- **Head**: `Logs` 20/700; stream path mono 12, copyable. Actions: `Clear view` (in-memory only,
  says so), `Reload`, `Copy` (filtered set), level menu.
- **Search**: `⌘F`, case-insensitive substring, `12 of 940` beside it.
- **Tabs**: `Errors` / `Access`, underline, 2px primary.
- **Follow** row 34px: switch, `Following`, `94 lines`, plus `2 warnings` when warnings exist.
- **Body**: one framed region (`--surface`, hairline, radius 8): a virtual list of 22px rows
  (time 66px `--muted`, level 30px coloured word, message truncated with ellipsis, never wrapped),
  and a **detail pane docked under it inside the same frame**, hairline-separated, separately
  scrollable, max 40% height. No shared horizontal scroll: the row shows a truncated preview and
  the detail pane carries the full text with `Copy` and `×`. Selection is tracked by record
  identity, not filtered index.
- **Empty**: `Nothing logged yet.` + `sing-box writes here once it runs.`
- **Read error**: the actual error and path from `Layout`; `Fix permissions…` only on macOS.
- **Unavailable stream** vs **permission denied** are different messages.

### 5.4 Doctor (`⌘3`)

- **Head**: `Doctor` 20/700. Summary counts the exclusive buckets: `10 passed, 1 warning`,
  `2 checks need attention`, `All 11 checks pass`. Warnings include Windows helper stopped,
  no active profile, unmanaged sing-box, unreadable logs. Right: `Run again`. Doctor also runs once
  at startup, as today.
- **Groups** mapped from check names: `Environment`, `Permissions`, `Service`, `Configuration`.
- **Rows** 44px: 16px glyph in a 20px slot, name 13/700, detail mono 12 `--muted`, status word
  right-aligned in a fixed 72px column (`OK` / `Warning` / `Failed`), `Fix` where a fix exists.
- **Five fixes** keep their current behaviour: `Create Folder`, `Open Profiles`, `Repair macOS
  sudoers` (preview + confirmation before writing), `Repair log permissions`, `Repair Windows
  helper`.
- `Open Profiles` navigates to Connections with that profile selected.
- **Hints** visible and copyable, including the Windows install command and the sudoers command.

### 5.5 Setup, a state machine

Always shown when a condition holds, regardless of `onboarding_dismissed`:

1. **Init failure.** `SingSwitch cannot reach the service`, error text, `Run Doctor`.
2. **Empty library.** three steps (managed sing-box, permissions, a profile).
3. **Profiles exist, none ready.** `Every profile still has TODO values.` + a row and `Fix` per
   incomplete profile.
4. **Permission block.** macOS sudoers missing, or Windows helper stopped but not recoverable by
   UAC: one row with the fix (preview and confirm for sudoers).
5. **Activation rolled back.** the actual outcome from `recover_inner`, one of four variants
   (§0), plus `Open Doctor` and `View logs`.

`onboarding_dismissed` suppresses only the welcome wording on a healthy install. It can never hide
a failure, and there is a `Show setup again` control in Settings that clears it.

Steps are rows with a right-aligned action slot of fixed width; only the next incomplete step is a
filled button. Copy uses the bundled-sing-box reality (`./scripts/install-macos.sh`,
`.\scripts\install.ps1`), never `brew`.

### 5.6 Settings (`⌘4`)

Rows 52px, hairlines, groups 26px apart.

- **Appearance**: `Theme`: `Light | Dark | System`.
- **Notifications** (both platforms): `Desktop notifications`; on macOS also `Show the connected
  profile name in the menu bar`.
- **Behaviour**: `Open at login` (failure surfaced inline, as today); `Refresh the public IP after
  connect`: describes the real behaviour: one lookup on launch when enabled, and after each
  switch, never on a timer.
- **System**: `Platform`, `Profiles directory`, `Active configuration`, `Error log`, `Access log`,
  `Service target` (`system/app.lexiflix.singswitch-daemon` or `sing-box (\\.\pipe\singswitch-control)`),
  `Managed sing-box`. Mono 12, copyable, full value on hover.
- **Setup**: `Show setup again` (clears `onboarding_dismissed`).
- **About**: version, licence, `Open log folder`, `Quit SingSwitch`.

**Migration.** `ui-settings.json` keeps `theme`, `launch_at_login`, `show_profile_in_menubar`,
`auto_refresh_ip`, `desktop_notifications`, window size, `onboarding_dismissed`. `sidebar_collapsed`
is dropped and stripped on the next save. Unknown fields are preserved on load.

### 5.7 Menu bar / tray

```
● Connected: home-vless
  utun123 · 103.24.9.17 · Singapore · PID 49398
Disconnect
──────────────
Switch profile
  ✓ home-vless          (Selected)
    sg-direct
    aws-sg-direct-ads-allowed
    work-tokyo  (needs setup, disabled)
    More Profiles ▸
──────────────
Restart service
Refresh public IP                       ⌘R
Sync remote profiles
Run Doctor…                             (top item when broken or failed)
──────────────
Open singswitch                              ⌘0
Logs
Settings…
──────────────
Quit singswitch
```

- Tooltip is `tray_tooltip()`'s text unchanged: `SingSwitch · Connected to home-vless`,
  `SingSwitch · <broken reason>`, `SingSwitch · <phase>`. This is separate from the menu headline.
- Menu title states carry the same words as the window, plus the observed IP and location.
- `broken` shows `Attention needed: <reason>` first, with `Run Doctor…` directly under it.
- `Needs setup` rows stay visible and disabled.
- Transitional: `Stopping…` / `Switching to sg-direct…`, actions disabled.
- Menu-bar title is the profile name, truncated to 18 characters, macOS only, and only when the
  setting is on.
- Windows: same menu through `gpui-tray`; the activation gesture and state icon are unchanged.

## 6. Design system

### 6.1 Colour

| Token | Light | Dark | Job |
|---|---|---|---|
| ground | `#F7F6F3` | `#161513` | window |
| surface | `#FFFFFF` | `#1D1C1A` | rail, framed regions |
| recessed | `#EFEEE9` | `#232220` | segmented trough |
| hair | `#E2E0D9` | `#2E2C29` | rules, borders |
| ink | `#171613` | `#EFEDE8` | primary text, filled buttons |
| ink2 | `#55524B` | `#B5B1A8` | values |
| muted | `#6E6A62` | `#9A968C` | labels |
| accent | `#4A47A3` | `#9C97F0` | selection wash, focus ring, on-switches |
| live | `#1C7A45` | `#46C97B` | running only |
| warn | `#8A5300` | `#E5A93B` | warnings only |
| danger | `#A3261D` | `#F07A6E` | failures only |

Measured, sRGB alpha composited:

| Surface | Text ink / ink2 / muted | Ring base / selected |
|---|---|---|
| Light ground | 16.7 / 7.2 / 5.0 | 7.14 / n/a |
| Light rail, 7% wash `#F2F2F9` | 16.25 / 7.00 / **4.84** | 7.72 / 6.93 |
| Dark ground | 15.6 / 8.5 / 6.2 | 7.04 / n/a |
| Dark rail, 12% wash `#2C2B34` | 11.97 / 6.55 / **4.75** | 6.57 / 5.40 |

Two rules follow from the numbers: the selection wash is used **only on rail rows** (on light
ground the same wash drops muted to 4.49), and dense secondary labels are `--ink2`, not `--muted`,
wherever the background varies.

Green means running. Switches use accent when on. Selection is wash + 700 + the `Selected` marker:
never colour alone.

### 6.2 Type

| Role | Face | Weight | Size |
|---|---|---|---|
| Screen title (Connections, Editor) | Feixen | 700 | 24 |
| Sub-screen title (Logs, Doctor, Settings) | Feixen | 700 | 20 |
| Row name, section heading, profile name | Feixen | 700 | 15 |
| Body, label, button | Feixen | 400 (700 emphasis) | 13-15 |
| Meta, hint, label | Feixen | 400 | 11.5-12.5 |
| Values, identifiers, logs, paths | Geist | 400 (700 emphasis) | 12-13 |

- Two weights only, matching the four shipped files. Never 500/600.
- Base 15; controls and dense lists 13; values in mono 12.
- Tabular numerals on every aligned mono column.
- Tracking -0.02em on titles only.
- Embedded via `text_system().add_fonts()` in `fonts.rs` before the first window; theme names
  `Feixen Standard` / `Geist Standard`. gpui resolves one family and then its own fallback stack
  (Helvetica / Segoe UI), so no CSS-style chain is used.

### 6.3 Space, shape, elevation

4px base (4/8/12/16/24/32); stage padding 24 × 20; rail padding 12; row insets 10. Radii 6 controls,
8 rows and framed regions, 10 dialogs. One hairline weight. No app-surface shadows; menus and
dialogs keep native shadow. No nested borders.

### 6.4 Motion

160ms `cubic-bezier(0.2,0,0,1)` on selection, hover, view change. Nothing on data refresh. The
running dot pulses only while a transition is in flight. Reduced motion: instant, static.

### 6.5 Control states

Default, hover, focus-visible (2px accent ring, 2px offset), active, disabled (40% opacity, no
hover), busy (spinner in the icon slot, label kept, width stable). Errors inline; dialogs only for
destructive and privileged actions.

## 7. Interaction

### 7.1 Keys

| Intent | macOS | Windows |
|---|---|---|
| Connections / Logs / Doctor / Settings | `⌘1` / `⌘2` / `⌘3` / `⌘4` | `Ctrl+1…4` |
| Settings (unchanged) | `⌘,` | `Ctrl+,` |
| Filter | `⌘F` | `Ctrl+F` |
| Switch selected (rail focused) | `Return` | `Enter` |
| Toggle service | `⇧⌘K` | `Ctrl+Shift+K` |
| Edit selected | `⌘E` | `Ctrl+E` |
| Save / save & reconnect | `⌘S` / `⇧⌘S` | `Ctrl+S` / `Ctrl+Shift+S` |
| New / import | `⌘N` / `⌘I` | `Ctrl+N` / `Ctrl+I` |
| Refresh IP | `⌘R` | `Ctrl+R` |
| Open window from tray | `⌘0` | none |
| Close window | `⌘W` | `Ctrl+W` |
| Quit | `⌘Q` | `Ctrl+Q` |

**Shortcut migration.** Current bindings are Dashboard 1, Profiles 2, Logs 3, Doctor 4, Settings 5.
Dashboard and Profiles merge, so v3 uses Connections 1, Logs 2, Doctor 3, Settings 4, and `⌘,`
keeps working for Settings. `⌘1` still lands on the profile list, which is where Dashboard's switch
lives; Logs and Doctor each move one keystroke left. This is stated in the changelog.

`Return` acts only when the rail owns focus. Inputs and dialogs keep their own defaults, so typing
cannot connect.

### 7.2 Native menus

`shell.rs` keeps singswitch, File, Connection, Edit, Window, with Undo/Redo/Cut/Copy/Paste/Select All.
Changes: File gains `New Profile…`, `Import from URL…`; Connection gains the same state-aware
`Connect`/`Disconnect` label as the toolbar; the removed Dashboard entry becomes `Connections`.
`Edit` keeps the standard responder actions; `Window` keeps minimise/zoom/bring-all-to-front.

### 7.3 Window lifecycle

One window. Closing hides it and drops the Dock icon on macOS (`on_window_should_close` returns
`true`); tray or Dock reopens it; `⌘Q` and the tray quit both pass the draft guard.
`--background` / `--minimized` still start without a window, and a missing tray still falls back to
a plain window (logged as today). Window size is restored from settings.

## 8. Feedback

One persistent, dismissible message region under the toolbar, visible on every screen, carrying
command outcomes: activation, stop, restart, per-profile sync and reconnect, bulk sync
(`3 synced, 1 failed: <name>: <reason>`), import, save, delete, privilege repair, login toggle,
clipboard copy (`Public IP copied`), and unexpected disconnects. Errors stay until dismissed;
successes auto-clear after 6s. `busy_phase` text appears in the toolbar headline, not here.

IP refresh keeps its loading guard (no duplicate lookups) and shows `Checking…`; sync keeps its
no-remote guard (`No source URL saved`) and the IP error detail is shown in the strip, not swallowed.

## 9. Alternates rejected

Five equal tabs (the product has one job). A live path diagram (reads as a stepper and the data to
populate it honestly does not exist). Cards (nine boxes competing). Brand-coloured primary (reads as
consumer VPN). A per-row switch on hover (unreachable by keyboard and touch).

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Proprietary Feixen cuts now in a GitHub repo | Keep the repo private or drop `assets/fonts/` before publishing; the app degrades to the platform face because `add_fonts` failure is logged, not fatal. Decide before any public release. |
| 30 profiles at 620px | 52px rows, stable order, no pinning; the running profile is named in the status strip. Verified at 960×620. |
| Long names, long Windows paths | Name ellipsises at the row's right edge, full name on the row's `Selected` line and in the stage head; paths middle-truncate with full value on hover and copy. |
| Crash loop reads as Running | `Restarted N times` + `last exit`, both visible while running. |
| Crash-loop assertion from one sample | `Restarting repeatedly` requires a rising count across samples. |
| Sidecar has no timestamp | `source saved`, not a sync age. |
| Docked log detail at 620px | Max 40% of the frame, separately scrollable, selection by record identity. |
| Draft guard across ten exits | One pending intent; native close returns `false`; resume once; file-specific revision under the save lock. |
| Async save races | Compare the specific file's revision at write time; global `disk_revision` is not used for this. |

## 11. Acceptance checklist (960 × 620, both platforms)

1. 30 profiles, active near the end: the running profile is named in the strip, list never reorders,
   `Reveal` scrolls to it.
2. A 48-character profile name: no overlap with actions, ellipsis in the rail, full name in the head.
3. A 96-character Windows config path: middle-truncated, hover shows all, copy works.
4. Dirty editor, then rail click, `⌘1`, `⌘N`, tray switch, `⌘Q`, window close: each prompts once,
   `Keep editing` preserves text and focus, `Discard` resumes the original intent exactly once.
5. Save fails: draft and focus survive, error visible, no navigation.
6. File changed on disk after load, then Save: refused with the reason, nothing overwritten.
7. Active profile unready: `Connect` disabled with a reason, tray identical, no silent activation.
8. Dismissed onboarding, empty library: setup condition 2 still shown.
9. Service fails mid-session: reason verbatim, `Run Doctor` present, tray shows the same reason.
10. Rollback variants: each of the four shows its own message.
11. Long log line: preview truncates, detail pane scrolls separately, copy returns the full line.
12. Log read error vs empty log: different copy, correct path.
13. Doctor with `ok + warning`: counted as a warning, never as a pass.
14. Windows: helper stopped, activation proceeds through UAC with correct copy.
15. Dark mode: every row, selected row, and focus ring at or above 4.5:1 (measured values in §6.1).

## 12. Capability map

| Capability (current home) | v3 home | Acceptance |
|---|---|---|
| Switch profile (Dashboard row, tray) | Stage primary, `Return`, tray | All three, state follows |
| Connect / Disconnect (title bar, menus, tray) | Toolbar every screen, Connection menu, tray, `⇧⌘K` | Present on Logs/Doctor/Settings |
| Restart service | `⋯`, tray | Works from Logs |
| Refresh status | `⋯`, toolbar ⟳ | Snapshot updates in place |
| Refresh public IP (loading guard) | Strip `Check`, `⋯`, tray, `⌘R` | No duplicate lookups |
| Copy public IP | `⋯` | Confirmation shown |
| Sync one (no-remote guard) | `⋯` `Sync from source` | Guard message when absent |
| Sync all + per-profile results | `⋯`, tray | `3 synced, 1 failed: …` |
| New / import URL (`⌘N`, `⌘I`) | Rail, setup, menus | JSON validation, focused input |
| Edit form + JSON, format, validate | Editor | 14 inputs + 6 switches reachable |
| Save, save & reconnect, save draft, invalid draft confirm | Editor | One live action |
| Revert, external change reload | Editor | Nothing lost without a prompt |
| Delete with running warning | `⋯`, dialog | Warns before stopping |
| Logs: tabs, follow, clear, reload, levels | Logs | All present |
| Logs: search (case-insensitive), copy line, copy filtered, warnings count | Logs | Counts and filters correct |
| Logs: fix permissions, read errors | Logs | macOS only for the fix |
| Doctor: run at startup, run again, 5 fixes, hints copy, `Open Profiles` | Doctor | Each fix reaches its destination |
| Settings: theme, login (with failure inline), menubar name, notifications (both platforms), auto IP, paths incl. service target, show setup again | Settings | Every row does something |
| Tray: status, tooltip, profiles, tools, open, logs, settings, quit, busy text | Tray | Parity + observed IP |
| Native menus, Undo/Redo/Cut/Copy/Paste | Menus | Unchanged behaviour |
| `--background` / `--minimized`, no-tray fallback, window size restore | Startup | Verified on both platforms |
| macOS launch-at-login agent, Dock hide/return | Settings, lifecycle | Toggle writes the plist |
| Windows UAC fallback, helper repair | Activation, Doctor | Never says "blocked" when UAC works |
| `--json`, `--plain`, CLI | CLI crate | `make check` |

All 48 public `AppStore` methods (`matches` excluded) map to a home above;
`set_sidebar_collapsed` is removed with the sidebar.
