use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AsyncApp, Context, SharedString, Task, WeakEntity};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use sbctl_core::controller::{Controller, Snapshot, StatusView};
use sbctl_core::doctor::DiagnosticCheck;
use sbctl_core::ip::{IpInfo, fetch_ip_info};
use sbctl_core::layout::Layout;
use sbctl_core::logs::{LogLevel, read_appended_lines, read_tail_lines};
use sbctl_core::profile::SKELETON_JSON;
use sbctl_core::service::{Health, RunState};

use crate::actions::NavTab;
use crate::icons::TrayVisualState;
use crate::shell::{is_launch_at_login_enabled, set_launch_at_login};
use crate::theme::{ThemePreference, UiSettings, apply_theme};

const MAX_LOG_ENTRIES: usize = 5_000;
const INITIAL_LOG_TAIL: usize = 300;
const ACTIVE_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30);
const POLL_LOG_INTERVAL: Duration = Duration::from_millis(800);

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
            Self::Info => level == LogLevel::Info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogRecord {
    pub line: SharedString,
    pub level: LogLevel,
}

pub struct AppStore {
    pub ctrl: Option<Arc<Controller>>,
    pub init_error: Option<String>,
    pub snapshot: Snapshot,
    pub settings: UiSettings,
    pub window_open: bool,
    pub log_revision: u64,
    pub disk_revision: u64,
    pub ip_info: Option<IpInfo>,
    pub ip_loading: bool,
    pub ip_error: Option<String>,
    pub busy_phase: Option<String>,
    pub banner: Option<(BannerTone, String)>,
    pub doctor_checks: Vec<DiagnosticCheck>,
    pub doctor_running: bool,
    pub selected_tab: NavTab,
    pub selected_profile: Option<String>,
    pub log_stream: LogStreamKind,
    pub log_follow: bool,
    pub log_severity: LogSeverityFilter,
    pub log_search_query: String,
    pub profile_search_query: String,
    pub error_logs: Vec<LogRecord>,
    pub access_logs: Vec<LogRecord>,
    error_log_offset: u64,
    access_log_offset: u64,
    log_poll_in_flight: bool,
    snapshot_poll_in_flight: bool,
    _watcher: Option<RecommendedWatcher>,
    _tasks: Vec<Task<()>>,
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

