use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AsyncApp, Context, SharedString, Task, WeakEntity};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use sbctl_core::controller::{Controller, Snapshot, StatusView};
use sbctl_core::doctor::DiagnosticCheck;
use sbctl_core::error::{ExitCode, Result as CoreResult, SbctlError};
use sbctl_core::ip::{IpInfo, fetch_ip_info};
use sbctl_core::layout::Layout;
use sbctl_core::logs::{LogLevel, read_appended_lines, read_tail_lines};
use sbctl_core::profile::{Profile, SKELETON_JSON};
use sbctl_core::service::{Health, RunState};

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
const BACKGROUND_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30);
const POLL_LOG_INTERVAL: Duration = Duration::from_millis(800);
const SNAPSHOT_TICK_INTERVAL: Duration = Duration::from_secs(1);
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(150);

/// Where a piece of feedback belongs on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeOrigin {
    /// The profile rail and its switch actions.
    #[allow(dead_code)]
    Rail,
    /// The stage header's Switch action.
    Stage,
    /// The toolbar connection control.
    Toolbar,
    /// The editor's pinned action bar.
    Editor,
    /// The log screen controls.
    Logs,
    /// The doctor screen.
    Doctor,
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

/// What the setup region should say, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupCondition {
    /// Nothing to resolve.
    None,
    /// The controller could not be created; the app cannot see the service.
    InitFailed(String),
    /// The active configuration is broken and is shown with its real reason.
    Broken(String),
    /// A blocking Doctor check, named as Doctor names it.
    Blocked(String),
    /// No profiles at all.
    EmptyLibrary,
    /// Profiles exist but none is ready; carries their names.
    Unfinished(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerTone {
    Info,
    Success,
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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum EditorValidationState {
    #[default]
    Idle,
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
    pub window_open: bool,
    pub log_revision: u64,
    pub disk_revision: u64,
    pub connected_since: Option<Instant>,
    pub last_checked: Option<Instant>,
    pub ip_info: Option<IpInfo>,
    pub ip_loading: bool,
    pub ip_error: Option<String>,
    /// Bumped on every request so a reply from an older egress is discarded.
    ip_generation: u64,
    /// A refresh was asked for while one was already running.
    ip_refresh_pending: bool,
    /// Dispatch counter for Doctor runs; stale results are discarded so a slow
    /// start-up pass cannot overwrite the checks a repair just produced.
    doctor_generation: u64,
    /// Dispatch counter for live validation; a late reply for older content is
    /// dropped so "Configuration valid" can never describe text it never saw.
    validation_generation: u64,
    pub busy_phase: Option<String>,
    /// Feedback shown next to the control that produced it. Replaces the old
    /// full-width banner, which scrolled out of view and mixed unrelated
    /// messages together.
    pub notice: Option<Notice>,
    pub editor_validation: Option<(String, EditorValidationState)>,
    pub doctor_checks: Vec<DiagnosticCheck>,
    pub doctor_running: bool,
    pub selected_tab: NavTab,
    pub selected_profile: Option<String>,
    /// True while the Connections stage is showing the editor for the selected profile.
    pub editing: bool,
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
    error_log_id: Option<sbctl_core::logs::LogFileId>,
    /// Bumped by `reload_logs` so an in-flight poll cannot append a delta
    /// computed against the offsets that reload just cleared.
    log_generation: u64,
    access_log_id: Option<sbctl_core::logs::LogFileId>,
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
    _watcher: Option<RecommendedWatcher>,
    watcher_task: Option<Task<()>>,
    _daemon_tasks: Vec<Task<()>>,
    snapshot_task: Option<Task<()>>,
    log_poll_task: Option<Task<()>>,
    ip_task: Option<Task<()>>,
    doctor_task: Option<Task<()>>,
    validation_task: Option<Task<()>>,
    mutation_task: Option<Task<()>>,
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
            placeholders: Vec::new(),
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
                c.set_notifications_enabled(settings.desktop_notifications);
                let c = Arc::new(c);
                let snap = c.snapshot().unwrap_or_else(|_| empty_snapshot());
                (Some(c), None, snap)
            }
            Err(err) => (None, Some(err.message.clone()), empty_snapshot()),
        };

        let initially_running = snapshot.running;
        let selected_profile = snapshot
            .active
            .clone()
            .or_else(|| snapshot.profiles.first().map(|p| p.name.clone()));

        let mut store = Self {
            ctrl,
            init_error: init_error.clone(),
            snapshot,
            settings,
            window_open: false,
            log_revision: 0,
            disk_revision: 0,
            connected_since: initially_running.then(Instant::now),
            last_checked: Some(Instant::now()),
            ip_info: None,
            ip_loading: false,
            ip_error: None,
            ip_generation: 0,
            ip_refresh_pending: false,
            doctor_generation: 0,
            validation_generation: 0,
            busy_phase: None,
            notice: init_error.map(|m| Notice {
                origin: NoticeOrigin::Toolbar,
                tone: BannerTone::Warning,
                message: m,
            }),
            editor_validation: None,
            doctor_checks: Vec::new(),
            doctor_running: false,
            selected_tab: NavTab::Profiles,
            editing: false,
            selected_profile,
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
            was_running_expected: initially_running,
            stop_requested: false,
            log_poll_in_flight: false,
            snapshot_poll_in_flight: false,
            _watcher: None,
            watcher_task: None,
            _daemon_tasks: Vec::new(),
            snapshot_task: None,
            log_poll_task: None,
            ip_task: None,
            doctor_task: None,
            validation_task: None,
            mutation_task: None,
        };

        store.reload_logs();
        store.restart_file_watcher(cx);
        store.start_background_pollers(cx);
        // Respect the setting at launch too, not only after a switch; the
        // toggle's own description promises this.
        if store.settings.auto_refresh_ip {
            store.refresh_ip(cx);
        }
        store.run_doctor(cx);
        store
    }

    pub fn tray_visual_state(&self) -> TrayVisualState {
        if self.busy_phase.is_some() {
            TrayVisualState::Transitioning
        } else if self.snapshot.broken.is_some() || self.snapshot.health.state == RunState::Error {
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
        let text = if let Some(phase) = &self.busy_phase {
            format!("sbctl · {phase}")
        } else if let Some(broken) = &self.snapshot.broken {
            format!("sbctl · {broken}")
        } else if self.snapshot.health.state == RunState::Error {
            "sbctl · Service failed · Open Doctor".to_string()
        } else if !self.snapshot.running {
            let needs_setup = self
                .selected_profile
                .as_ref()
                .and_then(|n| self.snapshot.profiles.iter().find(|p| &p.name == n))
                .is_some_and(|p| !p.ready());
            match (&self.snapshot.active, needs_setup) {
                (Some(name), true) => format!("sbctl · {name} needs setup"),
                (Some(name), false) => format!("sbctl · Disconnected · {name} selected"),
                (None, _) => "sbctl · Disconnected".to_string(),
            }
        } else {
            match &self.snapshot.active {
                Some(name) => format!("sbctl · Connected to {name}"),
                None => "sbctl · Connected".to_string(),
            }
        };
        if text.chars().count() > CAP {
            text.chars().take(CAP - 1).collect::<String>() + "…"
        } else {
            text
        }
    }

    pub fn layout(&self) -> Layout {
        self.ctrl
            .as_ref()
            .map(|c| c.layout.clone())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    Layout::windows()
                } else {
                    Layout::darwin()
                }
            })
    }

    pub fn log_counts(&self) -> (usize, usize) {
        let mut errors = 0usize;
        let mut warnings = 0usize;
        for rec in &self.error_logs {
            match rec.level {
                LogLevel::Error => errors += 1,
                LogLevel::Warn => warnings += 1,
                _ => {}
            }
        }
        (errors, warnings)
    }

    /// Which setup condition, if any, the user needs to resolve. Ordered by
    /// severity: a failure always wins over a welcome, and none of these is
    /// suppressed by `onboarding_dismissed` except the welcome itself.
    pub fn setup_condition(&self) -> SetupCondition {
        if let Some(err) = &self.init_error {
            return SetupCondition::InitFailed(err.clone());
        }
        if let Some(reason) = &self.snapshot.broken {
            return SetupCondition::Broken(reason.clone());
        }
        let has_profiles = !self.snapshot.profiles.is_empty();
        let any_ready = self.snapshot.profiles.iter().any(Profile::ready);
        let blocking = self
            .doctor_checks
            .iter()
            .find(|c| (!c.ok && !c.warning) || (c.name == "helper service" && !c.ok));

        if let Some(check) = blocking {
            return SetupCondition::Blocked(check.name.clone());
        }
        if !has_profiles {
            return SetupCondition::EmptyLibrary;
        }
        if !any_ready {
            let names: Vec<String> = self
                .snapshot
                .profiles
                .iter()
                .filter(|p| !p.ready())
                .map(|p| p.name.clone())
                .collect();
            return SetupCondition::Unfinished(names);
        }
        SetupCondition::None
    }

    pub fn set_window_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let was_open = self.window_open;
        self.window_open = open;
        if open && !was_open {
            self.refresh_snapshot(false, cx);
            if self.selected_tab == NavTab::Logs && self.log_follow {
                self.poll_logs_async(cx);
            }
        }
    }

    pub fn update_window_size(&mut self, width: f32, height: f32) {
        let w = width.max(MIN_WINDOW_WIDTH);
        let h = height.max(MIN_WINDOW_HEIGHT);
        if (w - self.settings.window_width).abs() >= WINDOW_SIZE_EPSILON
            || (h - self.settings.window_height).abs() >= WINDOW_SIZE_EPSILON
        {
            self.settings.window_width = w;
            self.settings.window_height = h;
            // Window geometry is not a user action; a failure here should not
            // raise a notice mid-interaction.
            let _ = self.settings.save();
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
            if tab == NavTab::Logs && self.log_follow {
                self.poll_logs_async(cx);
            }
            cx.notify();
        }
    }

    pub fn set_log_stream(&mut self, stream: LogStreamKind, cx: &mut Context<Self>) {
        if self.log_stream != stream {
            self.log_stream = stream;
            if self.log_follow {
                self.poll_logs_async(cx);
            }
            cx.notify();
        }
    }

    pub fn set_log_follow(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.log_follow = follow;
        if follow && self.selected_tab == NavTab::Logs {
            self.poll_logs_async(cx);
        }
        cx.notify();
    }

    pub fn clear_logs(&mut self, cx: &mut Context<Self>) {
        match self.log_stream {
            LogStreamKind::Error => self.error_logs.clear(),
            LogStreamKind::Access => self.access_logs.clear(),
        }
        self.log_revision = self.log_revision.wrapping_add(1);
        cx.notify();
    }

    pub fn select_profile(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.selected_profile = name;
        self.editing = false;
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
        self.editing = false;
        self.editor_validation = None;
        cx.notify();
    }

    pub fn clear_editor_validation(&mut self, profile: &str, cx: &mut Context<Self>) {
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

    /// Convenience for the common "copied" case, which reports on its button.
    #[allow(dead_code)]
    pub fn note_copied(&mut self, origin: NoticeOrigin, what: &str, cx: &mut Context<Self>) {
        self.set_notice(origin, BannerTone::Success, format!("{what} copied"), cx);
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
    fn persist_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.settings.save() {
            self.set_notice(
                NoticeOrigin::Settings,
                BannerTone::Error,
                format!("Could not save settings: {err}"),
                cx,
            );
        }
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

    pub fn set_desktop_notifications(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.desktop_notifications = enabled;
        if let Some(ctrl) = &self.ctrl {
            ctrl.set_notifications_enabled(enabled);
        }
        self.persist_settings(cx);
        cx.notify();
    }

    pub fn toggle_launch_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match set_launch_at_login(enabled) {
            Ok(()) => {
                self.settings.launch_at_login = enabled;
                self.persist_settings(cx);
                self.set_notice(
                    NoticeOrigin::Settings,
                    BannerTone::Success,
                    if enabled {
                        "Launch at login enabled"
                    } else {
                        "Launch at login disabled"
                    },
                    cx,
                );
            }
            Err(err) => {
                self.set_notice(
                    NoticeOrigin::Toolbar,
                    BannerTone::Error,
                    err.to_string(),
                    cx,
                );
            }
        }
    }

    pub fn read_profile_content(&self, name: &str) -> String {
        if let Some(p) = self.snapshot.profiles.iter().find(|p| p.name == name)
            && let Ok(raw) = std::fs::read_to_string(&p.path)
        {
            return raw;
        }
        SKELETON_JSON.to_string()
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
        self.snapshot = snap;
        self.last_checked = Some(Instant::now());
        if profiles_changed || bump_disk {
            self.disk_revision = self.disk_revision.wrapping_add(1);
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
        if self.snapshot_poll_in_flight && !force_disk_bump {
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
                store.last_checked = Some(Instant::now());
                if let Ok(s) = snap {
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
                            "sing-box stopped unexpectedly. Check Logs or Doctor.",
                        );
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: "sing-box stopped unexpectedly. Check Logs or Doctor."
                                .to_string(),
                        });
                    }
                    // Repaint only when something the window shows has changed.
                    // Notifying on every poll re-ran every render body (and, until
                    // the profile facts were cached, re-read a file) for nothing.
                    if changed || force_disk_bump || unexpected_drop || confirmed_stop {
                        cx.notify();
                    }
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

    pub fn connect_active(&mut self, cx: &mut Context<Self>) {
        // Prefer the selected profile, then the remembered active one, but never
        // an unready profile: activating one replaces a working config with a
        // broken half-finished document.
        let ready = |name: &String| {
            self.snapshot
                .profiles
                .iter()
                .any(|p| &p.name == name && p.ready())
        };
        let target = self
            .selected_profile
            .clone()
            .filter(|n| ready(n))
            .or_else(|| self.snapshot.active.clone().filter(|n| ready(n)))
            .or_else(|| {
                self.snapshot
                    .profiles
                    .iter()
                    .find(|p| p.ready())
                    .map(|p| p.name.clone())
            });
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

    pub fn restart_active(&mut self, cx: &mut Context<Self>) {
        if let Some(active) = self.snapshot.active.clone() {
            self.activate_profile(active, NoticeOrigin::Stage, cx);
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

        self.busy_phase = Some(format!("Activating {name}..."));
        self.notice = None;
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(()) => {
                        store.connected_since = Some(Instant::now());
                        store.was_running_expected = true;
                        store.selected_profile = Some(name.clone());
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Success,
                            message: format!("Connected to {name}"),
                        });
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        if err.code != ExitCode::Service {
                            ctrl.notify_error("sing-box activation failed", &err.message);
                        }
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    // Invalidate first so a late reply from the old egress
                    // cannot be shown as the new one.
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn save_and_activate(&mut self, name: String, content: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Saving and activating {name}..."));
        self.notice = None;
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Success,
                            message: format!("Saved and connected to {name}"),
                        });
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
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    // Invalidate first so a late reply from the old egress
                    // cannot be shown as the new one.
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        });
        self.mutation_task = Some(task);
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
        self.notice = None;
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(()) => {
                        store.connected_since = None;
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Info,
                            message: "sing-box stopped".to_string(),
                        });
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    // Invalidate first so a late reply from the old egress
                    // cannot be shown as the new one.
                    store.invalidate_ip();
                    store.refresh_ip(cx);
                }
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn create_profile(&mut self, name: &str, cx: &mut Context<Self>) -> CoreResult<()> {
        let Some(ctrl) = self.ctrl.clone() else {
            return Err(SbctlError::general(
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported operating system".to_string()),
            ));
        };
        ctrl.gui_create_profile(name)?;
        if let Ok(snap) = ctrl.snapshot() {
            self.apply_snapshot(snap, true);
        }
        self.selected_profile = Some(name.to_string());
        self.selected_tab = NavTab::Profiles;
        self.editor_validation = None;
        self.restart_file_watcher(cx);
        self.set_notice(
            NoticeOrigin::Stage,
            BannerTone::Success,
            format!("Created {name}"),
            cx,
        );
        Ok(())
    }

    pub fn pull_profile(&mut self, name: String, url: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Downloading {name}..."));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(()) => {
                        store.selected_profile = Some(name.clone());
                        store.selected_tab = NavTab::Profiles;
                        store.editor_validation = None;
                        store.restart_file_watcher(cx);
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Success,
                            message: format!("Imported {name}"),
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn sync_profile(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some(format!("Syncing {name}..."));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_sync_one(&name);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(reloaded) => {
                        let note = if reloaded {
                            format!("Synced and reloaded {name}")
                        } else {
                            format!("Synced {name}")
                        };
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Success,
                            message: note,
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn sync_all(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };

        self.busy_phase = Some("Syncing remote profiles...".to_string());
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                            store.notice = Some(Notice {
                                origin: NoticeOrigin::Stage,
                                tone: BannerTone::Warning,
                                message: format!("Synced {synced}; failed: {details}"),
                            });
                        } else if report.synced.is_empty() {
                            store.notice = Some(Notice {
                                origin: NoticeOrigin::Stage,
                                tone: BannerTone::Info,
                                message: "No remote profiles to sync".to_string(),
                            });
                        } else {
                            let synced = count_noun(report.synced.len(), "profile", "profiles");
                            store.notice = Some(Notice {
                                origin: NoticeOrigin::Stage,
                                tone: BannerTone::Success,
                                message: format!("Synced {synced}"),
                            });
                        }
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
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
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(reloaded) => {
                        let msg = if reloaded {
                            format!("Saved {name} and reloaded sing-box")
                        } else if allow_invalid {
                            format!("Saved draft {name}")
                        } else {
                            format!("Saved {name}")
                        };
                        if !allow_invalid {
                            store.editor_validation =
                                Some((name.clone(), EditorValidationState::Valid));
                        }
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Success,
                            message: msg,
                        });
                    }
                    Err(err) => {
                        store.editor_validation = Some((
                            name.clone(),
                            EditorValidationState::Invalid(error_text(&err)),
                        ));
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
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
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Editor,
                            tone: BannerTone::Success,
                            message: "Configuration valid".to_string(),
                        });
                    }
                    Err(err) => {
                        let msg = error_text(&err);
                        store.editor_validation =
                            Some((name, EditorValidationState::Invalid(msg.clone())));
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Editor,
                            tone: BannerTone::Error,
                            message: msg,
                        });
                    }
                }
                cx.notify();
            });
        });
        self.validation_task = Some(task);
    }

    pub fn delete_profile(&mut self, name: String, force: bool, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.busy_phase = Some(format!("Deleting {name}..."));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
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
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Info,
                            message: format!("Deleted {name}"),
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn ensure_profiles_dir(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.busy_phase = Some("Creating profiles directory...".to_string());
        let doctor_generation = self.bump_doctor_generation();
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap, checks) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_ensure_profiles_dir();
                    let snap = ctrl.snapshot().ok();
                    let checks = ctrl.doctor();
                    (res, snap, checks)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                if store.doctor_generation == doctor_generation {
                    store.doctor_checks = checks;
                }
                match res {
                    Ok(()) => {
                        store.restart_file_watcher(cx);
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Success,
                            message: format!("Created {}", store.layout().profiles_dir.display()),
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Stage,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn repair_macos_sudoers(&mut self, root_script: String, cx: &mut Context<Self>) {
        self.run_macos_admin_script(
            "Authorizing sudoers install...",
            root_script,
            "Installed /etc/sudoers.d/sbctl",
            false,
            NoticeOrigin::Doctor,
            cx,
        );
    }

    pub fn repair_macos_logs(&mut self, origin: NoticeOrigin, cx: &mut Context<Self>) {
        let Some(root_script) = sbctl_core::doctor::macos_logs_root_script(&self.layout()) else {
            return;
        };
        self.run_macos_admin_script(
            "Authorizing log permissions...",
            root_script,
            "Updated /var/log/sing-box permissions",
            true,
            origin,
            cx,
        );
    }

    fn run_macos_admin_script(
        &mut self,
        phase: &'static str,
        root_script: String,
        success_msg: &'static str,
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
        let doctor_generation = self.bump_doctor_generation();
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, checks, snap) = cx
                .background_executor()
                .spawn(async move {
                    let apple_script = format!(
                        "do shell script \"{}\" with administrator privileges",
                        root_script.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    let out = std::process::Command::new("osascript")
                        .args(["-e", &apple_script])
                        .output();
                    let checks = ctrl.doctor();
                    let snap = ctrl.snapshot().ok();
                    (out, checks, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                if store.doctor_generation == doctor_generation {
                    store.doctor_checks = checks;
                }
                if let Some(s) = snap {
                    store.apply_snapshot(s, false);
                }
                match res {
                    Ok(out) if out.status.success() => {
                        if reload_logs_after {
                            store.reload_logs();
                        }
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Success,
                            message: success_msg.to_string(),
                        });
                    }
                    Ok(out) => {
                        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        let banner = if msg.is_empty() {
                            "Administrator authorization failed".to_string()
                        } else {
                            msg
                        };
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Error,
                            message: banner,
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin,
                            tone: BannerTone::Error,
                            message: err.to_string(),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    pub fn repair_windows_helper(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.require_ctrl(cx) else {
            return;
        };
        self.busy_phase = Some("Starting sbctl-agent...".to_string());
        let doctor_generation = self.bump_doctor_generation();
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, checks, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.gui_repair_windows_helper();
                    let checks = ctrl.doctor();
                    let snap = ctrl.snapshot().ok();
                    (res, checks, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                if store.doctor_generation == doctor_generation {
                    store.doctor_checks = checks;
                }
                if let Some(s) = snap {
                    store.apply_snapshot(s, true);
                }
                match res {
                    Ok(()) => {
                        store.restart_file_watcher(cx);
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Doctor,
                            tone: BannerTone::Success,
                            message: "sbctl-agent and profiles directory ready".to_string(),
                        });
                    }
                    Err(err) => {
                        store.notice = Some(Notice {
                            origin: NoticeOrigin::Doctor,
                            tone: BannerTone::Error,
                            message: error_text(&err),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.mutation_task = Some(task);
    }

    /// Drop the cached egress because the tunnel is about to change under it.
    ///
    /// Called on a successful connect or switch. Without this a reply from the
    /// previous egress can land afterwards and be presented as the current one.
    pub fn invalidate_ip(&mut self) {
        self.ip_generation = self.ip_generation.wrapping_add(1);
        self.ip_info = None;
        self.ip_refresh_pending = true;
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

    /// Next Doctor generation. Repair paths call this so their fresh checks
    /// replace, and are not replaced by, anything already in flight.
    fn bump_doctor_generation(&mut self) -> u64 {
        self.doctor_generation = self.doctor_generation.wrapping_add(1);
        self.doctor_generation
    }

    pub fn run_doctor(&mut self, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };
        if self.doctor_running {
            return;
        }
        self.doctor_running = true;
        let generation = self.bump_doctor_generation();
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let checks = cx
                .background_executor()
                .spawn(async move { ctrl.doctor() })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.doctor_running = false;
                if store.doctor_generation == generation {
                    store.doctor_checks = checks;
                }
                cx.notify();
            });
        });
        self.doctor_task = Some(task);
    }

    pub fn reload_logs(&mut self) {
        let layout = self.layout();
        self.error_logs.clear();
        self.access_logs.clear();
        self.error_log_offset = 0;
        self.access_log_offset = 0;
        self.log_generation = self.log_generation.wrapping_add(1);
        // Refresh identity so the next poll compares against the file we just
        // read rather than the one that existed before a rotation.
        self.error_log_id = sbctl_core::logs::log_file_id(&layout.error_log_path);
        self.access_log_id = sbctl_core::logs::log_file_id(&layout.access_log_path);

        load_tail_stream(
            &layout.error_log_path,
            &mut self.error_logs,
            &mut self.error_log_offset,
            &mut self.error_log_err,
        );
        load_tail_stream(
            &layout.access_log_path,
            &mut self.access_logs,
            &mut self.access_log_offset,
            &mut self.access_log_err,
        );
        self.log_revision = self.log_revision.wrapping_add(1);
    }

    pub fn poll_logs_async(&mut self, cx: &mut Context<Self>) {
        if self.log_poll_in_flight {
            return;
        }
        self.log_poll_in_flight = true;

        let layout = self.layout();
        let err_path = layout.error_log_path.clone();
        let acc_path = layout.access_log_path.clone();
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
                        let err_id_now = sbctl_core::logs::log_file_id(&err_path);
                        let acc_id_now = sbctl_core::logs::log_file_id(&acc_path);
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
        self._watcher = None;
        self.watcher_task = None;

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

        if let Ok(mut w) = watcher {
            for dir in dirs {
                let _ = w.watch(&dir, RecursiveMode::NonRecursive);
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
                                && (store.window_open || store.selected_tab == NavTab::Logs)
                                && store.log_follow
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
        let log_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(POLL_LOG_INTERVAL).await;
                let keep_going = this
                    .update(cx, |store, cx| {
                        if store.window_open
                            && store.selected_tab == NavTab::Logs
                            && store.log_follow
                        {
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

        if self.ctrl.is_some() {
            let snap_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut last_poll = Instant::now();
                loop {
                    cx.background_executor().timer(SNAPSHOT_TICK_INTERVAL).await;
                    let keep_going = this
                        .update(cx, |store, cx| {
                            let interval = if store.window_open {
                                ACTIVE_SNAPSHOT_INTERVAL
                            } else {
                                BACKGROUND_SNAPSHOT_INTERVAL
                            };
                            if store.busy_phase.is_none() && last_poll.elapsed() >= interval {
                                last_poll = Instant::now();
                                store.refresh_snapshot(false, cx);
                                if store.window_open && store.log_follow {
                                    store.poll_logs_async(cx);
                                }
                            }
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

/// An error message for the window, including the hints the CLI also prints.
///
/// `Display` is the message alone, so rollback outcomes ("no previous profile",
/// "could not restore X", "reverted to X but restart failed") all reached the UI
/// as the same generic line.
fn error_text(err: &sbctl_core::error::SbctlError) -> String {
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
    // and every consumer (list, search, copy, detail) sees clean text.
    let line = sbctl_core::logs::strip_ansi(&line);
    let level = LogLevel::classify(&line);
    LogRecord {
        id: next_log_id(),
        line: SharedString::from(line),
        level,
    }
}

fn push_bounded_logs(buf: &mut Vec<LogRecord>, lines: Vec<String>) {
    // Delegated to core so the eviction logic is covered by tests that run on
    // every platform, not only where the GUI crate compiles.
    sbctl_core::logs::append_bounded(buf, lines, MAX_LOG_ENTRIES, into_log_record);
}

pub(crate) fn count_noun(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}
