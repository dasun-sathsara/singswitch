use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AsyncApp, Context, SharedString, Task, WeakEntity};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use singswitch_core::controller::{Controller, Snapshot, StatusView};
use singswitch_core::error::{ExitCode, Result as CoreResult, SingSwitchError};
use singswitch_core::form_model::ProfileFormModel;
use singswitch_core::ip::{IpInfo, fetch_ip_info};
use singswitch_core::layout::Layout;
use singswitch_core::logs::{LogLevel, read_appended_lines, read_tail_lines};
use singswitch_core::service::{Health, RunState};

use crate::actions::NavTab;
use crate::icons::TrayVisualState;
use crate::shell::{is_launch_at_login_enabled, set_launch_at_login};
use crate::theme::{
    MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, ThemePreference, UiSettings, WINDOW_SIZE_EPSILON,
    apply_theme,
};

const MAX_LOG_ENTRIES: usize = 5_000;
const INITIAL_LOG_TAIL: usize = 300;
const ACTIVE_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);
// The file watcher already reacts to config writes, so the poll behind a hidden
// window only guards against a daemon that died unnoticed.
const BACKGROUND_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(180);
const SNAPSHOT_TICK_INTERVAL: Duration = Duration::from_secs(1);
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(150);
/// How long an administrator password dialog may stay open.
///
/// The 30s command timeout exists for `launchctl`-style calls; this one waits on
/// a human, and killing it mid-prompt leaves the user typing into nothing.
const ADMIN_PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// Where a piece of feedback belongs on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeOrigin {
    /// The Connections page.
    Stage,
    /// The toolbar connection control.
    Toolbar,
    /// The editor's pinned action bar.
    Editor,
    /// The log screen controls.
    Logs,
    /// The settings rows.
    Settings,
}

/// Inline feedback attached to one origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub origin: NoticeOrigin,
    pub tone: BannerTone,
    pub message: String,
}

/// The last latency probe for one profile, as the profile list shows it.
///
/// Pinned to the disk revision it was measured against: a config rewritten
/// after the probe no longer describes what was measured, so the column falls
/// back to "—" instead of showing a number for a different server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyReading {
    pub since_disk_revision: u64,
    pub result: Result<Duration, String>,
}

/// Tone of a notice strip. Only `Warning` and `Error` are raised in practice:
/// routine success and info are visible in the state they change, so they do
/// not get a strip (AGENTS.md, "UI feedback rules").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerTone {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LogStreamKind {
    #[default]
    Error,
    Access,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LogSeverityFilter {
    #[default]
    All,
    Error,
    Warn,
    Info,
}

