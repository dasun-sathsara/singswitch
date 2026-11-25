use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::activator::{
    Activator, CopyActivator, Rollback, SymlinkActivator, write_file_atomic,
    write_new_file_exclusive,
};
use crate::checker::{Checker, ExecChecker, check_temp_bytes};
use crate::doctor::diagnose_active;
use crate::error::{ExitCode, Result, SingSwitchError, store_error};
use crate::form_model::ProfileFormModel;
use crate::layout::{Layout, PlatformKind};
use crate::lock::AdvisoryLock;
use crate::notify::{Notifier, ToggleableNotifier, default_notifier};
use crate::profile::{Profile, SKELETON_JSON, list_profiles, path_for, validate_name};
use crate::remote::{fetch_remote_profile, normalise_profile_url, validate_remote_body};
use crate::runner::{ExecRunner, Runner, SudoRunner, TraceHook};
use crate::service::launchd::LaunchdManager;
use crate::service::win_rpc::{
    HELPER_SERVICE_NAME, PipeRequest, PipeResponse, try_send_pipe_request,
};
use crate::service::winsw::WinSwManager;
use crate::service::{Health, Manager, RunState};
use crate::source::{read_source, remove_source, sourced_profiles, write_source};

/// Wait after restart before sampling again to catch KeepAlive crash loops.
pub const DEFAULT_SETTLE_DELAY: Duration = Duration::from_millis(2500);

/// Wait for a freshly restarted daemon to report running before capturing the baseline sample.
pub const DEFAULT_START_GRACE: Duration = Duration::from_secs(3);

const START_POLL_INTERVAL: Duration = Duration::from_millis(150);
const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Progress updates emitted during verified profile activation (used by GUI spinners).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationPhase {
    Validating,
    UpdatingConfig,
    RestartingService,
    VerifyingHealth,
    RollingBack,
}

impl ActivationPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Validating => "Validating...",
            Self::UpdatingConfig => "Updating config...",
            Self::RestartingService => "Restarting service...",
            Self::VerifyingHealth => "Checking tunnel...",
            Self::RollingBack => "Rolling back...",
        }
    }
}

/// Consistent point-in-time snapshot of the service and all stored profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub health: Health,
    pub active: Option<String>,
    pub running: bool,
    pub profiles: Vec<Profile>,
    pub broken: Option<String>,
    pub status_view: StatusView,
}

/// Presentation-ready status view derived from a `Snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    pub state: RunState,
    pub profile: Option<String>,
    pub running: bool,
    pub tun: Option<String>,
    pub server: Option<String>,
    pub pid: Option<u32>,
    /// Fields the active profile still needs, for the agent-facing JSON.
    pub missing: Vec<crate::readiness::Field>,
    pub broken: Option<String>,
}

/// Summary returned after syncing one or all remote profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub synced: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Whether a remote refresh actually altered what is on disk.
///
/// The caller needs this to decide whether to touch the running service at all:
/// reloading identical bytes restarts the daemon and drops the tunnel for no
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshOutcome {
    /// The file was created, or its contents changed.
    Changed,
    /// The fetched body already matched the file; nothing was written.
    Unchanged,
}

/// Whether a freshly fetched body differs from the bytes already on disk.
///
/// A missing file always counts as a change, even for an empty body: it still
/// has to be created.
fn body_differs(existing: Option<&[u8]>, fetched: &[u8]) -> bool {
    existing.is_none_or(|current| current != fetched)
}

/// Optional trace callback for `--verbose` diagnostics.
pub type DebugHook = Arc<dyn Fn(&str) + Send + Sync>;

/// High-level orchestrator used by the CLI, the Windows helper service, and the desktop GUI.
///
/// Every dependency is injected behind a trait, so service control can be exercised
/// without root privileges.
pub struct Controller {
    pub layout: Layout,
    pub manager: Arc<dyn Manager>,
    pub activator: Arc<dyn Activator>,
    pub checker: Arc<dyn Checker>,
    pub runner: Arc<dyn Runner>,
    pub notifier: Arc<dyn Notifier>,
    pub notifications_enabled: Arc<AtomicBool>,
    pub skeleton: String,
    pub settle_delay: Duration,
    pub start_grace: Duration,
    pub lock_timeout: Duration,
    pub sleep_fn: Arc<dyn Fn(Duration) + Send + Sync>,
    pub debug_hook: Option<DebugHook>,
}

impl Controller {
    /// Construct a production `Controller` for the host OS.
    pub fn from_system(trace: Option<TraceHook>) -> Result<Self> {
        let layout = Layout::detect()?;
        Ok(Self::for_layout(layout, trace))
    }

    /// Construct a `Controller` wired to real system runners for a given `Layout`.
    pub fn for_layout(layout: Layout, trace: Option<TraceHook>) -> Self {
        let notifier = default_notifier(layout.platform);
        Self::for_layout_with_notifier(layout, trace, notifier)
    }

    /// Construct a `Controller` with a caller-supplied notifier, used by the app
    /// so failure notices come from the system notification center instead of a
    /// subprocess.
    pub fn for_layout_with_notifier(
        layout: Layout,
        trace: Option<TraceHook>,
        notifier: Box<dyn Notifier>,
    ) -> Self {
        let exec_runner: Arc<dyn Runner> = Arc::new(ExecRunner {
            trace,
            ..Default::default()
        });
        let enabled = Arc::new(AtomicBool::new(true));
        let notifier: Arc<dyn Notifier> =
            Arc::new(ToggleableNotifier::new(notifier, enabled.clone()));
        let mut ctrl = Self::with_runner(
            layout,
            exec_runner,
            Arc::new(ExecChecker::default()),
            notifier,
        );
        ctrl.notifications_enabled = enabled;
        ctrl
    }