        let (ctrl, init_error, snapshot) = match Controller::from_system(None) {
            Ok(c) => {
                let c = Arc::new(c);
                let snap = c.snapshot().unwrap_or_else(|_| empty_snapshot());
                (Some(c), None, snap)
            }
            Err(err) => {
                let msg = err.message.clone();
                (None, Some(msg), empty_snapshot())
            }
        };

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
            ip_info: None,
            ip_loading: false,
            ip_error: None,
            busy_phase: None,
            banner: init_error.map(|m| (BannerTone::Warning, m)),
            doctor_checks: Vec::new(),
            doctor_running: false,
            selected_tab: NavTab::Dashboard,
            selected_profile,
            log_stream: LogStreamKind::Error,
            log_follow: true,
            log_severity: LogSeverityFilter::All,
            log_search_query: String::new(),
            profile_search_query: String::new(),
            error_logs: Vec::new(),
            access_logs: Vec::new(),
            error_log_offset: 0,
            access_log_offset: 0,
            log_poll_in_flight: false,
            snapshot_poll_in_flight: false,
            _watcher: None,
            _tasks: Vec::new(),
        };

        store.reload_logs();
        store.start_file_watcher(cx);
        store.start_background_pollers(cx);
        store.refresh_ip(cx);
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

    pub fn tray_tooltip(&self) -> String {
        if let Some(phase) = &self.busy_phase {
            return format!("sbctl - {phase}");
        }
        if let Some(broken) = &self.snapshot.broken {
            return format!("sbctl - {broken}");
        }
        match (&self.snapshot.running, &self.snapshot.active) {
            (true, Some(name)) => format!("sbctl - Connected ({name})"),
            (true, None) => "sbctl - Connected".to_string(),
            (false, Some(name)) => format!("sbctl - Disconnected (active: {name})"),
            (false, None) => "sbctl - Disconnected".to_string(),
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
        let w = width.max(820.0);
        let h = height.max(540.0);
        if (w - self.settings.window_width).abs() >= 2.0
            || (h - self.settings.window_height).abs() >= 2.0
        {
            self.settings.window_width = w;
            self.settings.window_height = h;
            let _ = self.settings.save();
        }
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
        cx.notify();
    }

    pub fn dismiss_banner(&mut self, cx: &mut Context<Self>) {
        self.banner = None;
        cx.notify();
    }

    pub fn set_banner(&mut self, tone: BannerTone, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.banner = Some((tone, msg.into()));
        cx.notify();
    }

    pub fn set_theme_preference(&mut self, pref: ThemePreference, cx: &mut Context<Self>) {
        self.settings.theme = pref;
        let _ = self.settings.save();
        apply_theme(pref, None, cx);
        cx.notify();
    }

    pub fn set_show_profile_in_menubar(&mut self, show: bool, cx: &mut Context<Self>) {
        self.settings.show_profile_in_menubar = show;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn set_auto_refresh_ip(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.auto_refresh_ip = enabled;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.settings.sidebar_collapsed = collapsed;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_launch_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match set_launch_at_login(enabled) {
            Ok(()) => {
                self.settings.launch_at_login = enabled;
                let _ = self.settings.save();
                self.set_banner(
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
                self.set_banner(BannerTone::Error, err, cx);
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
                .spawn(async move { ctrl.snapshot() })
                .await;
            let _ = this.update(cx, |store, cx| {
                store.snapshot_poll_in_flight = false;
                if let Ok(s) = snap {
                    let changed = s != store.snapshot;
                    let profiles_changed = s.profiles != store.snapshot.profiles;
                    if changed || force_disk_bump {
                        store.snapshot = s;
                        if profiles_changed || force_disk_bump {
                            store.disk_revision = store.disk_revision.wrapping_add(1);
                        }
                        let prev_sel = store.selected_profile.clone();
                        if prev_sel.is_none()
                            || !prev_sel.as_ref().is_some_and(|n| {
                                store.snapshot.profiles.iter().any(|p| &p.name == n)
                            })
                        {
                            store.selected_profile = store.snapshot.active.clone().or_else(|| {
                                store.snapshot.profiles.first().map(|p| p.name.clone())
                            });
                        }
                        cx.notify();
                    }
                }
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn toggle_service(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        if self.snapshot.running {
            self.disconnect(cx);
        } else {
            self.connect_active(cx);
        }
    }

    pub fn connect_active(&mut self, cx: &mut Context<Self>) {
        let target = self.snapshot.active.clone().or_else(|| {
            self.snapshot
                .profiles
                .iter()
                .find(|p| p.ready())
                .map(|p| p.name.clone())
        });
        let Some(name) = target else {
            self.selected_tab = NavTab::Profiles;
            self.set_banner(
                BannerTone::Warning,
                "No ready profile available. Create or finish setting up a profile first.",
                cx,
            );
            return;
        };
        self.activate_profile(name, cx);
    }

    pub fn activate_profile(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            self.set_banner(
                BannerTone::Warning,
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported OS".to_string()),
                cx,
            );
            return;
        };

        self.busy_phase = Some(format!("Activating {name}..."));
        self.banner = None;
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let worker = cx.background_executor().spawn(async move {
                let res = ctrl.gui_activate(&target, |phase| {
                    let _ = phase_tx.send_blocking(phase.label().to_string());
                });
                let snap = ctrl.snapshot().ok();
                (res, snap)
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
                    store.snapshot = s;
                }
                match res {
                    Ok(()) => {
                        store.selected_profile = Some(name.clone());
                        store.banner = Some((BannerTone::Success, format!("Connected to {name}")));
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.refresh_ip(cx);
                }
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn save_and_activate(&mut self, name: String, content: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            self.set_banner(
                BannerTone::Warning,
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported OS".to_string()),
                cx,
            );
            return;
        };

        self.busy_phase = Some(format!("Saving and activating {name}..."));
        self.banner = None;
        cx.notify();

        let (phase_tx, phase_rx) = smol::channel::unbounded();
        let target = name.clone();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let worker = cx.background_executor().spawn(async move {
                // Save validates first (allow_invalid = false), then activate verifies health.
                match ctrl.gui_save_profile(&target, &content, false) {
                    Ok(true) => {
                        let snap = ctrl.snapshot().ok();
                        (Ok(()), snap)
                    }
                    Ok(false) => {
                        let res = ctrl.gui_activate(&target, |phase| {
                            let _ = phase_tx.send_blocking(phase.label().to_string());
                        });
                        let snap = ctrl.snapshot().ok();
                        (res, snap)
                    }
                    Err(err) => {
                        let snap = ctrl.snapshot().ok();
                        (Err(err), snap)
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
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
                }
                match res {
                    Ok(()) => {
                        store.selected_profile = Some(name.clone());
                        store.banner = Some((
                            BannerTone::Success,
                            format!("Saved and connected to {name}"),
                        ));
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.refresh_ip(cx);
                }
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            self.set_banner(
                BannerTone::Warning,
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported OS".to_string()),
                cx,
            );
            return;
        };

        self.busy_phase = Some("Stopping sing-box service...".to_string());
        self.banner = None;
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
                    store.snapshot = s;
                }
                match res {
                    Ok(()) => {
                        store.banner = Some((BannerTone::Info, "sing-box stopped".to_string()));
                        should_refresh_ip = store.settings.auto_refresh_ip;
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
                if should_refresh_ip {
                    store.refresh_ip(cx);
                }
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn create_profile(&mut self, name: String, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(ctrl) = self.ctrl.clone() else {
            return Err(self
                .init_error
                .clone()
                .unwrap_or_else(|| "Unsupported OS".to_string()));
        };
        match ctrl.create_from_skeleton(&name) {
            Ok(_) => {
                if let Ok(snap) = ctrl.snapshot() {
                    self.snapshot = snap;
                }
                self.disk_revision = self.disk_revision.wrapping_add(1);
                self.selected_profile = Some(name.clone());
                self.selected_tab = NavTab::Profiles;
                self.set_banner(
                    BannerTone::Success,
                    format!("Created profile {name} from skeleton"),
                    cx,
                );
                Ok(())
            }
            Err(err) => Err(err.to_string()),
        }
    }

    pub fn pull_profile(&mut self, name: String, url: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            self.set_banner(
                BannerTone::Warning,
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported OS".to_string()),
                cx,
            );
            return;
        };

        self.busy_phase = Some(format!("Downloading {name}..."));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.pull_profile(&name, &url);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
                }
                match res {
                    Ok(_) => {
                        store.selected_profile = Some(name.clone());
                        store.selected_tab = NavTab::Profiles;
                        store.banner =
                            Some((BannerTone::Success, format!("Imported profile {name}")));
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn sync_profile(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };

        self.busy_phase = Some(format!("Syncing {name}..."));
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.sync_one(&name);
                    let snap = ctrl.snapshot().ok();
                    (name, res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
                }
                match res {
                    Ok(reloaded) => {
                        let note = if reloaded {
                            format!("Synced {name} and reloaded active service")
                        } else {
                            format!("Synced {name}")
                        };
                        store.banner = Some((BannerTone::Success, note));
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn sync_all(&mut self, cx: &mut Context<Self>) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };

        self.busy_phase = Some("Syncing remote profiles...".to_string());
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (res, snap) = cx
                .background_executor()
                .spawn(async move {
                    let res = ctrl.sync_all();
                    let snap = ctrl.snapshot().ok();
                    (res, snap)
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.busy_phase = None;
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
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
                            store.banner = Some((
                                BannerTone::Warning,
                                format!(
                                    "Synced {} profile(s); failed: {details}",
                                    report.synced.len()
                                ),
                            ));
                        } else if report.synced.is_empty() {
                            store.banner =
                                Some((BannerTone::Info, "No remote profiles to sync".to_string()));
                        } else {
                            store.banner = Some((
                                BannerTone::Success,
                                format!("Synced {} remote profile(s)", report.synced.len()),
                            ));
                        }
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn save_profile_content(
        &mut self,
        name: String,
        content: String,
        allow_invalid: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy_phase.is_some() {
            return;
        }
        let Some(ctrl) = self.ctrl.clone() else {
            self.set_banner(
                BannerTone::Warning,
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "Unsupported OS".to_string()),
                cx,
            );
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
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
                }
                match res {
                    Ok(reloaded) => {
                        let msg = if reloaded {
                            format!("Saved {name} and reloaded sing-box")
                        } else {
                            format!("Saved {name}")
                        };
                        store.banner = Some((BannerTone::Success, msg));
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn validate_live_content(&mut self, name: String, content: String, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = cx
                .background_executor()
                .spawn(async move { ctrl.check_json_content(&name, &content) })
                .await;
            let _ = this.update(cx, |store, cx| {
                match res {
                    Ok(()) => {
                        store.banner = Some((
                            BannerTone::Success,
                            "Current buffer is valid under sing-box check".to_string(),
                        ));
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn delete_profile(&mut self, name: String, force: bool, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
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
                store.disk_revision = store.disk_revision.wrapping_add(1);
                if let Some(s) = snap {
                    store.snapshot = s;
                }
                match res {
                    Ok(_) => {
                        if store.selected_profile.as_deref() == Some(name.as_str()) {
                            store.selected_profile = store.snapshot.active.clone().or_else(|| {
                                store.snapshot.profiles.first().map(|p| p.name.clone())
                            });
                        }
                        store.banner = Some((BannerTone::Info, format!("Deleted {name}")));
                    }
                    Err(err) => {
                        store.banner = Some((BannerTone::Error, format!("{err}")));
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn refresh_ip(&mut self, cx: &mut Context<Self>) {
        if self.ip_loading {
            return;
        }
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
                match res {
                    Ok(info) => {
                        store.ip_info = Some(info);
                        store.ip_error = None;
                    }
                    Err(err) => {
                        store.ip_error = Some(err.message.clone());
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn run_doctor(&mut self, cx: &mut Context<Self>) {
        let Some(ctrl) = self.ctrl.clone() else {
            return;
        };
        if self.doctor_running {
            return;
        }
        self.doctor_running = true;
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let checks = cx
                .background_executor()
                .spawn(async move { ctrl.doctor() })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.doctor_running = false;
                store.doctor_checks = checks;
                cx.notify();
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn reload_logs(&mut self) {
        let layout = self.layout();
        self.error_logs.clear();
        self.access_logs.clear();
        self.error_log_offset = 0;
        self.access_log_offset = 0;

        if let Ok((lines, off)) = read_tail_lines(&layout.error_log_path, INITIAL_LOG_TAIL) {
            self.error_log_offset = off;
            self.error_logs = lines.into_iter().map(into_log_record).collect();
        }
        if let Ok((lines, off)) = read_tail_lines(&layout.access_log_path, INITIAL_LOG_TAIL) {
            self.access_log_offset = off;
            self.access_logs = lines.into_iter().map(into_log_record).collect();
        }
        self.log_revision = self.log_revision.wrapping_add(1);
    }

    pub fn poll_logs_async(&mut self, cx: &mut Context<Self>) {
        if self.log_poll_in_flight {
            return;
        }
        self.log_poll_in_flight = true;

        let layout = self.layout();
        let err_off = self.error_log_offset;
        let acc_off = self.access_log_offset;

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (err_res, acc_res) = cx
                .background_executor()
                .spawn(async move {
                    (
                        read_appended_lines(&layout.error_log_path, err_off),
                        read_appended_lines(&layout.access_log_path, acc_off),
                    )
                })
                .await;

            let _ = this.update(cx, |store, cx| {
                store.log_poll_in_flight = false;
                let mut changed = false;

                if let Ok((lines, new_off, truncated)) = err_res {
                    if truncated || new_off < store.error_log_offset {
                        store.error_logs.clear();
                        changed = true;
                    }
                    store.error_log_offset = new_off;
                    if !lines.is_empty() {
                        changed = true;
                        push_bounded_logs(&mut store.error_logs, lines);
                    }
                }
                if let Ok((lines, new_off, truncated)) = acc_res {
                    if truncated || new_off < store.access_log_offset {
                        store.access_logs.clear();
                        changed = true;
                    }
                    store.access_log_offset = new_off;
                    if !lines.is_empty() {
                        changed = true;
                        push_bounded_logs(&mut store.access_logs, lines);
                    }
                }
                if changed {
                    store.log_revision = store.log_revision.wrapping_add(1);
                    cx.notify();
                }
            });
        });
        self._tasks.push(task);
        self.prune_completed_tasks();
    }

    pub fn active_log_records(&self) -> &[LogRecord] {
        match self.log_stream {
            LogStreamKind::Error => &self.error_logs,
            LogStreamKind::Access => &self.access_logs,
        }
    }

    fn start_file_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(ctrl) = &self.ctrl else {
            return;
        };
        let watch_dirs: Vec<PathBuf> = [
            Some(ctrl.layout.profiles_dir.clone()),
            ctrl.layout.active_config_path.parent().map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        .filter(|p| p.exists())
        .collect();

        if watch_dirs.is_empty() {
            return;
        }

        let (tx, rx) = smol::channel::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if res.is_ok() {
                let _ = tx.try_send(());
            }
        });

        if let Ok(mut w) = watcher {
            for dir in watch_dirs {
                let _ = w.watch(&dir, RecursiveMode::NonRecursive);
            }
            self._watcher = Some(w);
            let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                while rx.recv().await.is_ok() {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    while rx.try_recv().is_ok() {}
                    let _ = this.update(cx, |store, cx| {
                        store.refresh_snapshot(true, cx);
                    });
                }
            });
            self._tasks.push(task);
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
        self._tasks.push(log_task);

        if self.ctrl.is_some() {
            let snap_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut last_poll = Instant::now();
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
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
                            }
                        })
                        .is_ok();
                    if !keep_going {
                        break;
                    }
                }
            });
            self._tasks.push(snap_task);
        }
    }

    fn prune_completed_tasks(&mut self) {
        if self._tasks.len() > 24 {
            self._tasks.drain(..self._tasks.len() - 16);
        }
    }
}

fn into_log_record(line: String) -> LogRecord {
    let level = LogLevel::classify(&line);
    LogRecord {
        line: SharedString::from(line),
        level,
    }
}

fn push_bounded_logs(buf: &mut Vec<LogRecord>, lines: Vec<String>) {
    for line in lines {
        buf.push(into_log_record(line));
    }
    if buf.len() > MAX_LOG_ENTRIES {
        let overflow = buf.len() - MAX_LOG_ENTRIES;
        buf.drain(..overflow);
    }
}