impl LogSeverityFilter {
    pub fn matches(self, level: LogLevel) -> bool {
        match self {
            Self::All => true,
            Self::Error => level == LogLevel::Error,
            Self::Warn => matches!(level, LogLevel::Error | LogLevel::Warn),
            // "and up" must include the levels above Info, or the menu item
            // hides the very lines it claims to show.
            Self::Info => matches!(level, LogLevel::Error | LogLevel::Warn | LogLevel::Info),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorValidationState {
    Validating,
    Valid,
    Invalid(String),
}

#[derive(Clone, Debug)]
pub struct LogRecord {
    /// Stable identity for selection. Two identical lines are two records, and
    /// a filtered index renumbers while the buffer drains.
    pub id: u64,
    pub line: SharedString,
    pub level: LogLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEventKind {
    Config,
    Logs,
    Both,
}

pub struct AppStore {
    pub ctrl: Option<Arc<Controller>>,
    pub init_error: Option<String>,
    pub snapshot: Snapshot,
    pub settings: UiSettings,
    /// Geometry last written to disk, so a close can tell a real change from the
    /// live copy the bounds observer keeps current.
    persisted_window_width: f32,
    persisted_window_height: f32,
    pub window_open: bool,
    /// Whether a tray/menu-bar item exists to restore the window from.
    ///
    /// Closing the window with neither a tray nor a Dock icon leaves nothing to
    /// click, so the close path consults this before hiding.
    pub tray_available: bool,
    /// Error/Warn counts for the error stream, kept incrementally.
    error_log_counts: singswitch_core::logs::LogCounts,
    /// Stand-in layout for the init-failed state.
    ///
    /// Building `Layout::darwin()` walks PATH and stats binaries, and the UI
    /// deliberately keeps rendering that state, so it is resolved once.
    fallback_layout: Layout,
    pub log_revision: u64,
    pub disk_revision: u64,
    pub editor_json_mode: bool,
    pub connected_since: Option<Instant>,
    /// Message from the last failed probe, cleared by the next success.
    snapshot_error: Option<String>,
    /// Whether any probe has answered, successfully or not.
    pub probed_once: bool,
    /// A selection chosen between a snapshot and its poll, which that poll must
    /// not overwrite. Holds the name and how many stale snapshots it may skip;
    /// bounded so a profile that never appears cannot freeze the selection.
    pinned_selection: Option<(String, u8)>,
    pub ip_info: Option<IpInfo>,
    pub ip_loading: bool,
    pub ip_error: Option<String>,
    /// Bumped on every request so a reply from an older egress is discarded.
    ip_generation: u64,
    /// A refresh was asked for while one was already running.
    ip_refresh_pending: bool,
    /// Dispatch counter for live validation; a late reply for older content is
    /// dropped so "Configuration valid" can never describe text it never saw.
    validation_generation: u64,
    pub busy_phase: Option<String>,
    /// The profile the in-flight operation targets, if it targets one. The
    /// per-row Connect button spins only for this row, so one operation does
    /// not start a repeating animation on every row in the list.
    pub busy_target: Option<String>,
    /// An automatic sync is running.
    ///
    /// Separate from `busy_phase` so the UI does not report the app as busy on
    /// a schedule nobody asked for, while still keeping the next tick from
    /// starting a second sync on top of the first.
    auto_sync_in_flight: bool,
    /// A scheduled sync is running (see `auto_sync_in_flight`).
    ///
    /// The interval is measured on the wall clock, not `Instant`: a closed
    /// laptop lid freezes the monotonic clock, which would stretch "every
    /// hour" by the length of every nap. Wall clock keeps the cadence honest
    /// across suspend and relaunch alike.
    last_auto_sync_at_wall: Option<u64>,
    /// Feedback scoped to the control or screen that produced it. The Toolbar
    /// origin is the one strip that spans the window.
    pub notice: Option<Notice>,
    /// Last latency probe per profile, rendered in the profile list's LATENCY
    /// column. Keyed by name; a profile's entry is dropped with the profile.
    pub latency_results: std::collections::HashMap<String, LatencyReading>,
    /// Profiles with a probe currently running, so a second click on the same
    /// row cannot stack a second `tools fetch`.
    latency_in_flight: HashSet<String>,
    pub editor_validation: Option<(String, EditorValidationState)>,
    pub selected_tab: NavTab,
    pub selected_profile: Option<String>,
    /// True while the Connections stage is showing the editor for the selected profile.
    pub editing: bool,
    /// Unsaved editor buffers, keyed by profile name.
    ///
    /// Kept here rather than in the editor panel because the panel dies with the
    /// window: closing mid-edit used to discard the text with no way back.
    pub drafts: std::collections::HashMap<String, String>,
    pub log_stream: LogStreamKind,
    pub log_follow: bool,
    pub log_severity: LogSeverityFilter,
    pub log_search_query: String,
    pub profile_search_query: String,
    pub error_logs: Vec<LogRecord>,
    pub access_logs: Vec<LogRecord>,
    pub error_log_err: Option<String>,
    pub access_log_err: Option<String>,
    error_log_offset: u64,
    access_log_offset: u64,
    error_log_id: Option<singswitch_core::logs::LogFileId>,
    /// Bumped by `reload_logs` so an in-flight poll cannot append a delta
    /// computed against the offsets that reload just cleared.
    log_generation: u64,
    access_log_id: Option<singswitch_core::logs::LogFileId>,
    was_running_expected: bool,
    /// A stop this app asked for is in progress or awaiting confirmation.
    ///
    /// The snapshot-derived `was_running_expected` cannot express intent: a
    /// poll that started before the click can land during the stop and re-arm
    /// it, which turns a deliberate Disconnect into a false "stopped
    /// unexpectedly" alert.
    stop_requested: bool,
    log_poll_in_flight: bool,
    snapshot_poll_in_flight: bool,
    /// A refresh was requested while a poll was running.
    ///
    /// `cx.spawn` cancellation stops the await, not the blocking probe already
    /// handed to the background executor, so letting forced refreshes through
    /// stacked concurrent `launchctl` probes and full profile scans.
    snapshot_poll_pending: bool,
    /// Whether the queued refresh needs the disk revision bumped too.
    snapshot_poll_force_pending: bool,
    _watcher: Option<RecommendedWatcher>,
    watcher_task: Option<Task<()>>,
    /// Serialized writer for UI settings and the launch-at-login toggle.
    ///
    /// Both are fsync'd writes; doing them inside the click handler stalled
    /// the UI thread (AGENTS.md: blocked operations go through the async
    /// layer). A single channel keeps a strict order: two quick toggles
    /// persist in the order they were clicked, never interleaved.
    settings_save_tx: Option<smol::channel::Sender<SettingsSaveJob>>,
    _daemon_tasks: Vec<Task<()>>,
    snapshot_task: Option<Task<()>>,
    log_poll_task: Option<Task<()>>,
    ip_task: Option<Task<()>>,
    validation_task: Option<Task<()>>,
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        health: Health::unknown(RunState::Stopped),
        active: None,
        running: false,
        profiles: Vec::new(),
        broken: None,
        status_view: StatusView {
            state: RunState::Stopped,
            profile: None,
            running: false,
            tun: None,
            server: None,
            pid: None,
            missing: Vec::new(),
            broken: None,
        },
    }
}

impl AppStore {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut settings = UiSettings::load();
        settings.launch_at_login = is_launch_at_login_enabled();

        let (ctrl, init_error, snapshot) = match Layout::detect().map(|layout| {
            Controller::for_layout_with_notifier(
                layout,
                None,
                Box::new(crate::notifier::NativeNotifier),
            )
        }) {
            Ok(c) => {
                // Desktop notifications are failure-only (AGENTS.md), so the
                // toggle stays on.
                c.set_notifications_enabled(true);
                // No synchronous snapshot here: it shelled out to
                // `launchctl print` (30s deadline) and re-parsed every profile
                // file before the first frame. The first paint shows an unknown
                // state for as long as the async probe takes.
                (Some(Arc::new(c)), None, empty_snapshot())
            }
            Err(err) => (None, Some(err.message.clone()), empty_snapshot()),
        };

        let selected_profile = snapshot
            .active
            .clone()
            .or_else(|| snapshot.profiles.first().map(|p| p.name.clone()));

        let mut store = Self {
            ctrl,
            init_error: init_error.clone(),
            snapshot,
            persisted_window_width: settings.window_width,
            persisted_window_height: settings.window_height,
            settings,
            window_open: false,
            tray_available: false,
            error_log_counts: singswitch_core::logs::LogCounts::default(),
            fallback_layout: if cfg!(windows) {
                Layout::windows()
            } else {
                Layout::darwin()
            },
            log_revision: 0,
            disk_revision: 0,
            editor_json_mode: false,
            // The constructor's snapshot is always the empty placeholder, so
            // there is no launch-time baseline to record. The first probe sets
            // both of these in `apply_snapshot`.
            connected_since: None,
            snapshot_error: None,
            probed_once: false,
            pinned_selection: None,
            ip_info: None,
            ip_loading: false,
            ip_error: None,
            ip_generation: 0,
            ip_refresh_pending: false,
            validation_generation: 0,
            busy_phase: None,
            busy_target: None,
            auto_sync_in_flight: false,
            last_auto_sync_at_wall: None,
            notice: init_error.map(|m| Notice {
                origin: NoticeOrigin::Toolbar,
                tone: BannerTone::Warning,
                message: m,
            }),
            latency_results: std::collections::HashMap::new(),
            latency_in_flight: HashSet::new(),
            editor_validation: None,
            selected_tab: NavTab::Profiles,
            editing: false,
            selected_profile,
            drafts: std::collections::HashMap::new(),
            log_stream: LogStreamKind::Error,
            log_follow: true,
            log_severity: LogSeverityFilter::All,
            log_search_query: String::new(),
            profile_search_query: String::new(),
            error_logs: Vec::new(),
            access_logs: Vec::new(),
            error_log_err: None,
            access_log_err: None,
            error_log_offset: 0,
            access_log_offset: 0,
            log_generation: 0,
            error_log_id: None,
            access_log_id: None,
            was_running_expected: false,
            stop_requested: false,
            log_poll_in_flight: false,
            snapshot_poll_in_flight: false,
            snapshot_poll_pending: false,
            snapshot_poll_force_pending: false,
            _watcher: None,
            watcher_task: None,
            settings_save_tx: None,
            _daemon_tasks: Vec::new(),
            snapshot_task: None,
            log_poll_task: None,
            ip_task: None,
            validation_task: None,
        };

        store.reload_logs();
        store.restart_file_watcher(cx);
        store.start_background_pollers(cx);
        // The poller's first tick waits a full interval, so ask now. This runs
        // on the background executor and cannot delay the first frame.
        store.refresh_snapshot(false, cx);
        store
    }

    /// Whether the state on screen is still the placeholder.
    ///
    /// The store starts empty and fills from the background probe, so without
    /// this the first frame of every launch claimed "Disconnected" and showed a
    /// first-run card to people who already had profiles.
    /// Whether a latency probe is running for one profile, for the render side.
    pub fn latency_probing(&self, name: &str) -> bool {
        self.latency_in_flight.contains(name)
    }

    pub fn is_probing(&self) -> bool {
        // First answer only. Keying this on `snapshot_poll_in_flight` flipped the
        // first-run card and tray tooltip on every 5s poll for a user who simply
        // has no profiles yet. With no controller at all there is nothing to
        // wait for, so the init error speaks for itself.
        self.ctrl.is_some() && !self.probed_once
    }

    /// Drop the banner a failed probe left behind, once the probe succeeds.
    fn clear_probe_error_notice(&mut self) {
        if self
            .notice
            .as_ref()
            .is_some_and(|n| n.message.starts_with("Could not read the service state:"))
        {
            self.notice = None;
        }
    }

    /// The one-line state name every surface shows.
    ///
    /// The toolbar, the connection card and the tray menu each had their own
    /// copy of this mapping and had already drifted ("Service issue" vs
    /// "Connection issue"), and only one of them accounted for the pre-probe
    /// frame. The short form drops the profile name; the tray builds its own
    /// longer sentence from the same state.
    pub fn status_label(&self) -> &'static str {
        if self.is_probing() {
            return "Checking…";
        }
        // `tray_visual_state` already folds busy into Transitioning.
        match self.tray_visual_state() {
            TrayVisualState::Transitioning => "Working…",
            TrayVisualState::Broken => "Connection issue",
            TrayVisualState::Connected => "Connected",
            TrayVisualState::Disconnected => "Disconnected",
        }
    }

    /// Whether an action should be refused because something else is running.
    ///
    /// `init_error` is deliberately not part of this: it never clears in the
    /// real lifecycle, so including it disabled every mutating control for
    /// the whole session after one failed `Layout::detect`. A detached store
    /// is already refused per-action by `require_ctrl`, which posts why.
    pub fn is_busy(&self) -> bool {
        self.busy_phase.is_some()
    }

    pub fn tray_visual_state(&self) -> TrayVisualState {
        if self.busy_phase.is_some() {
            TrayVisualState::Transitioning
        } else if self.init_error.is_some()
            || self.snapshot.broken.is_some()
            || self.snapshot.health.state == RunState::Error
        {
            TrayVisualState::Broken
        } else if self.snapshot.running {
            TrayVisualState::Connected
        } else {
            TrayVisualState::Disconnected
        }
    }

    /// Tooltip text: state first, then the profile, capped so an unusually
    /// long name cannot produce an unreadable tooltip.
    pub fn tray_tooltip(&self) -> String {
        const CAP: usize = 120;
        let text = if self.is_probing() {
            // The icon is installed before the first probe answers; naming a
            // state here would be a guess, not an observation.
            "SingSwitch".to_string()
        } else if let Some(phase) = &self.busy_phase {
            format!("SingSwitch · {phase}")
        } else if let Some(issue) = self.init_error.as_ref().or(self.snapshot.broken.as_ref()) {
            format!("SingSwitch · {issue}")
        } else if self.snapshot.health.state == RunState::Error {
            "SingSwitch · Service failed · Open Logs".to_string()
        } else if !self.snapshot.running {
            let needs_setup = self
                .selected_profile
                .as_ref()
                .and_then(|n| self.snapshot.profiles.iter().find(|p| &p.name == n))
                .is_some_and(|p| !p.ready());
            // Same source the connection card uses, so the two cannot name
            // different profiles.
            match (&self.selected_profile, needs_setup) {
                (Some(name), true) => format!("SingSwitch · {name} needs setup"),
                (Some(name), false) => format!("SingSwitch · Disconnected · {name} selected"),
                (None, _) => "SingSwitch · Disconnected".to_string(),
            }
        } else {
            match &self.snapshot.active {
                Some(name) => format!("SingSwitch · Connected to {name}"),
                None => "SingSwitch · Connected".to_string(),
            }
        };
        if text.chars().count() > CAP {
            text.chars().take(CAP - 1).collect::<String>() + "…"
        } else {
            text
        }
    }

    /// Borrowed, not cloned: callers were copying every path in the layout on
    /// each call, and the settings and logs views call it on every render.
    pub fn layout(&self) -> &Layout {
        match self.ctrl.as_ref() {
            Some(c) => &c.layout,
            None => &self.fallback_layout,
        }
    }

    /// Preview mode only: the nodes scene renders the strip against a config
    /// under /tmp, but nothing outside preview ever constructs a store without
    /// a controller, so the override lives in the same place the preview state
    /// is seeded instead of reaching real layout code.
    pub fn preview_point_active_config(&mut self, path: std::path::PathBuf) {
        self.fallback_layout.active_config_path = path;
    }

    /// Error and warning counts for the navigation badge.
    ///
    /// Read on every shell render; it used to rescan up to 5000 records each
    /// time. The counts are refreshed when the buffer changes instead, so a
    /// repaint for any other reason costs nothing.
    pub fn log_counts(&self) -> (usize, usize) {
        (
            self.error_log_counts.errors(),
            self.error_log_counts.warnings(),
        )
    }

    /// Bring the error/warn counts up to date with the buffer. Incremental when
    /// the records it already counted are still a prefix of the buffer.
    fn sync_log_counts(&mut self) {
        self.error_log_counts
            .sync(&self.error_logs, |r| r.id, |r| r.level);
    }

    pub fn set_window_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let was_open = self.window_open;
        self.window_open = open;
        if open && !was_open {
            self.refresh_snapshot(false, cx);
            if self.should_poll_logs() {
                self.poll_logs_async(cx);
            }
            // Only once somebody is looking: launching into the tray no longer
            // spends an outbound request on a pane that is never drawn.
            if self.settings.auto_refresh_ip && self.ip_info.is_none() && !self.ip_loading {
                self.refresh_ip(cx);
            }
        }
    }