    /// Construct a `Controller` with an injected `Runner`, `Checker`, and `Notifier`.
    pub fn with_runner(
        layout: Layout,
        runner: Arc<dyn Runner>,
        checker: Arc<dyn Checker>,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        let (manager, activator): (Arc<dyn Manager>, Arc<dyn Activator>) = match layout.platform {
            PlatformKind::Darwin => {
                let sudo: Arc<dyn Runner> = Arc::new(SudoRunner::new(runner.clone()));
                let mgr = Arc::new(LaunchdManager::new(
                    layout.ctl_bin.clone(),
                    layout.launchd_label.clone(),
                    layout.plist_path.clone(),
                    sudo.clone(),
                ));
                // No privileged hook: the installer gives the target user the
                // config directory, so activating is an ordinary atomic symlink
                // swap. It used to run `sudo ln -sfn <profiles>/*.json <config>`,
                // whose NOPASSWD rule accepted `../` traversal.
                let act = Arc::new(SymlinkActivator::new(
                    layout.active_config_path.clone(),
                    None,
                ));
                (mgr, act)
            }
            PlatformKind::Windows => {
                let mgr = Arc::new(WinSwManager::new(
                    layout.service_name.clone(),
                    runner.clone(),
                ));
                let act = Arc::new(CopyActivator::new(
                    layout.active_config_path.clone(),
                    layout
                        .active_name_path
                        .clone()
                        .unwrap_or_else(|| layout.profiles_dir.join("active-profile")),
                ));
                (mgr, act)
            }
        };

        Self {
            layout,
            manager,
            activator,
            checker,
            runner,
            notifier,
            notifications_enabled: Arc::new(AtomicBool::new(true)),
            skeleton: SKELETON_JSON.to_string(),
            settle_delay: DEFAULT_SETTLE_DELAY,
            start_grace: DEFAULT_START_GRACE,
            lock_timeout: LOCK_ACQUIRE_TIMEOUT,
            sleep_fn: Arc::new(std::thread::sleep),
            debug_hook: None,
        }
    }

    pub fn set_notifications_enabled(&self, enabled: bool) {
        self.notifications_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn notify_error(&self, title: &str, message: &str) {
        self.notifier.notify_error(title, message);
    }

    fn debug(&self, msg: impl AsRef<str>) {
        if let Some(hook) = &self.debug_hook {
            hook(msg.as_ref());
        }
    }

    fn is_active_and_running(&self, name: &str) -> (bool, bool) {
        let active = match self.activator.active_name() {
            Ok(active) => active,
            Err(err) => {
                // Callers treat a profile they cannot prove is live as inactive, but
                // say so: an unreadable marker used to look exactly like "none active".
                self.debug(format!("could not read the active profile: {err}"));
                None
            }
        };
        let is_active = active.as_deref() == Some(name);
        let running = is_active
            && self
                .manager
                .probe()
                .is_ok_and(|h| h.state == RunState::Running);
        (is_active, running)
    }

    /// Put the previous profile back. Returns the reasons it could not, so the
    /// caller never announces a successful revert that did not happen.
    fn restore_previous_profile(
        &self,
        path: &std::path::Path,
        old_bytes: &[u8],
        restart: bool,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        if let Err(err) = write_file_atomic(path, old_bytes, 0o644) {
            failures.push(format!("rewrite: {err}"));
        }
        if self.layout.active_name_path.is_some()
            && let Err(err) = self.activator.activate(path)
        {
            failures.push(format!("reactivate: {err}"));
        }
        if restart && let Err(err) = self.manager.restart() {
            failures.push(format!("restart: {err}"));
        }
        failures
    }

    /// Acquire the file lock.
    fn acquire_lock(&self) -> Result<AdvisoryLock> {
        AdvisoryLock::acquire(&self.layout.lock_path(), self.lock_timeout)
    }

    /// Take one atomic snapshot of service health, active profile, and all profiles on disk.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let health = match self.manager.probe() {
            Ok(h) => h,
            Err(err) if err.is_permission() => return Err(err),
            Err(err) => {
                self.debug(format!("could not read service state: {err}"));
                Health::unknown(RunState::Error)
            }
        };
        let running = health.state == RunState::Running;
        let active = match self.activator.active_name() {
            Ok(name) => name,
            Err(err) => {
                self.debug(format!("could not read active profile: {err}"));
                None
            }
        };

        let profiles = list_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;

        let broken = active
            .as_deref()
            .and_then(|name| diagnose_active(&self.layout, name, &profiles));