    /// Record the window size. `persist` is false while a resize is in flight:
    /// AppKit emits continuous bounds events, and a synchronous write per step
    /// put up to ~100 blocking disk writes per second on the UI thread.
    pub fn update_window_size(&mut self, width: f32, height: f32, persist: bool) {
        let w = width.max(MIN_WINDOW_WIDTH);
        let h = height.max(MIN_WINDOW_HEIGHT);
        let moved = (w - self.settings.window_width).abs() >= WINDOW_SIZE_EPSILON
            || (h - self.settings.window_height).abs() >= WINDOW_SIZE_EPSILON;
        if moved {
            self.settings.window_width = w;
            self.settings.window_height = h;
        }
        // Compare against what was last written, not against the live value: the
        // bounds observer mirrors every resize into `settings`, so a close that
        // checks `moved` would find the size already equal and never write it.
        let unwritten = (self.settings.window_width - self.persisted_window_width).abs()
            >= WINDOW_SIZE_EPSILON
            || (self.settings.window_height - self.persisted_window_height).abs()
                >= WINDOW_SIZE_EPSILON;
        if persist && unwritten {
            // Window geometry is not a user action; a failure here should
            // not raise a notice mid-interaction.
            if self.settings.save().is_ok() {
                self.persisted_window_width = self.settings.window_width;
                self.persisted_window_height = self.settings.window_height;
            }
        }
    }

    fn require_ctrl(&mut self, cx: &mut Context<Self>) -> Option<Arc<Controller>> {
        if let Some(ctrl) = &self.ctrl {
            return Some(ctrl.clone());
        }
        self.set_notice(
            NoticeOrigin::Toolbar,
            BannerTone::Warning,
            self.init_error
                .clone()
                .unwrap_or_else(|| "Unsupported operating system".to_string()),
            cx,
        );
        None
    }

    pub fn set_tab(&mut self, tab: NavTab, cx: &mut Context<Self>) {
        if self.selected_tab != tab {
            self.selected_tab = tab;
            if self.should_poll_logs() {
                self.poll_logs_async(cx);
            }
            cx.notify();
        }
    }

    /// Set the log search filter. A setter rather than a raw field write from
    /// the view: the notify travels with the mutation, and a future side
    /// effect (debounce, hint count) has one place to hook.
    pub fn set_log_search(&mut self, query: String, cx: &mut Context<Self>) {
        if self.log_search_query != query {
            self.log_search_query = query;
            cx.notify();
        }
    }

    /// Set the severity filter; same rationale as `set_log_search`.
    pub fn set_log_severity(&mut self, filter: LogSeverityFilter, cx: &mut Context<Self>) {
        if self.log_severity != filter {
            self.log_severity = filter;
            cx.notify();
        }
    }

    /// Set the profiles-stage search; same rationale as `set_log_search`.
    pub fn set_profile_search(&mut self, query: String, cx: &mut Context<Self>) {
        if self.profile_search_query != query {
            self.profile_search_query = query;
            cx.notify();
        }
    }

    pub fn set_log_stream(&mut self, stream: LogStreamKind, cx: &mut Context<Self>) {
        if self.log_stream != stream {
            self.log_stream = stream;
            if self.should_poll_logs() {
                self.poll_logs_async(cx);
            }
            cx.notify();
        }
    }

    pub fn set_log_follow(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.log_follow = follow;
        if self.should_poll_logs() {
            self.poll_logs_async(cx);
        }
        cx.notify();
    }

    /// Drop the search and level filters, leaving the buffered lines alone.
    pub fn reset_log_filters(&mut self, cx: &mut Context<Self>) {
        self.log_search_query.clear();
        self.log_severity = LogSeverityFilter::All;
        cx.notify();
    }

    pub fn clear_logs(&mut self, cx: &mut Context<Self>) {
        match self.log_stream {
            LogStreamKind::Error => self.error_logs.clear(),
            LogStreamKind::Access => self.access_logs.clear(),
        }
        self.sync_log_counts();
        self.log_revision = self.log_revision.wrapping_add(1);
        cx.notify();
    }

    pub fn select_profile(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        // Do not drop out of the editor on a selection change. Changing which
        // profile you are looking at is not the same as leaving the editor, and
        // unmounting it mid-edit made a rail click destructive.
        self.selected_profile = name;
        // A validation reply for the profile we just left must not repaint the
        // one we are looking at.
        self.validation_generation = self.validation_generation.wrapping_add(1);
        self.editor_validation = None;
        cx.notify();
    }

    /// Open the editor for the selected profile.
    pub fn begin_edit(&mut self, cx: &mut Context<Self>) {
        if self.selected_profile.is_some() {
            self.editing = true;
            self.selected_tab = NavTab::Profiles;
            self.editor_validation = None;
            cx.notify();
        }
    }

    /// Leave the editor and return to the Connections stage.
    pub fn end_edit(&mut self, cx: &mut Context<Self>) {
        self.validation_generation = self.validation_generation.wrapping_add(1);
        self.editing = false;
        self.editor_validation = None;
        cx.notify();
    }

    pub fn clear_editor_validation(&mut self, profile: &str, cx: &mut Context<Self>) {
        self.validation_generation = self.validation_generation.wrapping_add(1);
        if self
            .editor_validation
            .as_ref()
            .is_some_and(|(p, _)| p == profile)
        {
            self.editor_validation = None;
            cx.notify();
        }
    }

    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        cx.notify();
    }

    /// Drop the current notice only if it belongs to `origin`.
    fn clear_notice_for(&mut self, origin: NoticeOrigin) {
        if self.notice.as_ref().is_some_and(|n| n.origin == origin) {
            self.notice = None;
        }
    }

    /// Record feedback for a specific origin. Callers name where the message
    /// belongs so it renders beside that control, not at the top of the window.
    pub fn set_notice(
        &mut self,
        origin: NoticeOrigin,
        tone: BannerTone,
        msg: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(Notice {
            origin,
            tone,
            message: msg.into(),
        });
        cx.notify();
    }

    /// The notice for one origin, if it is the current one.
    pub fn notice_for(&self, origin: NoticeOrigin) -> Option<&Notice> {
        self.notice.as_ref().filter(|n| n.origin == origin)
    }

    /// Persist the current settings and report a write failure.
    ///
    /// A read-only config directory used to swallow every toggle silently: the
    /// switch stayed flipped while nothing was written, and the next launch
    /// reverted it. The in-memory value is kept so the session behaves as the
    /// user asked, but the failure is stated next to the setting.
    /// Queue the current settings for a background save.
    ///
    /// The save fsyncs (`write_file_atomic`); running it on the UI thread
    /// stalled the click that flipped a setting. The worker applies jobs in
    /// order, so the last state always wins.
    fn persist_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let Ok(()) = self.settings_save_sender(cx).try_send(settings) else {
            // The worker is gone only if the store is being dropped; a lost
            // settings write then no longer matters to the session.
            return;
        };
    }

    /// Lazily spawn the one settings-save worker for this store.
    fn settings_save_sender(
        &mut self,
        cx: &mut Context<Self>,
    ) -> smol::channel::Sender<SettingsSaveJob> {
        if let Some(tx) = &self.settings_save_tx {
            return tx.clone();
        }
        let (tx, rx) = smol::channel::unbounded::<SettingsSaveJob>();
        self.settings_save_tx = Some(tx.clone());
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(settings) = rx.recv().await {
                let result = cx
                    .background_executor()
                    .spawn(async move { settings.save() })
                    .await;
                if let Err(err) = result {
                    let text = format!("Could not save settings: {err}");
                    let _ = this.update(cx, |store, cx| {
                        store.set_notice(NoticeOrigin::Settings, BannerTone::Error, text, cx);
                    });
                }
            }
        })
        .detach();
        tx
    }

    pub fn set_theme_preference(&mut self, pref: ThemePreference, cx: &mut Context<Self>) {
        self.settings.theme = pref;
        self.persist_settings(cx);
        apply_theme(pref, None, cx);
        cx.notify();
    }

    pub fn set_auto_refresh_ip(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.auto_refresh_ip = enabled;
        self.persist_settings(cx);
        cx.notify();
    }

    /// Set the automatic sync interval, clamped to the supported range.
    ///
    /// `minutes` arrives from a menu of fixed choices, but it is clamped here
    /// as well: the settings file is hand-editable, and the setter is the one
    /// place every path converges.
    pub fn set_auto_sync_interval_minutes(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.settings.auto_sync_interval_minutes = minutes.clamp(
            crate::theme::MIN_AUTO_SYNC_INTERVAL_MINUTES,
            crate::theme::MAX_AUTO_SYNC_INTERVAL_MINUTES,
        );
        // A new schedule starts counting now, not from whenever the last
        // cycle ran under the old interval.
        self.last_auto_sync_at_wall = unix_now();
        self.persist_settings(cx);
        cx.notify();
    }

    pub fn toggle_launch_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        // The toggle writes a LaunchAgent plist (create_dir_all + write) and
        // then persists settings (fsync); both belong off the UI thread. The
        // switch shows the requested state at once; a failure reverts it from
        // the background task's completion.
        self.settings.launch_at_login = enabled;
        self.persist_settings(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = cx
                .background_executor()
                .spawn(async move { set_launch_at_login(enabled) })
                .await;
            match result {
                Ok(()) => {
                    // The switch already shows the new state; no banner.
                }
                Err(err) => {
                    let text = err.to_string();
                    let _ = this.update(cx, |store, cx| {
                        // Revert: the toggle claimed a change that did not
                        // take, and a switch showing a lie is worse than a
                        // switch flashing back.
                        store.settings.launch_at_login = !enabled;
                        store.persist_settings(cx);
                        store.set_notice(NoticeOrigin::Settings, BannerTone::Error, text, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// The profile's on-disk text, as of the last snapshot.
    ///
    /// Served from the snapshot (`list_profiles` reads every file while
    /// listing), not re-read here: the editor calls this from its render
    /// path, and a disk read there is against the no-sync-I/O-in-render rule.
    /// The snapshot refreshes on every disk revision bump, so the editor sees
    /// outside changes at the same latency it always did.
    pub fn read_profile_content(&self, name: &str) -> Option<String> {
        self.snapshot
            .profiles
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.raw.clone())
    }

    fn apply_snapshot(&mut self, snap: Snapshot, bump_disk: bool) {
        let now_running = snap.running;
        if now_running {
            if self.connected_since.is_none() {
                self.connected_since = Some(Instant::now());
            }
            if !self.stop_requested {
                self.was_running_expected = true;
            }
        } else {
            self.connected_since = None;
        }

        let profiles_changed = snap.profiles != self.snapshot.profiles;
        if profiles_changed {
            // A profile that no longer exists takes its latency reading with it;
            // a renamed-away entry must not paint a row for a name that is gone.
            self.latency_results
                .retain(|name, _| snap.profiles.iter().any(|p| &p.name == name));
        }
        self.snapshot = snap;
        if profiles_changed || bump_disk {
            self.disk_revision = self.disk_revision.wrapping_add(1);
        }

        // A snapshot that started before a create still lacks the new profile.
        // Let it skip the selection repair, once: the next poll will have it, and
        // the bound means a profile that never appears cannot pin anything.
        if let Some((name, skips)) = self.pinned_selection.clone() {
            if self.snapshot.profiles.iter().any(|p| p.name == name) {
                self.pinned_selection = None;
                self.selected_profile = Some(name);
                return;
            }
            if skips > 0 {
                self.pinned_selection = Some((name, skips - 1));
                return;
            }
            self.pinned_selection = None;
        }

        let prev_sel = self.selected_profile.clone();
        if prev_sel.is_none()
            || !prev_sel
                .as_ref()
                .is_some_and(|n| self.snapshot.profiles.iter().any(|p| &p.name == n))
        {
            // A dangling active symlink must not pin the selection to a name
            // that has no profile behind it; fall through to a real one.
            let active_exists = self
                .snapshot
                .active
                .clone()
                .filter(|n| self.snapshot.profiles.iter().any(|p| &p.name == n));
            self.selected_profile =
                active_exists.or_else(|| self.snapshot.profiles.first().map(|p| p.name.clone()));
        }
    }

    pub fn refresh_snapshot(&mut self, force_disk_bump: bool, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };
        if self.snapshot_poll_in_flight {
            // Queue rather than stack. The queued bump still lands: it runs when
            // the poll already in flight completes.
            self.snapshot_poll_pending = true;
            self.snapshot_poll_force_pending |= force_disk_bump;
            return;
        }
        self.snapshot_poll_in_flight = true;

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let snap = cx
                .background_executor()
                .spawn({
                    let ctrl = ctrl.clone();
                    async move { ctrl.snapshot() }
                })
                .await;
            let _ = this.update(cx, |store, cx| {
                store.snapshot_poll_in_flight = false;
                // The first answer always repaints: the placeholder the window is
                // showing can be *equal* to the real state (stopped, no profiles),
                // so `changed` alone would leave "Checking…" up forever.
                let first_answer = !store.probed_once;
                store.probed_once = true;
                if first_answer {
                    cx.notify();
                }
                if let Ok(s) = snap {
                    if store.snapshot_error.take().is_some() {
                        // The fault is gone; its banner should not outlive it.
                        store.clear_probe_error_notice();
                    }
                    let unexpected_drop = store.was_running_expected
                        && !s.running
                        && store.busy_phase.is_none()
                        && !store.stop_requested;
                    let confirmed_stop = store.stop_requested && !s.running;
                    let changed = s != store.snapshot;
                    if changed || force_disk_bump {
                        store.apply_snapshot(s, force_disk_bump);
                    }
                    if confirmed_stop {
                        store.stop_requested = false;
                    }
                    if unexpected_drop {
                        store.was_running_expected = false;
                        ctrl.notify_error(
                            "sing-box disconnected",
                            "sing-box stopped unexpectedly. Check Logs.",
                        );
                        // Report where the user actually is: a crash is just as
                        // likely while the editor or Logs is open.
                        store.set_notice(
                            store.notice_origin_for_current_tab(),
                            BannerTone::Error,
                            "sing-box stopped unexpectedly. Check Logs.".to_string(),
                            cx,
                        );
                    }
                    // Repaint only when something the window shows has changed.
                    // Notifying on every poll re-ran every render body (and, until
                    // the profile facts were cached, re-read a file) for nothing.
                    if changed || force_disk_bump || unexpected_drop || confirmed_stop {
                        cx.notify();
                    }
                } else if let Err(err) = snap {
                    // A failing probe used to leave the first frame's
                    // "Checking…" up forever, since nothing repainted. Show why
                    // and stop pretending a probe is still running.
                    let message = format!("Could not read the service state: {}", err.message);
                    let repeated = store.snapshot_error.as_deref() == Some(&message);
                    store.snapshot_error = Some(message.clone());
                    // Only on change: a probe that keeps failing every 5s must not
                    // keep replacing whatever else the user is reading.
                    if !repeated {
                        store.set_notice(
                            store.notice_origin_for_current_tab(),
                            BannerTone::Error,
                            message,
                            cx,
                        );
                    }
                    cx.notify();
                }
                // Drain a queued refresh whatever the outcome, or a failed poll
                // would strand a pending forced bump forever.
                if store.snapshot_poll_pending {
                    store.snapshot_poll_pending = false;
                    let force = std::mem::take(&mut store.snapshot_poll_force_pending);
                    store.refresh_snapshot(force, cx);
                }
            });
        });
        self.snapshot_task = Some(task);
    }

    pub fn toggle_service(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        if self.snapshot.running {
            self.disconnect(NoticeOrigin::Toolbar, cx);
        } else {
            self.connect_active(cx);
        }
    }

    pub fn ready_connection_target(&self) -> Option<&str> {
        let ready = |name: &&str| {
            self.snapshot
                .profiles
                .iter()
                .any(|p| p.name == *name && p.ready())
        };
        self.selected_profile
            .as_deref()
            .filter(ready)
            .or_else(|| self.snapshot.active.as_deref().filter(ready))
            .or_else(|| {
                self.snapshot
                    .profiles
                    .iter()
                    .find(|p| p.ready())
                    .map(|p| p.name.as_str())
            })
    }

    pub fn connect_active(&mut self, cx: &mut Context<Self>) {
        let target = self.ready_connection_target().map(str::to_owned);
        let Some(name) = target else {
            self.selected_tab = NavTab::Profiles;
            self.set_notice(
                NoticeOrigin::Toolbar,
                BannerTone::Warning,
                "No ready profile. Create or finish configuring a profile first.",
                cx,
            );
            return;
        };
        self.activate_profile(name, NoticeOrigin::Toolbar, cx);
    }

    /// Which surface is showing right now, for menu commands that must report
    /// somewhere the user can see.
    pub fn notice_origin_for_current_tab(&self) -> NoticeOrigin {
        match self.selected_tab {
            NavTab::Logs => NoticeOrigin::Logs,
            NavTab::Settings => NoticeOrigin::Settings,
            NavTab::Profiles if self.editing => NoticeOrigin::Editor,
            NavTab::Profiles => NoticeOrigin::Stage,
        }
    }

    pub fn restart_active(&mut self, origin: NoticeOrigin, cx: &mut Context<Self>) {
        if let Some(active) = self.snapshot.active.clone() {
            self.activate_profile(active, origin, cx);
        } else {
            self.connect_active(cx);
        }
    }

    /// Activate `name`, reporting the outcome on the surface that asked for it.
    ///
    /// The stage unmounts while the editor is open and the toolbar is never the
    /// stage, so a hard-coded origin posts results where nothing draws them.
    pub fn activate_profile(&mut self, name: String, origin: NoticeOrigin, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.stop_requested = false;
        self.busy_phase = Some(format!("Activating {name}..."));
        self.busy_target = Some(name.clone());
        // Clear only this surface's previous message. One slot backs every
        // origin, so clearing unconditionally discarded e.g. an unread settings
        // failure just because the user started a connect.
        self.clear_notice_for(origin);
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let worker = cx.background_executor().spawn({
                let ctrl = ctrl.clone();
                async move {
                    let res = ctrl.gui_activate(&target, |phase| {
                        let _ = phase_tx.send_blocking(phase.label().to_string());
                    });
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                }
            });

            while let Ok(label) = phase_rx.recv().await {
                let _ = this.update(cx, |store, cx| {
                    store.busy_phase = Some(label);
                    cx.notify();
                });
            }

            let (res, snap) = worker.await;
            let mut should_refresh_ip = false;
            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(()) => {
                        store.connected_since = Some(Instant::now());
                        store.was_running_expected = true;
                        store.selected_profile = Some(name.clone());
                        // No success banner: the connection card already shows
                        // "Connected" with this profile's name.
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        if err.code != ExitCode::Service {
                            ctrl.notify_error("sing-box activation failed", &err.message);
                        }
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        })
        .detach();
    }

    pub fn save_and_activate(&mut self, name: String, content: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.stop_requested = false;
        self.busy_phase = Some(format!("Saving and activating {name}..."));
        self.busy_target = Some(name.clone());
        self.clear_notice_for(NoticeOrigin::Editor);
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let worker = cx.background_executor().spawn({
                let ctrl = ctrl.clone();
                async move {
                    match ctrl.gui_save_profile(&target, &content, false) {
                        Ok(true) => (Ok(()), ctrl.snapshot().ok()),
                        Ok(false) => {
                            let res = ctrl.gui_activate(&target, |phase| {
                                let _ = phase_tx.send_blocking(phase.label().to_string());
                            });
                            (res, ctrl.snapshot().ok())
                        }
                        Err(err) => (Err(err), ctrl.snapshot().ok()),
                    }
                }
            });

            while let Ok(label) = phase_rx.recv().await {
                let _ = this.update(cx, |store, cx| {
                    store.busy_phase = Some(label);
                    cx.notify();
                });
            }

            let (res, snap) = worker.await;
            let mut should_refresh_ip = false;
            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(()) => {
                        store.connected_since = Some(Instant::now());
                        store.was_running_expected = true;
                        store.selected_profile = Some(name.clone());
                        store.editor_validation =
                            Some((name.clone(), EditorValidationState::Valid));
                        // Save & Connect runs while the editor is mounted, so
                        // the result belongs on the editor, not the stage it
                        // covers.
                        // No success banner: the editor's validation line and the
                        // connection card both show the result.

                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        if err.code != ExitCode::Service {
                            ctrl.notify_error("sing-box activation failed", &err.message);
                        }
                        store.editor_validation = Some((
                            name.clone(),
                            EditorValidationState::Invalid(error_text(&err)),
                        ));
                        store.set_notice(
                            NoticeOrigin::Editor,
                            BannerTone::Error,
                            error_text(&err),
                            cx,
                        );
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        })
        .detach();
    }

    /// Stop sing-box, reporting the outcome where the control lives.
    pub fn disconnect(&mut self, origin: NoticeOrigin, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.was_running_expected = false;
        self.stop_requested = true;
        self.busy_phase = Some("Stopping sing-box...".to_string());
        self.clear_notice_for(origin);
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_stop();
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                })
                .await;

            let mut should_refresh_ip = false;
            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(()) => {
                        // Clear the intent here too: a keepalive can bring the
                        // service straight back up, and nothing else would ever
                        // clear it, which silences later crash alerts.
                        store.stop_requested = false;
                        store.was_running_expected = false;
                        store.connected_since = None;
                        // No info banner: the card flips to "Disconnected".

                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        // The stop did not happen. Leaving the intent set would
                        // suppress the next "stopped unexpectedly" alert for the
                        // rest of the session.
                        store.stop_requested = false;
                        store.was_running_expected = store.snapshot.running;
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        })
        .detach();
    }

    /// Record that a share-link import created `name` on disk.
    ///
    /// Mirrors the post-landing half of `pull_profile`: the pin holds the
    /// selection against a poll that started earlier and lacks the name, and
    /// the validation generation drops the previous profile's pending reply.
    pub fn import_landed(&mut self, name: String, cx: &mut Context<Self>) {
        self.pinned_selection = Some((name.clone(), 1));
        self.selected_profile = Some(name.clone());
        self.selected_tab = NavTab::Profiles;
        self.editing = false;
        self.validation_generation = self.validation_generation.wrapping_add(1);
        self.editor_validation = None;
        cx.notify();
    }

    /// Create a profile from the skeleton and stage it.
    ///
    /// The core call runs on the background executor, not on the UI thread:
    /// on Windows non-elevated it can dispatch a UAC prompt and block on it,
    /// and even the local happy path fsyncs the new file. The rest of this
    /// method (selection, tab, watcher restart) is applied back on the main
    /// thread once the create lands.
    pub fn create_profile(&mut self, name: &str, cx: &mut Context<Self>) -> CoreResult<()> {
        let Some(ctrl) = self.ctrl.clone() else {
            return Err(SingSwitchError::general(
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported operating system".to_string()),
            ));
        };
        let name = name.to_string();
        self.busy_phase = Some(format!("Creating {name}..."));
        self.busy_target = Some(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let name_for_work = name.clone();
            let result = cx
                .background_executor()
                .spawn(async move { ctrl.gui_create_profile(&name_for_work) })
                .await;
            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                match result {
                    Ok(_) => {
                        // A poll that started before this create lands with a
                        // profile list that does not contain the new name yet,
                        // and would reselect over it. Hold the selection until
                        // a snapshot that knows about the name arrives.
                        store.pinned_selection = Some((name.clone(), 1));
                        store.selected_profile = Some(name.clone());
                        // The probe and profile scan belong on the background
                        // executor: a direct `ctrl.snapshot()` here ran
                        // `launchctl print` (30s deadline) and re-parsed every
                        // profile from the handler.
                        store.refresh_snapshot(true, cx);
                        store.selected_tab = NavTab::Profiles;
                        store.validation_generation = store.validation_generation.wrapping_add(1);
                        store.editor_validation = None;
                        // A new profile is shown on the stage, so leave the
                        // editor if it was open: otherwise the stage is
                        // unmounted and its "Created" notice is never drawn.
                        store.editing = false;
                        store.restart_file_watcher(cx);
                    }
                    Err(err) => {
                        store.set_notice(
                            NoticeOrigin::Stage,
                            BannerTone::Error,
                            error_text(&err),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();

        Ok(())
    }

    pub fn pull_profile(
        &mut self,
        name: String,
        url: String,
        origin: NoticeOrigin,
        cx: &mut Context<Self>,
    ) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Downloading {name}..."));
        self.busy_target = Some(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_pull_profile(&name, &url);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(()) => {
                        // Same race as `create_profile`: the snapshot taken here
                        // may precede the import, and a poll that started earlier
                        // lands later with a profile list that still lacks this
                        // name, reselecting over it. Pin the selection until a
                        // snapshot that knows about the name arrives.
                        store.pinned_selection = Some((name.clone(), 1));
                        store.selected_profile = Some(name.clone());
                        store.selected_tab = NavTab::Profiles;
                        store.editing = false;
                        store.validation_generation = store.validation_generation.wrapping_add(1);
                        store.editor_validation = None;
                        store.restart_file_watcher(cx);
                        // The new row appears in the list; no banner.
                    }
                    Err(err) => {
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Decide whether a sync is due, and record the attempt when it is.
    ///
    /// Returns whether a sync should start now; on `true` it also records the
    /// attempt, so a sync that fails still waits a full interval before the
    /// next one rather than retrying every second.
    fn auto_sync_due(&mut self, now_unix: Option<u64>) -> bool {
        let minutes = self.settings.auto_sync_interval_minutes;
        if minutes == 0 {
            return false;
        }
        // A hidden window still syncs: the whole point is to keep profiles
        // current while the app sits in the tray. A busy store does not: the
        // restart that a changed profile triggers would fight whatever the
        // user is already running.
        if self.busy_phase.is_some() || self.auto_sync_in_flight || self.ctrl.is_none() {
            return false;
        }
        let Some(now) = now_unix else {
            // No wall clock this instant: decide again next tick rather than
            // guessing.
            return false;
        };
        let interval = u64::from(minutes) * 60;
        // `last` is the wall-clock seconds of the last sync — the persisted
        // stamp while it is young, this process's own memory once it is the
        // fresher of the two. Never having run is due immediately; a stamp in
        // the future (clock moved back, or the file edited) counts as "just
        // ran" so a regression never stampedes.
        let last = self
            .last_auto_sync_at_wall
            .or(self.settings.last_auto_sync_at);
        let elapsed = last.and_then(|l| now.checked_sub(l)).unwrap_or(u64::MAX);
        let due = elapsed >= interval;
        if due {
            self.last_auto_sync_at_wall = Some(now);
            // The persisted stamp is updated only on success below; this
            // in-memory copy keeps the next tick from re-firing while the
            // background half runs.
        }
        due
    }

    /// Run one automatic sync when its interval has come around.
    ///
    /// Called from the shared ticker loop; it does the same work the toolbar's
    /// "Sync remote profiles" button does, on the same background task shape,
    /// so an automatic sync and a manual one cannot behave differently.
    pub fn maybe_auto_sync(&mut self, cx: &mut Context<Self>) {
        if !self.auto_sync_due(unix_now()) {
            return;
        }
        // The persisted stamp is written only after the sync succeeds, so a
        // settings file that cannot be written cannot silently resurrect the
        // old stamp; and persist_settings' own error notice is not overwritten
        // by this cycle's completion notice a second later.
        self.run_auto_sync(cx);
    }

    /// The background half of an automatic sync, identical in shape to `sync_all`.
    fn run_auto_sync(&mut self, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };
        // No busy spinner: nobody asked for this, and a control that goes dead
        // on its own schedule reads as a hang. The busy guard in
        // `auto_sync_due` already keeps it from stacking.
        self.auto_sync_in_flight = true;

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_sync_all();
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.auto_sync_in_flight = false;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                // Only disturb the notice when the outcome deserves one: a
                // clean cycle reports nothing (the user asked for freshness,
                // not for commentary), and an error notice the user has not
                // read yet survives a scheduled cycle landing behind it.
                match &res {
                    Ok(report) if !report.failed.is_empty() => {
                        store.set_notice(
                            NoticeOrigin::Toolbar,
                            BannerTone::Warning,
                            format!(
                                "Automatic sync failed for {}",
                                count_noun(report.failed.len(), "profile", "profiles")
                            ),
                            cx,
                        );
                    }
                    Ok(report) if report.synced.is_empty() => {
                        // Nothing to sync is the idle state, not a message.
                    }
                    Ok(_) => {
                        // Persist the stamp on success only: this is the value
                        // a relaunch seeds its own timer from. Writing it with
                        // the result lets a failure keep its age, so the next
                        // cycle is still a full interval away.
                        if let Some(stamp) = store.last_auto_sync_at_wall {
                            store.settings.last_auto_sync_at = Some(stamp);
                            store.persist_settings(cx);
                        }
                        // Profiles were fetched and stored without error. Not
                        // every one of those changed the running profile, and a
                        // cycle that fetched-and-verified but stored nothing
                        // new deserves no banner at all.
                    }
                    Err(err) => {
                        store.set_notice(
                            NoticeOrigin::Toolbar,
                            BannerTone::Error,
                            error_text(err),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn sync_profile(&mut self, name: String, origin: NoticeOrigin, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Syncing {name}..."));
        self.busy_target = Some(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_sync_one(&name);
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(_reloaded) => {
                        // The profile list's disk revision moves on; no banner.
                    }
                    Err(err) => {
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn sync_all(&mut self, origin: NoticeOrigin, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some("Syncing remote profiles...".to_string());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_sync_all();
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(report) => {
                        if !report.failed.is_empty() {
                            let details = report
                                .failed
                                .iter()
                                .map(|(n, e)| format!("{n}: {e}"))
                                .collect::<Vec<_>>()
                                .join("; ");
                            let synced = count_noun(report.synced.len(), "profile", "profiles");
                            store.set_notice(
                                origin,
                                BannerTone::Warning,
                                format!("Synced {synced}; failed: {details}"),
                                cx,
                            );
                        } else if report.synced.is_empty() {
                            // Nothing to sync is the idle state, not a message.
                        } else {
                            // The list refreshes; no banner.
                        }
                    }
                    Err(err) => {
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn save_profile_content(
        &mut self,
        name: String,
        content: String,
        allow_invalid: bool,
        cx: &mut Context<Self>,
    ) {
        let origin = NoticeOrigin::Editor;
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Saving {name}..."));
        self.busy_target = Some(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_save_profile(&name, &content, allow_invalid);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(reloaded) => {
                        let _ = reloaded;
                        if !allow_invalid {
                            store.editor_validation =
                                Some((name.clone(), EditorValidationState::Valid));
                        }
                        // The editor's validation line says "Configuration valid".
                    }
                    Err(err) => {
                        store.editor_validation = Some((
                            name.clone(),
                            EditorValidationState::Invalid(error_text(&err)),
                        ));
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn validate_live_content(&mut self, name: String, content: String, cx: &mut Context<Self>) {
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.validation_generation = self.validation_generation.wrapping_add(1);
        let generation = self.validation_generation;
        self.editor_validation = Some((name.clone(), EditorValidationState::Validating));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.check_json_content(&name, &content);
                    (name, res)
                })
                .await;
            let _ = this.update(cx, |store, cx| {
                if store.validation_generation != generation {
                    return;
                }
                match res {
                    Ok(()) => {
                        store.editor_validation = Some((name, EditorValidationState::Valid));
                        // editor_validation already carries the valid state.
                    }
                    Err(err) => {
                        let msg = error_text(&err);
                        store.editor_validation =
                            Some((name, EditorValidationState::Invalid(msg.clone())));
                        store.set_notice(NoticeOrigin::Editor, BannerTone::Error, msg, cx);
                    }
                }
                cx.notify();
            });
        });
        self.validation_task = Some(task);
    }

    /// Delete a profile, reporting the outcome where the request came from.
    ///
    /// The stage is unmounted while the editor is open, so a hard-coded origin
    /// made a refusal (and a success) invisible from the editor.
    pub fn delete_profile(
        &mut self,
        name: String,
        force: bool,
        origin: NoticeOrigin,
        cx: &mut Context<Self>,
    ) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.busy_phase = Some(format!("Deleting {name}..."));
        self.busy_target = Some(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_delete_profile(&name, force);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(_) => {
                        if store.selected_profile.as_deref() == Some(name.as_str()) {
                            store.selected_profile = store.snapshot.active.clone().or_else(|| {
                                store.snapshot.profiles.first().map(|p| p.name.clone())
                            });
                        }
                        // The row disappears from the list; no banner.
                    }
                    Err(err) => {
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop the cached egress because the tunnel is about to change under it.
    ///
    /// Called on a successful connect or switch. Without this a reply from the
    /// previous egress can land afterwards and be presented as the current one.
    pub fn invalidate_ip(&mut self) {
        // Generation bump only. Setting the pending flag here made every
        // connect/switch run two lookups: the caller refreshes immediately, and
        // the leftover flag started a second fetch when the first finished.
        self.ip_generation = self.ip_generation.wrapping_add(1);
        self.ip_info = None;
    }

    /// A latency probe against a named profile's outbound. The result lands in
    /// `latency_results` so the profile list's LATENCY column shows it; it never
    /// becomes a banner, because a measurement is data, not a failure.
    ///
    /// Testing must never touch the running daemon: `tools fetch` only
    /// pre-starts the outbound.
    pub fn test_profile_latency(&mut self, name: String, cx: &mut Context<Self>) {
        if self.latency_in_flight.contains(&name) {
            return;
        }
        let Some(path) = self
            .snapshot
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .map(|profile| profile.path.clone())
        else {
            return;
        };
        self.latency_in_flight.insert(name.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let probe_name = name.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    // The outbound tag lives in the document, and the store's
                    // snapshot does not carry it.
                    let tag = std::fs::read_to_string(&path)
                        .map_err(|err| format!("could not read {probe_name}: {err}"))
                        .and_then(|raw| {
                            ProfileFormModel::from_json_str(&raw)
                                .map_err(|err| format!("{probe_name} is not a valid config: {err}"))
                        })
                        .and_then(|model| match model.has_managed_outbound {
                            true => Ok(model.outbound_tag),
                            false => Err(format!("{probe_name} has no outbound to test")),
                        })?;
                    singswitch_core::checker::ExecChecker::default()
                        .probe_latency(
                            &path,
                            &tag,
                            singswitch_core::checker::DEFAULT_LATENCY_PROBE_URL,
                        )
                        .map_err(|err| crate::state::error_text(&err))
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.latency_in_flight.remove(&name);
                store.latency_results.insert(
                    name.clone(),
                    LatencyReading {
                        since_disk_revision: store.disk_revision,
                        result,
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    pub fn refresh_ip(&mut self, cx: &mut Context<Self>) {
        if self.ip_loading {
            // Queue it: a switch during a fetch must still end with a refresh,
            // otherwise the previous egress stays on screen.
            self.ip_refresh_pending = true;
            return;
        }
        self.ip_generation = self.ip_generation.wrapping_add(1);
        let generation = self.ip_generation;
        self.ip_loading = true;
        self.ip_error = None;
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = cx
                .background_executor()
                .spawn(async move { fetch_ip_info(None) })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.ip_loading = false;
                // A reply from before the tunnel changed describes an egress
                // this screen no longer has.
                if store.ip_generation == generation {
                    match res {
                        Ok(info) => {
                            store.ip_info = Some(info);
                            store.ip_error = None;
                        }
                        Err(err) => {
                            store.ip_error = Some(err.message.clone());
                        }
                    }
                }
                if store.ip_refresh_pending {
                    store.ip_refresh_pending = false;
                    store.refresh_ip(cx);
                }
                cx.notify();
            });
        });
        self.ip_task = Some(task);
    }

    pub fn repair_macos_logs(&mut self, origin: NoticeOrigin, cx: &mut Context<Self>) {
        let Some(root_script) = singswitch_core::doctor::macos_logs_root_script(self.layout())
        else {
            return;
        };
        self.run_macos_admin_script(
            "Authorizing log permissions...",
            root_script,
            true,
            origin,
            cx,
        );
    }

    fn run_macos_admin_script(
        &mut self,
        phase: &'static str,
        root_script: String,
        reload_logs_after: bool,
        origin: NoticeOrigin,
        cx: &mut Context<Self>,
    ) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.busy_phase = Some(phase.to_string());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let apple_script = format!(
                        "do shell script \"{}\" with administrator privileges",
                        root_script.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    // A separate runner instance, because this one waits on a
                    // human typing a password: the 30s command timeout is far too
                    // short, and killing it mid-prompt is worse than waiting out
                    // ADMIN_PROMPT_TIMEOUT.
                    // The cost is that `busy_phase` stays set while the dialog is
                    // open, so snapshot polling pauses with it; the slowest path
                    // is bounded by ADMIN_PROMPT_TIMEOUT, not forever.
                    let out = singswitch_core::runner::Runner::run(
                        &singswitch_core::runner::ExecRunner {
                            timeout: ADMIN_PROMPT_TIMEOUT,
                            trace: None,
                        },
                        "osascript",
                        &["-e", &apple_script],
                    );
                    let snap = ctrl.snapshot().ok();
                    (out, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.busy_target = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(out) if out.success => {
                        if reload_logs_after {
                            store.reload_logs();
                        }
                        // No success banner; the fix is visible in the logs.
                    }
                    Ok(out) => {
                        let msg = out.combined.trim().to_string();
                        let banner = if msg.is_empty() {
                            "Administrator authorization failed".to_string()
                        } else {
                            msg
                        };
                        store.set_notice(origin, BannerTone::Error, banner, cx);
                    }
                    Err(err) => {
                        store.set_notice(origin, BannerTone::Error, error_text(&err), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn reload_logs(&mut self) {
        // The borrowed layout cannot be held across the buffer mutation below,
        // so clone the two paths rather than the whole layout.
        let error_log_path = self.layout().error_log_path.clone();
        let access_log_path = self.layout().access_log_path.clone();
        self.error_logs.clear();
        self.access_logs.clear();
        self.error_log_offset = 0;
        self.access_log_offset = 0;
        self.log_generation = self.log_generation.wrapping_add(1);
        // Refresh identity so the next poll compares against the file we just
        // read rather than the one that existed before a rotation.
        self.error_log_id = singswitch_core::logs::log_file_id(&error_log_path);
        self.access_log_id = singswitch_core::logs::log_file_id(&access_log_path);

        load_tail_stream(
            &error_log_path,
            &mut self.error_logs,
            &mut self.error_log_offset,
            &mut self.error_log_err,
        );
        load_tail_stream(
            &access_log_path,
            &mut self.access_logs,
            &mut self.access_log_offset,
            &mut self.access_log_err,
        );
        // After the tails are loaded, or the badge counts the emptied buffer.
        self.sync_log_counts();
        self.log_revision = self.log_revision.wrapping_add(1);
    }

    /// Whether the logs on screen are worth polling.
    ///
    /// Reading the files for a pane nobody is looking at wasted IO: the watcher
    /// used `window_open || tab == Logs`, which is true for a hidden window with
    /// Logs still selected.
    fn should_poll_logs(&self) -> bool {
        self.window_open && self.selected_tab == NavTab::Logs && self.log_follow
    }

    pub fn poll_logs_async(&mut self, cx: &mut Context<Self>) {
        if self.log_poll_in_flight {
            return;
        }
        self.log_poll_in_flight = true;

        let err_path = self.layout().error_log_path.clone();
        let acc_path = self.layout().access_log_path.clone();
        let err_off = self.error_log_offset;
        let acc_off = self.access_log_offset;
        let err_id = self.error_log_id;
        let acc_id = self.access_log_id;
        let log_generation = self.log_generation;

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (err_res, acc_res, err_id_now, acc_id_now) = cx
                .background_executor()
                .spawn({
                    let err_path = err_path.clone();
                    let acc_path = acc_path.clone();
                    async move {
                        let err_id_now = singswitch_core::logs::log_file_id(&err_path);
                        let acc_id_now = singswitch_core::logs::log_file_id(&acc_path);
                        // A replaced file must be read from the start even when
                        // it is longer than the previous offset, otherwise the
                        // first bytes of the new file are skipped.
                        let err_from = match (err_id, err_id_now) {
                            (Some(prev), Some(now)) if prev.is_different_file(&now) => 0,
                            _ => err_off,
                        };
                        let acc_from = match (acc_id, acc_id_now) {
                            (Some(prev), Some(now)) if prev.is_different_file(&now) => 0,
                            _ => acc_off,
                        };
                        (
                            read_appended_lines(&err_path, err_from),
                            read_appended_lines(&acc_path, acc_from),
                            err_id_now,
                            acc_id_now,
                        )
                    }
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.log_poll_in_flight = false;
                if store.log_generation != log_generation {
                    // `reload_logs` already re-read the tails; this delta was
                    // computed against the offsets it replaced.
                    return;
                }
                let err_rotated = matches!(
                    (store.error_log_id, err_id_now),
                    (Some(prev), Some(now)) if prev.is_different_file(&now)
                );
                let acc_rotated = matches!(
                    (store.access_log_id, acc_id_now),
                    (Some(prev), Some(now)) if prev.is_different_file(&now)
                );
                if err_rotated {
                    store.error_logs.clear();
                    store.error_log_offset = 0;
                }
                if acc_rotated {
                    store.access_logs.clear();
                    store.access_log_offset = 0;
                }
                store.error_log_id = err_id_now;
                store.access_log_id = acc_id_now;
                let mut changed = err_rotated || acc_rotated;
                changed |= apply_log_delta(
                    &mut store.error_logs,
                    &mut store.error_log_offset,
                    &mut store.error_log_err,
                    &err_path,
                    err_res,
                );
                if changed || err_rotated {
                    store.sync_log_counts();
                }
                changed |= apply_log_delta(
                    &mut store.access_logs,
                    &mut store.access_log_offset,
                    &mut store.access_log_err,
                    &acc_path,
                    acc_res,
                );
                if changed {
                    store.log_revision = store.log_revision.wrapping_add(1);
                    cx.notify();
                }
            });
        });
        self.log_poll_task = Some(task);
    }

    pub fn active_log_records(&self) -> &[LogRecord] {
        match self.log_stream {
            LogStreamKind::Error => &self.error_logs,
            LogStreamKind::Access => &self.access_logs,
        }
    }

    pub fn active_log_error(&self) -> Option<&str> {
        match self.log_stream {
            LogStreamKind::Error => self.error_log_err.as_deref(),
            LogStreamKind::Access => self.access_log_err.as_deref(),
        }
    }

    fn restart_file_watcher(&mut self, cx: &mut Context<Self>) {
        // The watched paths come from `layout`, which never changes for the
        // life of the store, and a live watcher already covers them:
        // rebuilding on every create/pull dropped real events (the profile
        // file just written) for a watcher that watched the same paths. Only
        // an absent watcher is built.
        if self._watcher.is_some() {
            return;
        }

        let Some(ctrl) = &self.ctrl else {
            return;
        };
        let profiles_dir = ctrl.layout.profiles_dir.clone();
        let active_cfg = ctrl.layout.active_config_path.clone();
        let err_log = ctrl.layout.error_log_path.clone();
        let acc_log = ctrl.layout.access_log_path.clone();

        let mut dirs = HashSet::new();
        for dir in [
            Some(profiles_dir.clone()),
            active_cfg.parent().map(PathBuf::from),
            err_log.parent().map(PathBuf::from),
            acc_log.parent().map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        {
            if dir.exists() {
                dirs.insert(dir);
            }
        }

        if dirs.is_empty() {
            return;
        }

        let (tx, rx) = smol::channel::unbounded::<WatchEventKind>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res
                && let Some(kind) = classify_watch_event(
                    &event.paths,
                    &profiles_dir,
                    &active_cfg,
                    &err_log,
                    &acc_log,
                )
            {
                let _ = tx.try_send(kind);
            }
        });

        let mut w = match watcher {
            Ok(w) => w,
            Err(err) => {
                // The watcher is what makes config writes visible; with it
                // dead the window can sit on stale profiles until the next
                // poll (up to 180 s hidden). Say so instead of looking live.
                crate::diag::log(&format!("file watcher unavailable: {err}"));
                self.set_notice(
                    NoticeOrigin::Stage,
                    BannerTone::Warning,
                    "Live profile updates are unavailable; changes appear on the next refresh.",
                    cx,
                );
                return;
            }
        };
        {
            let mut failures = Vec::new();
            for dir in dirs {
                if let Err(err) = w.watch(&dir, RecursiveMode::NonRecursive) {
                    failures.push(format!("{}: {err}", dir.display()));
                }
            }
            if !failures.is_empty() {
                // Partial coverage still updates logs via `list_profiles`
                // on the poll cadence, so warn rather than declare the
                // watcher dead.
                crate::diag::log(&format!(
                    "file watcher: could not watch {}",
                    failures.join(", ")
                ));
                let message = format!(
                    "Could not watch {} for changes; profile updates appear on the next refresh.",
                    failures.join(", ")
                );
                self.set_notice(NoticeOrigin::Stage, BannerTone::Warning, message, cx);
            }
            self._watcher = Some(w);
            self.watcher_task = Some(cx.spawn(
                async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    while let Ok(first) = rx.recv().await {
                        cx.background_executor().timer(WATCHER_DEBOUNCE).await;
                        let mut combined = first;
                        while let Ok(more) = rx.try_recv() {
                            combined = merge_watch_kinds(combined, more);
                        }
                        let _ = this.update(cx, |store, cx| {
                            if matches!(combined, WatchEventKind::Config | WatchEventKind::Both) {
                                store.refresh_snapshot(true, cx);
                            }
                            if matches!(combined, WatchEventKind::Logs | WatchEventKind::Both)
                                && store.should_poll_logs()
                            {
                                store.poll_logs_async(cx);
                            }
                        });
                    }
                },
            ));
        }
    }

    fn start_background_pollers(&mut self, cx: &mut Context<Self>) {
        if self.ctrl.is_none() {
            // No service to poll, but the Logs tab still follows the files.
            let log_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(SNAPSHOT_TICK_INTERVAL).await;
                    let keep_going = this
                        .update(cx, |store, cx| {
                            if store.should_poll_logs() {
                                store.poll_logs_async(cx);
                            }
                        })
                        .is_ok();
                    if !keep_going {
                        break;
                    }
                }
            });
            self._daemon_tasks.push(log_task);
            return;
        }
        // One ticker drives both. A second 800ms loop for the log tail re-tested
        // the same flag 75 times a minute while the app sat in the tray.
        let snap_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut last_poll = Instant::now();
            loop {
                cx.background_executor().timer(SNAPSHOT_TICK_INTERVAL).await;
                let keep_going = this
                    .update(cx, |store, cx| {
                        if store.should_poll_logs() {
                            store.poll_logs_async(cx);
                        }
                        let interval = if store.window_open {
                            ACTIVE_SNAPSHOT_INTERVAL
                        } else {
                            BACKGROUND_SNAPSHOT_INTERVAL
                        };
                        if store.busy_phase.is_none() && last_poll.elapsed() >= interval {
                            last_poll = Instant::now();
                            store.refresh_snapshot(false, cx);
                        }
                        // Rides this tick on purpose. A second timer would be
                        // another wake-up a second and another thing to cancel
                        // when the store goes away.
                        store.maybe_auto_sync(cx);
                    })
                    .is_ok();
                if !keep_going {
                    break;
                }
            }
        });
        self._daemon_tasks.push(snap_task);
    }
}

fn classify_watch_event(
    paths: &[PathBuf],
    profiles_dir: &Path,
    active_cfg: &Path,
    err_log: &Path,
    acc_log: &Path,
) -> Option<WatchEventKind> {
    let mut saw_cfg = false;
    let mut saw_log = false;
    for path in paths {
        if path == err_log || path == acc_log {
            saw_log = true;
        } else if path == active_cfg || path == profiles_dir || path.parent() == Some(profiles_dir)
        {
            saw_cfg = true;
        }
    }
    match (saw_cfg, saw_log) {
        (true, true) => Some(WatchEventKind::Both),
        (true, false) => Some(WatchEventKind::Config),
        (false, true) => Some(WatchEventKind::Logs),
        (false, false) => None,
    }
}

fn merge_watch_kinds(a: WatchEventKind, b: WatchEventKind) -> WatchEventKind {
    if a == b { a } else { WatchEventKind::Both }
}

fn format_log_err(path: &Path, err: &std::io::Error) -> Option<String> {
    if err.kind() == std::io::ErrorKind::NotFound {
        None
    } else {
        Some(format!("{}: {err}", path.display()))
    }
}

fn load_tail_stream(
    path: &Path,
    buf: &mut Vec<LogRecord>,
    offset: &mut u64,
    err_slot: &mut Option<String>,
) {
    match read_tail_lines(path, INITIAL_LOG_TAIL) {
        Ok((lines, off)) => {
            *offset = off;
            *buf = lines.into_iter().map(into_log_record).collect();
            *err_slot = None;
        }
        Err(err) => {
            *err_slot = format_log_err(path, &err);
        }
    }
}

fn apply_log_delta(
    buf: &mut Vec<LogRecord>,
    offset: &mut u64,
    err_slot: &mut Option<String>,
    path: &Path,
    res: std::io::Result<(Vec<String>, u64, bool)>,
) -> bool {
    let (lines, new_off, truncated) = match res {
        Ok(tuple) => tuple,
        Err(err) => {
            let next_err = format_log_err(path, &err);
            if *err_slot != next_err {
                *err_slot = next_err;
                return true;
            }
            return false;
        }
    };
    let mut changed = err_slot.take().is_some();
    if truncated || new_off < *offset {
        buf.clear();
        changed = true;
    }
    *offset = new_off;
    if !lines.is_empty() {
        changed = true;
        push_bounded_logs(buf, lines);
    }
    changed
}

/// One queued UI-settings save, produced by `persist_settings`.
type SettingsSaveJob = UiSettings;

/// An error message for the window, including the hints the CLI also prints.
///
/// `Display` is the message alone, so rollback outcomes ("no previous profile",
/// "could not restore X", "reverted to X but restart failed") all reached the UI
/// as the same generic line.
pub fn error_text(err: &singswitch_core::error::SingSwitchError) -> String {
    if err.hints.is_empty() {
        return err.message.clone();
    }
    format!("{} — {}", err.message, err.hints.join("; "))
}

fn next_log_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn into_log_record(line: String) -> LogRecord {
    // sing-box writes coloured output even to a file, so strip escapes once here
    // and every consumer (list, search, copy, detail) sees clean text. Only a
    // line that actually carries an escape byte pays for a stripped copy; a
    // clean line goes straight into the SharedString, same gate as
    // `LogLevel::classify`.
    let line = if line.as_bytes().contains(&0x1b) {
        singswitch_core::logs::strip_ansi(&line)
    } else {
        line
    };
    let level = LogLevel::classify(&line);
    LogRecord {
        id: next_log_id(),
        line: SharedString::from(line),
        level,
    }
}

fn push_bounded_logs(buf: &mut Vec<LogRecord>, lines: Vec<String>) {
    // Delegated to core so the eviction logic lives in one place.
    singswitch_core::logs::append_bounded(buf, lines, MAX_LOG_ENTRIES, into_log_record);
}

pub(crate) fn count_noun(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// Wall-clock seconds since the epoch, for the persisted auto-sync stamp.
///
/// `None` on a clock before 1970, which no real machine has; the caller then
/// simply treats the interval as already elapsed.
fn unix_now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