        let status_view = self.build_status_view(
            &health,
            active.as_deref(),
            running,
            &profiles,
            broken.as_deref(),
        );
        Ok(Snapshot {
            health,
            active,
            running,
            profiles,
            broken,
            status_view,
        })
    }

    fn build_status_view(
        &self,
        health: &Health,
        active: Option<&str>,
        running: bool,
        profiles: &[Profile],
        broken: Option<&str>,
    ) -> StatusView {
        let mut view = StatusView {
            state: health.state,
            profile: active.map(ToString::to_string),
            running,
            tun: None,
            server: None,
            pid: health.pid,
            missing: Vec::new(),
            broken: broken.map(ToString::to_string),
        };
        if let Some(active_name) = active
            && let Some(p) = profiles.iter().find(|p| p.name == active_name)
        {
            view.missing = p.missing.clone();
            view.tun = p.facts.tun_interface.clone();
            view.server = p.facts.endpoint.clone();
        }
        view
    }

    /// Validate `name`, swap the active configuration, restart the service, and confirm
    /// it stays running past the settle delay. Automatically rolls back on crash loops.
    pub fn activate(&self, name: &str) -> Result<()> {
        self.activate_with_progress(name, |_| {})
    }

    /// Same as `activate`, emitting `ActivationPhase` updates for responsive UI feedback.
    pub fn activate_with_progress(
        &self,
        name: &str,
        on_phase: impl FnMut(ActivationPhase),
    ) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.activate_locked(name, on_phase)
    }

    fn activate_locked(&self, name: &str, mut on_phase: impl FnMut(ActivationPhase)) -> Result<()> {
        validate_name(name)?;

        let profiles = list_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;
        let available: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        let target = profiles
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| SingSwitchError::profile_not_found(name, &available))?;

        if !target.ready() {
            return Err(SingSwitchError::profile_incomplete(
                &target.name,
                &target.missing,
            ));
        }

        on_phase(ActivationPhase::Validating);
        self.debug(format!("validating {}", target.path.display()));
        self.checker.check(&target.path)?;

        // Probe first so missing privileges fail before modifying the active config
        // and so rollback knows whether to restart or stop the service.
        let was_running = match self.manager.probe() {
            Ok(before) => {
                self.debug(format!(
                    "state before activation: {} (pid={:?} restarts={:?})",
                    before.state, before.pid, before.restarts
                ));
                before.state == RunState::Running
            }
            Err(err) if err.is_permission() => return Err(err),
            Err(err) => {
                self.debug(format!("pre-activation probe failed: {err}"));
                false
            }
        };

        on_phase(ActivationPhase::UpdatingConfig);
        let rollback = self.activator.activate(&target.path)?;

        on_phase(ActivationPhase::RestartingService);
        if let Err(restart_err) = self.manager.restart() {
            on_phase(ActivationPhase::RollingBack);
            return Err(self.recover(
                rollback.as_ref(),
                was_running,
                Some(restart_err),
                "the sing-box service did not restart",
            ));
        }

        on_phase(ActivationPhase::VerifyingHealth);
        if let Some(problem) = self.verify_healthy() {
            on_phase(ActivationPhase::RollingBack);
            return Err(self.recover(
                rollback.as_ref(),
                was_running,
                None,
                &format!("{} started but {problem}", target.name),
            ));
        }

        // No desktop notification on a routine switch: the app window shows
        // the new active profile, and the tray tooltip carries it too.
        Ok(())
    }

    fn verify_healthy(&self) -> Option<String> {
        let Some(baseline) = self.await_running() else {
            return Some("did not stay up".to_string());
        };
        self.debug(format!(
            "baseline after restart: {} (pid={:?} restarts={:?} last_exit={:?})",
            baseline.state, baseline.pid, baseline.restarts, baseline.last_exit
        ));

        if !self.settle_delay.is_zero() {
            self.debug(format!(
                "waiting {:?} to confirm service stays up",
                self.settle_delay
            ));
            (self.sleep_fn)(self.settle_delay);
        }

        let after = match self.manager.probe() {
            Ok(h) => h,
            Err(err) => {
                // Same rule as `await_running`: an unreadable state is unknown, not
                // healthy. Returning None here reported a switch to a config sing-box
                // may never have loaded.
                self.debug(format!(
                    "could not confirm post-settle service health: {err}"
                ));
                return Some("could not be checked after starting".to_string());
            }
        };
        self.debug(format!(
            "state after settling: {} (pid={:?} restarts={:?} last_exit={:?})",
            after.state, after.pid, after.restarts, after.last_exit
        ));

        after.crashed_since(&baseline)
    }

    fn await_running(&self) -> Option<Health> {
        let deadline = Instant::now() + self.start_grace;
        let mut attempt = 0usize;
        loop {
            match self.manager.probe() {
                Ok(health) => {
                    if health.state == RunState::Running {
                        return Some(health);
                    }
                    if attempt > 0 && Instant::now() >= deadline {
                        self.debug(format!(
                            "service still {} after {:?}",
                            health.state, self.start_grace
                        ));
                        return None;
                    }
                }
                Err(err) => {
                    // A probe error means the state is unknown, not healthy.
                    // Reporting it as running skipped rollback and announced a
                    // switch that may never have taken effect.
                    self.debug(format!("probe error while awaiting start: {err}"));
                    if attempt > 0 && Instant::now() >= deadline {
                        return None;
                    }
                }
            }
            attempt += 1;
            if self.start_grace.is_zero() {
                return None;
            }
            (self.sleep_fn)(START_POLL_INTERVAL);
        }
    }

    fn recover(
        &self,
        rollback: &dyn Rollback,
        was_running: bool,
        cause: Option<SingSwitchError>,
        summary: &str,
    ) -> SingSwitchError {
        let err = self.recover_inner(rollback, was_running, cause, summary);
        self.notifier
            .notify_error("sing-box activation failed", &err.message);
        err
    }

    fn recover_inner(
        &self,
        rollback: &dyn Rollback,
        was_running: bool,
        cause: Option<SingSwitchError>,
        summary: &str,
    ) -> SingSwitchError {
        let mut failure = SingSwitchError::service(summary);
        if let Some(c) = cause {
            failure = failure.with_cause(c);
        }

        if !rollback.known() {
            return match self.manager.stop() {
                Ok(()) => failure.with_hints([
                    "there was no previous profile to fall back to; stopped sing-box",
                    "inspect the service output with: singswitch logs",
                    "fix the profile and re-run: singswitch use <name>",
                ]),
                // Claiming "stopped sing-box" after a failed stop is how a user ends
                // up with a daemon running a config nobody chose.
                Err(stop_err) => failure.with_hints([
                    format!(
                        "there was no previous profile to fall back to, and sing-box could not be stopped: {stop_err}"
                    ),
                    "inspect the service output with: singswitch logs".to_string(),
                    "stop it manually with: singswitch off".to_string(),
                ]),
            };
        }

        let previous = rollback.description();
        if let Err(rb_err) = rollback.rollback() {
            return failure.with_hints([
                format!("could not restore {previous} automatically: {rb_err}"),
                format!("restore it manually with: singswitch use {previous}"),
                "or stop sing-box entirely with: singswitch off".to_string(),
            ]);
        }

        if was_running {
            if let Err(restart_err) = self.manager.restart() {
                return failure.with_hints([
                    format!(
                        "reverted to {previous}, but the service did not restart: {restart_err}"
                    ),
                    "inspect the service output with: singswitch logs".to_string(),
                ]);
            }
            failure.message = format!("{summary}; reverted to {previous}");
        } else {
            failure.message = match self.manager.stop() {
                Ok(()) => format!("{summary}; reverted to {previous} (service kept stopped)"),
                Err(stop_err) => format!(
                    "{summary}; reverted to {previous}, but the service did not stop: {stop_err}"
                ),
            };
        }
        failure.with_hint("inspect the service output with: singswitch logs")
    }

    /// Stop the sing-box service.
    pub fn stop(&self) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.manager.stop()?;
        // No desktop notification on a routine stop (see AGENTS.md).
        Ok(())
    }

    /// Create a brand-new profile from the canonical skeleton template.
    pub fn create_from_skeleton(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        fs::create_dir_all(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "create"))?;

        let dest = path_for(&self.layout.profiles_dir, name);
        write_new_file_exclusive(&dest, self.skeleton.as_bytes(), 0o644).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                SingSwitchError::general(format!("a profile named {name:?} already exists"))
                    .with_hint(format!("edit it with: singswitch edit {name}"))
            } else {
                store_error(err, &self.layout.profiles_dir, "create")
            }
        })?;
        Ok(dest)
    }

    /// Validate `content` with `sing-box check` (unless `allow_invalid` is true), write it
    /// atomically to `name.json`, and reload the service if `name` is currently active and running.
    ///
    /// Returns `Ok(true)` if the live running service was reloaded, `Ok(false)` otherwise.
    pub fn save_profile(&self, name: &str, content: &str, allow_invalid: bool) -> Result<bool> {
        let _lock = self.acquire_lock()?;
        validate_name(name)?;

        let path = path_for(&self.layout.profiles_dir, name);

        // Parse, then check the fields this outbound type needs, then let
        // sing-box decide validity. Running the validator first reported
        // "invalid public_key" for an unfinished profile - true and useless.
        let parsed = serde_json::from_str::<serde_json::Value>(content);
        if let (Ok(tree), false) = (&parsed, allow_invalid) {
            let missing = ProfileFormModel::from_value(tree).missing();
            if !missing.is_empty() {
                return Err(SingSwitchError::profile_incomplete(name, &missing));
            }
        }
        if !allow_invalid {
            check_temp_bytes(
                self.checker.as_ref(),
                "singswitch-validate-",
                content.as_bytes(),
            )?;
        }

        let (is_active, running) = if allow_invalid {
            if self.activator.active_name()?.as_deref() == Some(name) {
                return Err(SingSwitchError::validation(format!(
                    "Cannot save the active profile {name} as a draft. Validate it before saving."
                )));
            }
            (false, false)
        } else {
            self.is_active_and_running(name)
        };

        let old_bytes = fs::read(&path).ok();
        write_file_atomic(&path, content.as_bytes(), 0o644)?;

        if !is_active {
            return Ok(false);
        }

        // Only re-point the active symlink at content that is ready to load.
        let complete = serde_json::from_str::<serde_json::Value>(content)
            .map(|t| ProfileFormModel::from_value(&t).missing().is_empty())
            .unwrap_or(false);
        if self.layout.active_name_path.is_some()
            && complete
            && let Err(err) = self.activator.activate(&path)
        {
            // Putting the previous bytes back can itself fail; a bare swallow
            // left the new (unactivatable) content on disk while the message
            // said nothing about it. The restore note rides the message: the
            // GUI banners print `message` and `hints` only, never `cause`.
            let restore_note = match old_bytes.as_deref() {
                Some(old) => match write_file_atomic(&path, old, 0o644) {
                    Ok(()) => String::new(),
                    Err(restore_err) => {
                        format!("; the previous contents could not be restored: {restore_err}")
                    }
                },
                None => String::new(),
            };
            // The activator's message and hint stay primary; re-wrapping this as
            // a general error replaced them with a directory-ownership one.
            let message = format!("{}{restore_note}", err.message);
            return Err(err.with_message(message));
        }

        if !running {
            return Ok(false);
        }

        self.debug(format!("reloading running service after editing {name}"));
        self.manager.restart()?;
        if let Some(problem) = self.verify_healthy() {
            self.notifier
                .notify_error("sing-box reload failed", &problem);
            if let Some(old) = &old_bytes {
                let failures = self.restore_previous_profile(&path, old, true);
                let message = if failures.is_empty() {
                    format!(
                        "sing-box reloaded {name} but {problem}; reverted {name} to previous contents"
                    )
                } else {
                    // Never claim a revert that did not happen.
                    format!(
                        "sing-box reloaded {name} but {problem}; reverting ALSO failed ({}). The profile may be left in the broken state.",
                        failures.join("; ")
                    )
                };
                return Err(SingSwitchError::service(message)
                    .with_hint("inspect the service output with: singswitch logs"));
            }
            return Err(SingSwitchError::service(format!(
                "sing-box reloaded {name} but {problem}"
            ))
            .with_hints([
                "inspect the service output with: singswitch logs".to_string(),
                format!("revise the profile with: singswitch edit {name}"),
            ]));
        }

        // No desktop notification on a routine reload (see AGENTS.md).
        Ok(true)
    }

    /// Download and store a profile from `raw_url` after validating with `sing-box check`.
    pub fn pull_profile(&self, name: &str, raw_url: &str) -> Result<Profile> {
        let _lock = self.acquire_lock()?;
        validate_name(name)?;
        let url = normalise_profile_url(raw_url)?;
        fs::create_dir_all(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "create"))?;

        let path = path_for(&self.layout.profiles_dir, name);
        if path.exists() {
            return Err(SingSwitchError::general(format!(
                "a profile named {name:?} already exists"
            ))
            .with_hints([
                format!("refresh it from its source with: singswitch sync {name}"),
                format!("or delete it first with: singswitch rm {name}"),
            ]));
        }

        let body = fetch_remote_profile(&url)?;
        validate_remote_body(self.checker.as_ref(), &body)?;

        write_new_file_exclusive(&path, &body, 0o644).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                SingSwitchError::general(format!("a profile named {name:?} already exists"))
                    .with_hint(format!(
                        "refresh it from its source with: singswitch sync {name}"
                    ))
            } else {
                store_error(err, &self.layout.profiles_dir, "write")
            }
        })?;

        if let Err(err) = write_source(&self.layout.profiles_dir, name, &url) {
            let _ = fs::remove_file(&path);
            return Err(store_error(
                err,
                &self.layout.profiles_dir,
                "record the source URL for",
            ));
        }

        let profiles = list_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;
        profiles
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                SingSwitchError::general(format!("pulled {name} but could not read it back"))
            })
    }

    /// Re-fetch one remotely pulled profile, reloading it via verified activation if active.
    pub fn sync_one(&self, name: &str) -> Result<bool> {
        let _lock = self.acquire_lock()?;
        validate_name(name)?;

        let url = read_source(&self.layout.profiles_dir, name)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?
            .ok_or_else(|| {
                SingSwitchError::general(format!("{name:?} has no recorded source URL")).with_hints(
                    [
                        format!("fetch it from a URL first with: singswitch pull {name} <url>"),
                        "sync without a name refreshes every profile that does have one"
                            .to_string(),
                    ],
                )
            })?;

        if self.refresh_profile_locked(name, &url)? == RefreshOutcome::Unchanged {
            // Nothing on disk moved, so re-pointing or restarting the service
            // would only interrupt the tunnel to load bytes it already has.
            return Ok(false);
        }
        let (is_active, running) = self.is_active_and_running(name);
        if !is_active {
            return Ok(false);
        }
        if !running {
            self.repoint_if_marker_platform(&path_for(&self.layout.profiles_dir, name));
            return Ok(false);
        }
        self.activate_locked(name, |_| {})?;
        Ok(true)
    }

    /// Re-point the active config at `target` without restarting, on the copy-based
    /// platform where the marker file has to follow the bytes.
    ///
    /// Best effort on purpose: the service is stopped, so nothing is loading the old
    /// config, and failing the refresh would report a sync that already happened as
    /// failed. It is not silent - the failure is traced for `--verbose` runs.
    fn repoint_if_marker_platform(&self, target: &std::path::Path) {
        if self.layout.active_name_path.is_none() {
            return;
        }
        if let Err(err) = self.activator.activate(target) {
            self.debug(format!(
                "could not re-point the active config at {}: {err}",
                target.display()
            ));
        }
    }

    /// Re-fetch every profile with a `.url` sidecar. Inactive profiles update first;
    /// the active profile updates last through verified activation.
    pub fn sync_all(&self) -> Result<SyncReport> {
        let _lock = self.acquire_lock()?;
        let (names, unreadable_sidecars) = sourced_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;

        let active = match self.activator.active_name() {
            Ok(active) => active,
            Err(err) => {
                self.debug(format!("could not read the active profile: {err}"));
                None
            }
        };
        let mut report = SyncReport {
            synced: Vec::new(),
            failed: Vec::new(),
        };
        // A sidecar that exists but cannot be read means a profile that would
        // sync went unrefreshed; `sourced_profiles` already refused to guess,
        // so the failure is named here rather than dropped from the report.
        for name in unreadable_sidecars {
            report
                .failed
                .push((name, "could not read the recorded source URL".to_string()));
        }

        for name in &names {
            if active.as_deref() == Some(name.as_str()) {
                continue;
            }
            let Ok(url) = read_source(&self.layout.profiles_dir, name) else {
                report.failed.push((
                    name.clone(),
                    "could not read the recorded source URL".to_string(),
                ));
                continue;
            };
            let Some(url) = url else {
                report
                    .failed
                    .push((name.clone(), "missing source URL".to_string()));
                continue;
            };
            // The outcome only matters for the active profile below; an
            // inactive one is never activated, so both outcomes are a success.
            match self.refresh_profile_locked(name, &url) {
                Ok(_) => report.synced.push(name.clone()),
                Err(err) => report.failed.push((name.clone(), err.to_string())),
            }
        }

        if let Some(active_name) = active
            && names.iter().any(|n| n == &active_name)
        {
            let path = path_for(&self.layout.profiles_dir, &active_name);
            let old_bytes = fs::read(&path).ok();
            let source = match read_source(&self.layout.profiles_dir, &active_name) {
                Ok(source) => source,
                Err(err) => {
                    report.failed.push((
                        active_name,
                        format!("could not read the recorded source URL: {err}"),
                    ));
                    return Ok(report);
                }
            };
            if let Some(url) = source {
                match self.refresh_profile_locked(&active_name, &url) {
                    Err(err) => report.failed.push((active_name, err.to_string())),
                    Ok(RefreshOutcome::Unchanged) => report.synced.push(active_name),
                    Ok(RefreshOutcome::Changed) => {
                        let running = self
                            .manager
                            .probe()
                            .is_ok_and(|h| h.state == RunState::Running);
                        if running {
                            if let Err(act_err) = self.activate_locked(&active_name, |_| {}) {
                                let mut detail = act_err.to_string();
                                if let Some(old) = &old_bytes {
                                    let failures = self.restore_previous_profile(&path, old, true);
                                    if !failures.is_empty() {
                                        detail = format!(
                                            "{detail}; reverting also failed ({})",
                                            failures.join("; ")
                                        );
                                    }
                                }
                                report.failed.push((active_name, detail));
                                return Ok(report);
                            }
                        } else {
                            self.repoint_if_marker_platform(&path);
                        }
                        report.synced.push(active_name);
                    }
                }
            }
        }

        Ok(report)
    }

    fn refresh_profile_locked(&self, name: &str, url: &str) -> Result<RefreshOutcome> {
        let path = path_for(&self.layout.profiles_dir, name);
        if !path.exists() {
            return Err(SingSwitchError::general(format!(
                "profile {name:?} is gone; its source URL is orphaned"
            ))
            .with_hint(format!("fetch it again with: singswitch pull {name} {url}")));
        }
        let body = fetch_remote_profile(url)?;
        validate_remote_body(self.checker.as_ref(), &body)?;

        // Compare before writing, not after: `write_file_atomic` replaces the
        // inode, so an unchanged body would still bump the mtime, wake the
        // profiles watcher, and leave the caller unable to tell a real change
        // from a no-op.
        let existing = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            // Vanished between the exists() check and here: recreate it.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(store_error(err, &self.layout.profiles_dir, "read")),
        };
        if !body_differs(existing.as_deref(), &body) {
            return Ok(RefreshOutcome::Unchanged);
        }

        write_file_atomic(&path, &body, 0o644)?;
        Ok(RefreshOutcome::Changed)
    }

    /// Delete a profile, refusing if active & running unless `force` is true (which stops
    /// sing-box first so it is never left running against a missing file).
    ///
    /// Returns `Ok(is_active_warning)` when deleted.
    pub fn delete_profile(&self, name: &str, force: bool) -> Result<bool> {
        let _lock = self.acquire_lock()?;
        validate_name(name)?;

        let profiles = list_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;
        let available: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        if !profiles.iter().any(|p| p.name == name) {
            return Err(SingSwitchError::profile_not_found(name, &available));
        }

        let (is_active, running) = self.is_active_and_running(name);

        if is_active && running && !force {
            return Err(SingSwitchError::general(format!(
                "{name} is in use by the running service"
            ))
            .with_hints([
                "stop sing-box first with: singswitch off".to_string(),
                format!("or delete it anyway with: singswitch rm {name} --force"),
            ]));
        }

        if is_active && running {
            self.debug("stopping sing-box before deleting active profile");
            self.manager.stop()?;
        }

        let path = path_for(&self.layout.profiles_dir, name);
        fs::remove_file(&path).map_err(|e| store_error(e, &self.layout.profiles_dir, "delete"))?;
        remove_source(&self.layout.profiles_dir, name)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "delete"))?;

        // Clear the active marker too, or it keeps naming a profile that no
        // longer exists and the UI shows a phantom active profile. The profile
        // itself is already gone, so a failed clear is reported, not swallowed:
        // returning Ok here told the caller the delete fully succeeded while
        // the UI kept pointing at a deleted file.
        if is_active && let Err(err) = self.activator.clear() {
            return Err(SingSwitchError::general(format!(
                "deleted {name} but could not clear its active marker: {err}"
            ))
            .with_hint("point the service elsewhere with: singswitch use <other-profile>"));
        }

        Ok(is_active)
    }

    /// Validate a named profile (or the currently active profile when `name` is `None`).
    pub fn check_profile(&self, name: Option<&str>) -> Result<(String, Vec<String>)> {
        let target_name = match name {
            Some(n) => n.to_string(),
            None => self.activator.active_name()?.ok_or_else(|| {
                SingSwitchError::general("no profile is active, so there is nothing to check")
                    .with_hints([
                        "name one explicitly: singswitch check <name>",
                        "or activate one first: singswitch use <name>",
                    ])
            })?,
        };
        validate_name(&target_name)?;

        let profiles = list_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;
        let available: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        let target = profiles
            .into_iter()
            .find(|p| p.name == target_name)
            .ok_or_else(|| SingSwitchError::profile_not_found(&target_name, &available))?;

        if !target.missing.is_empty() {
            return Err(SingSwitchError::profile_incomplete(
                &target.name,
                &target.missing,
            ));
        }

        self.checker.check(&target.path)?;
        Ok((target.name, Vec::new()))
    }

    /// Validate unsaved JSON editor contents strictly (syntax, placeholders, and `sing-box check`).
    pub fn check_json_content(&self, label: &str, content: &str) -> Result<()> {
        if let Err(err) = serde_json::from_str::<serde_json::Value>(content) {
            return Err(SingSwitchError::validation(format!(
                "invalid JSON syntax: {err}"
            )));
        }
        if let Ok(model) = ProfileFormModel::from_json_str(content) {
            let missing = model.missing();
            if !missing.is_empty() {
                return Err(SingSwitchError::profile_incomplete(label, &missing));
            }
        }
        check_temp_bytes(
            self.checker.as_ref(),
            "singswitch-live-check-",
            content.as_bytes(),
        )
    }

    /// Ensure the configured profiles directory exists.
    pub fn ensure_profiles_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "create"))
    }

    /// Ensure profiles directory exists and attempt to start the Windows helper service.
    pub fn repair_windows_helper(&self) -> Result<()> {
        self.ensure_profiles_dir()?;
        if self.layout.platform != PlatformKind::Windows {
            return Ok(());
        }
        let out = self.runner.run("sc.exe", &["start", HELPER_SERVICE_NAME])?;
        let lower = out.combined.to_ascii_lowercase();
        if out.success || lower.contains("already running") || lower.contains("1056") {
            return Ok(());
        }
        Err(SingSwitchError::service(format!(
            "could not start {HELPER_SERVICE_NAME}: {}",
            crate::error::first_line(out.combined.trim())
        ))
        .with_hint(
            "run powershell -ExecutionPolicy Bypass -File scripts\\install.ps1 as Administrator",
        ))
    }

    /// Execute a narrow Windows Named Pipe request directly inside an elevated `singswitch-agent` host.
    pub fn handle_pipe_request(&self, req: PipeRequest) -> PipeResponse {
        match req {
            PipeRequest::Probe => match self.manager.probe() {
                Ok(h) => PipeResponse::with_health(h),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::Activate { name } => match self.activate(&name) {
                Ok(()) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::Stop => match self.stop() {
                Ok(()) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::CreateProfile { name } => match self.create_from_skeleton(&name) {
                Ok(_) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::PullProfile { name, url } => match self.pull_profile(&name, &url) {
                Ok(_) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::SyncOne { name } => match self.sync_one(&name) {
                Ok(reloaded) => PipeResponse::with_reloaded(reloaded),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::SyncAll => match self.sync_all() {
                Ok(report) => PipeResponse::with_sync_report(report.synced, report.failed),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::SaveProfile {
                name,
                content,
                source_url,
                allow_invalid,
            } => match self.save_profile(&name, &content, allow_invalid) {
                Ok(reloaded) => {
                    if let Some(url) = source_url
                        && let Err(err) = write_source(&self.layout.profiles_dir, &name, &url)
                    {
                        // The profile bytes are already saved. Reporting success here
                        // left the GUI's "source" field claiming a URL nothing stored.
                        return PipeResponse::from_error(
                            &store_error(
                                err,
                                &self.layout.profiles_dir,
                                "record the source URL for",
                            )
                            .with_hint(format!("save {name} again to retry recording its source")),
                        );
                    }
                    PipeResponse::with_reloaded(reloaded)
                }
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::DeleteProfile { name, force } => match self.delete_profile(&name, force) {
                Ok(_) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::EnsureProfilesDir => match self.ensure_profiles_dir() {
                Ok(()) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::RepairWindowsHelper => match self.repair_windows_helper() {
                Ok(()) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
        }
    }

    /// Execute a privileged operation for GUI callers:
    /// - On macOS: runs directly through scoped `sudo -n` inside `Controller`.
    /// - On Windows: if elevated, runs locally; otherwise tries `singswitch-agent` Named Pipe first,
    ///   then falls back to `ShellExecuteExW("runas", SW_HIDE)` invoking `singswitch --elevated-op`.
    pub fn gui_activate(&self, name: &str, on_phase: impl FnMut(ActivationPhase)) -> Result<()> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            let req = PipeRequest::Activate {
                name: name.to_string(),
            };
            return self.dispatch_windows_privileged(&req).map(|_| ());
        }
        self.activate_with_progress(name, on_phase)
    }

    pub fn gui_stop(&self) -> Result<()> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            return self
                .dispatch_windows_privileged(&PipeRequest::Stop)
                .map(|_| ());
        }
        self.stop()
    }

    pub fn gui_create_profile(&self, name: &str) -> Result<PathBuf> {
        self.try_local_or_windows_rpc(
            || self.create_from_skeleton(name),
            || PipeRequest::CreateProfile {
                name: name.to_string(),
            },
            |_| path_for(&self.layout.profiles_dir, name),
        )
    }

    pub fn gui_pull_profile(&self, name: &str, url: &str) -> Result<()> {
        self.try_local_or_windows_rpc(
            || self.pull_profile(name, url).map(|_| ()),
            || PipeRequest::PullProfile {
                name: name.to_string(),
                url: url.to_string(),
            },
            |_| (),
        )
    }

    pub fn gui_sync_one(&self, name: &str) -> Result<bool> {
        self.try_local_or_windows_rpc(
            || self.sync_one(name),
            || PipeRequest::SyncOne {
                name: name.to_string(),
            },
            |resp| resp.reloaded,
        )
    }

    pub fn gui_sync_all(&self) -> Result<SyncReport> {
        self.try_local_or_windows_rpc(
            || self.sync_all(),
            || PipeRequest::SyncAll,
            |resp| SyncReport {
                synced: resp.synced,
                failed: resp.failed,
            },
        )
    }

    pub fn gui_save_profile(&self, name: &str, content: &str, allow_invalid: bool) -> Result<bool> {
        self.try_local_or_windows_rpc(
            || self.save_profile(name, content, allow_invalid),
            || PipeRequest::SaveProfile {
                name: name.to_string(),
                content: content.to_string(),
                source_url: None,
                allow_invalid,
            },
            |resp| resp.reloaded,
        )
    }

    pub fn gui_delete_profile(&self, name: &str, force: bool) -> Result<bool> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            let req = PipeRequest::DeleteProfile {
                name: name.to_string(),
                force,
            };
            self.dispatch_windows_privileged(&req)?;
            return Ok(false);
        }
        self.delete_profile(name, force)
    }

    pub fn gui_ensure_profiles_dir(&self) -> Result<()> {
        self.try_local_or_windows_rpc(
            || self.ensure_profiles_dir(),
            || PipeRequest::EnsureProfilesDir,
            |_| (),
        )
    }

    pub fn gui_repair_windows_helper(&self) -> Result<()> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            self.dispatch_windows_privileged_raw(&PipeRequest::RepairWindowsHelper)?;
            return Ok(());
        }
        self.repair_windows_helper()
    }

    fn try_local_or_windows_rpc<T>(
        &self,
        local: impl FnOnce() -> Result<T>,
        make_req: impl FnOnce() -> PipeRequest,
        map_resp: impl FnOnce(PipeResponse) -> T,
    ) -> Result<T> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            match local() {
                Ok(val) => return Ok(val),
                Err(err) if err.code == ExitCode::Permission => {
                    let resp = self.dispatch_windows_privileged_raw(&make_req())?;
                    return Ok(map_resp(resp));
                }
                Err(err) => return Err(err),
            }
        }
        local()
    }

    fn dispatch_windows_privileged(&self, req: &PipeRequest) -> Result<Option<Health>> {
        let resp = self.dispatch_windows_privileged_raw(req)?;
        Ok(resp.health)
    }

    fn dispatch_windows_privileged_raw(&self, req: &PipeRequest) -> Result<PipeResponse> {
        if let Some(resp) = try_send_pipe_request(&self.layout.pipe_name, req)? {
            let err_or_ok = resp.clone().into_result();
            err_or_ok?;
            return Ok(resp);
        }
        #[cfg(windows)]
        {
            run_windows_hidden_uac_op(req)
        }
        #[cfg(not(windows))]
        {
            Err(SingSwitchError::permission("helper pipe not available"))
        }
    }
}

#[cfg(windows)]
fn run_windows_hidden_uac_op(req: &PipeRequest) -> Result<PipeResponse> {
    let singswitch_cli = find_singswitch_cli_exe().ok_or_else(|| {
        SingSwitchError::permission("singswitch.exe CLI was not found alongside singswitch-app.exe")
            .with_hint("re-run scripts\\install.ps1 as Administrator")
    })?;

    let req_tmp = tempfile::Builder::new()
        .prefix("singswitch-req-")
        .suffix(".json")
        .tempfile()?;
    let resp_tmp = tempfile::Builder::new()
        .prefix("singswitch-resp-")
        .suffix(".json")
        .tempfile()?;
    let (_, req_path) = req_tmp
        .keep()
        .map_err(|e| SingSwitchError::general(e.error.to_string()))?;
    let (_, resp_path) = resp_tmp
        .keep()
        .map_err(|e| SingSwitchError::general(e.error.to_string()))?;

    let payload = serde_json::to_vec(req)
        .map_err(|e| SingSwitchError::general(format!("serialize elevated op: {e}")))?;
    fs::write(&req_path, payload)?;

    let args = vec![
        "--elevated-op".to_string(),
        req_path.to_string_lossy().into_owned(),
        "--elevated-out".to_string(),
        resp_path.to_string_lossy().into_owned(),
    ];

    let exit_res = crate::elevation::run_elevated_executable(&singswitch_cli, &args, true);
    let resp_raw = fs::read_to_string(&resp_path).ok();
    let _ = fs::remove_file(&req_path);
    let _ = fs::remove_file(&resp_path);

    let code = exit_res?;
    if let Some(raw) = resp_raw
        && let Ok(parsed) = serde_json::from_str::<PipeResponse>(&raw)
    {
        let cloned = parsed.clone();
        parsed.into_result()?;
        return Ok(cloned);
    }

    if code == 0 {
        // The child writes its response before exiting, so a clean exit with no
        // readable response means the result was lost. Reporting success here
        // silently dropped what the operation actually returned (a save that
        // reloaded sing-box looked like a plain save).
        Err(SingSwitchError::general(
            "elevated singswitch operation finished without a response",
        ))
    } else {
        Err(SingSwitchError::new(
            ExitCode::from_i32(code),
            "elevated singswitch operation failed",
        ))
    }
}

#[cfg(windows)]
fn find_singswitch_cli_exe() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("singswitch.exe");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    let fallback = crate::layout::lookup_binary(
        "singswitch.exe",
        &[r"C:\Program Files\singswitch\singswitch.exe"],
        "",
    );
    (!fallback.as_os_str().is_empty() && fallback.is_file()).then_some(fallback)
}
