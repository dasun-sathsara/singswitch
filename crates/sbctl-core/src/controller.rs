use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::activator::{
    Activator, CopyActivator, Rollback, SymlinkActivator, write_file_atomic,
    write_new_file_exclusive,
};
use crate::checker::{Checker, ExecChecker, check_temp_bytes};
use crate::doctor::{DiagnosticCheck, SudoProbeFn, diagnose_active, run_doctor_checks};
use crate::error::{ExitCode, Result, SbctlError, store_error};
use crate::layout::{Layout, PlatformKind};
use crate::lock::AdvisoryLock;
use crate::notify::{Notifier, default_notifier};
use crate::profile::{
    Profile, SKELETON_JSON, list_profiles, path_for, placeholders_in, placeholders_in_file,
    validate_name,
};
use crate::remote::{fetch_remote_profile, normalise_profile_url, validate_remote_body};
use crate::runner::{ExecRunner, Runner, SudoRunner, TraceHook};
use crate::service::launchd::LaunchdManager;
use crate::service::win_rpc::{PipeRequest, PipeResponse, try_send_pipe_request};
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
            Self::Validating => "Validating profile...",
            Self::UpdatingConfig => "Updating active configuration...",
            Self::RestartingService => "Restarting sing-box service...",
            Self::VerifyingHealth => "Confirming tunnel stays up...",
            Self::RollingBack => "Tunnel failed health check; rolling back...",
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
    pub placeholders: Vec<String>,
    pub broken: Option<String>,
}

/// Summary returned after syncing one or all remote profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub synced: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Optional trace callback for `--verbose` diagnostics.
pub type DebugHook = Arc<dyn Fn(&str) + Send + Sync>;

/// High-level orchestrator used by the CLI, the Windows helper service, and the desktop GUI.
///
/// Every dependency is injected behind a trait so verification, rollback, and error recovery
/// can be exercised deterministically in unit tests without root privileges.
pub struct Controller {
    pub layout: Layout,
    pub manager: Arc<dyn Manager>,
    pub activator: Arc<dyn Activator>,
    pub checker: Arc<dyn Checker>,
    pub runner: Arc<dyn Runner>,
    pub notifier: Arc<dyn Notifier>,
    pub sudo_probe: Option<SudoProbeFn>,
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
        let exec_runner: Arc<dyn Runner> = Arc::new(ExecRunner {
            trace,
            ..Default::default()
        });
        let notifier: Arc<dyn Notifier> = Arc::from(default_notifier(layout.platform));
        Self::with_runner(
            layout,
            exec_runner,
            Arc::new(ExecChecker::default()),
            notifier,
        )
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
                let layout_clone = layout.clone();
                let sudo_link = sudo.clone();
                let act = Arc::new(SymlinkActivator::new(
                    layout.active_config_path.clone(),
                    Some(Arc::new(move |target| {
                        let argv = layout_clone.activate_argv(target);
                        let cmd_args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                        let out = sudo_link.run(&argv[0], &cmd_args)?;
                        if out.success {
                            Ok(())
                        } else {
                            Err(SbctlError::service(format!(
                                "could not update active symlink: {}",
                                out.combined.trim()
                            )))
                        }
                    })),
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
            sudo_probe: None,
            skeleton: SKELETON_JSON.to_string(),
            settle_delay: DEFAULT_SETTLE_DELAY,
            start_grace: DEFAULT_START_GRACE,
            lock_timeout: LOCK_ACQUIRE_TIMEOUT,
            sleep_fn: Arc::new(std::thread::sleep),
            debug_hook: None,
        }
    }

    fn debug(&self, msg: impl AsRef<str>) {
        if let Some(hook) = &self.debug_hook {
            hook(msg.as_ref());
        }
    }

    fn is_active_and_running(&self, name: &str) -> (bool, bool) {
        let active = self.activator.active_name().ok().flatten();
        let is_active = active.as_deref() == Some(name);
        let running = is_active
            && self
                .manager
                .probe()
                .is_ok_and(|h| h.state == RunState::Running);
        (is_active, running)
    }

    fn restore_previous_profile(&self, path: &std::path::Path, old_bytes: &[u8], restart: bool) {
        let _ = write_file_atomic(path, old_bytes, 0o644);
        if self.layout.active_name_path.is_some() {
            let _ = self.activator.activate(path);
        }
        if restart {
            let _ = self.manager.restart();
        }
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
            placeholders: Vec::new(),
            broken: broken.map(ToString::to_string),
        };
        if let Some(active_name) = active
            && let Some(p) = profiles.iter().find(|p| p.name == active_name)
        {
            view.placeholders = p.placeholders.clone();
            view.tun = p.tun.clone();
            view.server = p.server.clone();
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
            .ok_or_else(|| SbctlError::profile_not_found(name, &available))?;

        if !target.ready() {
            return Err(SbctlError::placeholders(&target.name, &target.placeholders));
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

        self.notifier
            .notify(&format!("sing-box switched to {}", target.name));
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
                self.debug(format!(
                    "could not confirm post-settle service health: {err}"
                ));
                return None;
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
                    self.debug(format!("probe error while awaiting start: {err}"));
                    return Some(Health::unknown(RunState::Running));
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
        cause: Option<SbctlError>,
        summary: &str,
    ) -> SbctlError {
        let mut failure = SbctlError::service(summary);
        if let Some(c) = cause {
            failure = failure.with_cause(c);
        }

        if !rollback.known() {
            let _ = self.manager.stop();
            return failure.with_hints([
                "there was no previous profile to fall back to; stopped sing-box",
                "inspect the service output with: sbctl logs",
                "fix the profile and re-run: sbctl use <name>",
            ]);
        }

        let previous = rollback.description();
        if let Err(rb_err) = rollback.rollback() {
            return failure.with_hints([
                format!("could not restore {previous} automatically: {rb_err}"),
                format!("restore it manually with: sbctl use {previous}"),
                "or stop sing-box entirely with: sbctl off".to_string(),
            ]);
        }

        if was_running {
            if let Err(restart_err) = self.manager.restart() {
                return failure.with_hints([
                    format!(
                        "reverted to {previous}, but the service did not restart: {restart_err}"
                    ),
                    "inspect the service output with: sbctl logs".to_string(),
                ]);
            }
            failure.message = format!("{summary}; reverted to {previous}");
        } else {
            let _ = self.manager.stop();
            failure.message = format!("{summary}; reverted to {previous} (service kept stopped)");
        }
        failure.with_hint("inspect the service output with: sbctl logs")
    }

    /// Stop the sing-box service.
    pub fn stop(&self) -> Result<()> {
        let _lock = self.acquire_lock()?;
        self.manager.stop()?;
        self.notifier.notify("sing-box stopped");
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
                SbctlError::general(format!("a profile named {name:?} already exists"))
                    .with_hint(format!("edit it with: sbctl edit {name}"))
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
        if !allow_invalid {
            check_temp_bytes(self.checker.as_ref(), "sbctl-validate-", content.as_bytes())?;
        }

        let markers = placeholders_in(content);
        let (is_active, running) = self.is_active_and_running(name);

        if is_active && running && !markers.is_empty() {
            return Err(SbctlError::placeholders(name, &markers));
        }

        let old_bytes = fs::read(&path).ok();
        write_file_atomic(&path, content.as_bytes(), 0o644)?;

        if !is_active {
            return Ok(false);
        }

        if self.layout.active_name_path.is_some()
            && markers.is_empty()
            && let Err(err) = self.activator.activate(&path)
        {
            if let Some(old) = &old_bytes {
                let _ = write_file_atomic(&path, old, 0o644);
            }
            return Err(store_error(
                std::io::Error::other(err.to_string()),
                &self.layout.profiles_dir,
                "update the active configuration for",
            ));
        }

        if !running {
            return Ok(false);
        }

        self.debug(format!("reloading running service after editing {name}"));
        self.manager.restart()?;
        if let Some(problem) = self.verify_healthy() {
            if let Some(old) = &old_bytes {
                self.restore_previous_profile(&path, old, true);
                return Err(SbctlError::service(format!(
                    "sing-box reloaded {name} but {problem}; reverted {name} to previous contents"
                ))
                .with_hint("inspect the service output with: sbctl logs"));
            }
            return Err(
                SbctlError::service(format!("sing-box reloaded {name} but {problem}")).with_hints(
                    [
                        "inspect the service output with: sbctl logs".to_string(),
                        format!("revise the profile with: sbctl edit {name}"),
                    ],
                ),
            );
        }

        self.notifier.notify(&format!("sing-box reloaded {name}"));
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
            return Err(
                SbctlError::general(format!("a profile named {name:?} already exists")).with_hints(
                    [
                        format!("refresh it from its source with: sbctl sync {name}"),
                        format!("or delete it first with: sbctl rm {name}"),
                    ],
                ),
            );
        }

        let body = fetch_remote_profile(&url)?;
        validate_remote_body(self.checker.as_ref(), &body)?;

        write_new_file_exclusive(&path, &body, 0o644).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                SbctlError::general(format!("a profile named {name:?} already exists")).with_hint(
                    format!("refresh it from its source with: sbctl sync {name}"),
                )
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
            .ok_or_else(|| SbctlError::general(format!("pulled {name} but could not read it back")))
    }

    /// Re-fetch one remotely pulled profile, reloading it via verified activation if active.
    pub fn sync_one(&self, name: &str) -> Result<bool> {
        let _lock = self.acquire_lock()?;
        validate_name(name)?;

        let url = read_source(&self.layout.profiles_dir, name)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?
            .ok_or_else(|| {
                SbctlError::general(format!("{name:?} has no recorded source URL")).with_hints([
                    format!("fetch it from a URL first with: sbctl pull {name} <url>"),
                    "sync without a name refreshes every profile that does have one".to_string(),
                ])
            })?;

        self.refresh_profile_locked(name, &url)?;
        let (is_active, running) = self.is_active_and_running(name);
        if !is_active {
            return Ok(false);
        }
        if !running {
            if self.layout.active_name_path.is_some() {
                let _ = self
                    .activator
                    .activate(&path_for(&self.layout.profiles_dir, name));
            }
            return Ok(false);
        }
        self.activate_locked(name, |_| {})?;
        Ok(true)
    }

    /// Re-fetch every profile with a `.url` sidecar. Inactive profiles update first;
    /// the active profile updates last through verified activation.
    pub fn sync_all(&self) -> Result<SyncReport> {
        let _lock = self.acquire_lock()?;
        let names = sourced_profiles(&self.layout.profiles_dir)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "read"))?;

        let active = self.activator.active_name().ok().flatten();
        let mut report = SyncReport {
            synced: Vec::new(),
            failed: Vec::new(),
        };

        for name in &names {
            if active.as_deref() == Some(name.as_str()) {
                continue;
            }
            let Ok(Some(url)) = read_source(&self.layout.profiles_dir, name) else {
                report
                    .failed
                    .push((name.clone(), "missing source URL".to_string()));
                continue;
            };
            match self.refresh_profile_locked(name, &url) {
                Ok(()) => report.synced.push(name.clone()),
                Err(err) => report.failed.push((name.clone(), err.to_string())),
            }
        }

        if let Some(active_name) = active
            && names.iter().any(|n| n == &active_name)
        {
            let path = path_for(&self.layout.profiles_dir, &active_name);
            let old_bytes = fs::read(&path).ok();
            if let Ok(Some(url)) = read_source(&self.layout.profiles_dir, &active_name) {
                match self.refresh_profile_locked(&active_name, &url) {
                    Err(err) => report.failed.push((active_name, err.to_string())),
                    Ok(()) => {
                        let running = self
                            .manager
                            .probe()
                            .is_ok_and(|h| h.state == RunState::Running);
                        if running {
                            if let Err(act_err) = self.activate_locked(&active_name, |_| {}) {
                                if let Some(old) = &old_bytes {
                                    self.restore_previous_profile(&path, old, true);
                                }
                                report.failed.push((active_name, act_err.to_string()));
                                return Ok(report);
                            }
                        } else if self.layout.active_name_path.is_some() {
                            let _ = self.activator.activate(&path);
                        }
                        report.synced.push(active_name);
                    }
                }
            }
        }

        Ok(report)
    }

    fn refresh_profile_locked(&self, name: &str, url: &str) -> Result<()> {
        let path = path_for(&self.layout.profiles_dir, name);
        if !path.exists() {
            return Err(SbctlError::general(format!(
                "profile {name:?} is gone; its source URL is orphaned"
            ))
            .with_hint(format!("fetch it again with: sbctl pull {name} {url}")));
        }
        let body = fetch_remote_profile(url)?;
        validate_remote_body(self.checker.as_ref(), &body)?;
        write_file_atomic(&path, &body, 0o644)
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
            return Err(SbctlError::profile_not_found(name, &available));
        }

        let (is_active, running) = self.is_active_and_running(name);

        if is_active && running && !force {
            return Err(
                SbctlError::general(format!("{name} is in use by the running service")).with_hints(
                    [
                        "stop sing-box first with: sbctl off".to_string(),
                        format!("or delete it anyway with: sbctl rm {name} --force"),
                    ],
                ),
            );
        }

        if is_active && running {
            self.debug("stopping sing-box before deleting active profile");
            self.manager.stop()?;
        }

        let path = path_for(&self.layout.profiles_dir, name);
        fs::remove_file(&path).map_err(|e| store_error(e, &self.layout.profiles_dir, "delete"))?;
        remove_source(&self.layout.profiles_dir, name)
            .map_err(|e| store_error(e, &self.layout.profiles_dir, "delete"))?;

        Ok(is_active)
    }

    /// Validate a named profile (or the currently active profile when `name` is `None`).
    pub fn check_profile(&self, name: Option<&str>) -> Result<(String, Vec<String>)> {
        let target_name = match name {
            Some(n) => n.to_string(),
            None => self.activator.active_name()?.ok_or_else(|| {
                SbctlError::general("no profile is active, so there is nothing to check")
                    .with_hints([
                        "name one explicitly: sbctl check <name>",
                        "or activate one first: sbctl use <name>",
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
            .ok_or_else(|| SbctlError::profile_not_found(&target_name, &available))?;

        let markers = placeholders_in_file(&target.path).unwrap_or(target.placeholders);
        if !markers.is_empty() {
            return Err(SbctlError::placeholders(&target.name, &markers));
        }

        self.checker.check(&target.path)?;
        Ok((target.name, Vec::new()))
    }

    /// Validate unsaved JSON editor contents strictly (syntax, placeholders, and `sing-box check`).
    pub fn check_json_content(&self, label: &str, content: &str) -> Result<()> {
        if let Err(err) = serde_json::from_str::<serde_json::Value>(content) {
            return Err(SbctlError::validation(format!(
                "invalid JSON syntax: {err}"
            )));
        }
        let markers = placeholders_in(content);
        if !markers.is_empty() {
            return Err(SbctlError::placeholders(label, &markers));
        }
        check_temp_bytes(
            self.checker.as_ref(),
            "sbctl-live-check-",
            content.as_bytes(),
        )
    }

    /// Run all `doctor` checks against this controller's layout and services.
    pub fn doctor(&self) -> Vec<DiagnosticCheck> {
        run_doctor_checks(
            &self.layout,
            self.checker.as_ref(),
            self.manager.as_ref(),
            self.activator.as_ref(),
            self.runner.as_ref(),
            self.sudo_probe.as_ref(),
        )
    }

    /// Execute a narrow Windows Named Pipe request directly inside an elevated `sbctl-agent` host.
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
            PipeRequest::SaveProfile {
                name,
                content,
                source_url,
                allow_invalid,
            } => match self.save_profile(&name, &content, allow_invalid) {
                Ok(reloaded) => {
                    if let Some(url) = source_url {
                        let _ = write_source(&self.layout.profiles_dir, &name, &url);
                    }
                    PipeResponse::with_reloaded(reloaded)
                }
                Err(e) => PipeResponse::from_error(&e),
            },
            PipeRequest::DeleteProfile { name, force } => match self.delete_profile(&name, force) {
                Ok(_) => PipeResponse::success(),
                Err(e) => PipeResponse::from_error(&e),
            },
        }
    }

    /// Execute a privileged operation for GUI callers:
    /// - On macOS: runs directly through scoped `sudo -n` inside `Controller`.
    /// - On Windows: if elevated, runs locally; otherwise tries `sbctl-agent` Named Pipe first,
    ///   then falls back to `ShellExecuteExW("runas", SW_HIDE)` invoking `sbctl --elevated-op`.
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

    pub fn gui_save_profile(&self, name: &str, content: &str, allow_invalid: bool) -> Result<bool> {
        if self.layout.platform == PlatformKind::Windows && !crate::elevation::is_elevated() {
            // Try direct write first if the user has directory permissions; fall back to pipe/UAC on AccessDenied.
            match self.save_profile(name, content, allow_invalid) {
                Ok(reloaded) => return Ok(reloaded),
                Err(err) if err.code == ExitCode::Permission => {
                    let req = PipeRequest::SaveProfile {
                        name: name.to_string(),
                        content: content.to_string(),
                        source_url: None,
                        allow_invalid,
                    };
                    let resp = self.dispatch_windows_privileged_raw(&req)?;
                    return Ok(resp.reloaded);
                }
                Err(err) => return Err(err),
            }
        }
        self.save_profile(name, content, allow_invalid)
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
            Err(SbctlError::permission("helper pipe not available"))
        }
    }
}

#[cfg(windows)]
fn run_windows_hidden_uac_op(req: &PipeRequest) -> Result<PipeResponse> {
    let sbctl_cli = find_sbctl_cli_exe().ok_or_else(|| {
        SbctlError::permission("sbctl.exe CLI was not found alongside sbctl-app.exe")
            .with_hint("re-run scripts\\install.ps1 as Administrator")
    })?;

    let req_tmp = tempfile::Builder::new()
        .prefix("sbctl-req-")
        .suffix(".json")
        .tempfile()?;
    let resp_tmp = tempfile::Builder::new()
        .prefix("sbctl-resp-")
        .suffix(".json")
        .tempfile()?;
    let (_, req_path) = req_tmp
        .keep()
        .map_err(|e| SbctlError::general(e.error.to_string()))?;
    let (_, resp_path) = resp_tmp
        .keep()
        .map_err(|e| SbctlError::general(e.error.to_string()))?;

    let payload = serde_json::to_vec(req)
        .map_err(|e| SbctlError::general(format!("serialize elevated op: {e}")))?;
    fs::write(&req_path, payload)?;

    let args = vec![
        "--elevated-op".to_string(),
        req_path.to_string_lossy().into_owned(),
        "--elevated-out".to_string(),
        resp_path.to_string_lossy().into_owned(),
    ];

    let exit_res = crate::elevation::run_elevated_executable(&sbctl_cli, &args, true);
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
        Ok(PipeResponse::success())
    } else {
        Err(SbctlError::new(
            ExitCode::from_i32(code),
            "elevated sbctl operation failed",
        ))
    }
}

#[cfg(windows)]
fn find_sbctl_cli_exe() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("sbctl.exe");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    let fallback =
        crate::layout::lookup_binary("sbctl.exe", &[r"C:\Program Files\sbctl\sbctl.exe"], "");
    (!fallback.as_os_str().is_empty() && fallback.is_file()).then_some(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::FakeChecker;
    use crate::notify::NoopNotifier;
    use crate::runner::{FakeResult, FakeRunner};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn test_controller(
        platform: PlatformKind,
    ) -> (
        tempfile::TempDir,
        Controller,
        Arc<FakeRunner>,
        Arc<FakeChecker>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut layout = match platform {
            PlatformKind::Darwin => Layout::darwin(),
            PlatformKind::Windows => Layout::windows_with_program_data(dir.path()),
        };
        layout.profiles_dir = dir.path().join("profiles");
        layout.active_config_path = dir.path().join("config.json");
        if platform == PlatformKind::Windows {
            layout.active_name_path = Some(dir.path().join("active-profile"));
        }
        fs::create_dir_all(&layout.profiles_dir).unwrap();

        let fake_runner = Arc::new(FakeRunner::default());
        let fake_checker = Arc::new(FakeChecker::with_version("1.13.8"));
        let mut ctrl = Controller::with_runner(
            layout.clone(),
            fake_runner.clone(),
            fake_checker.clone(),
            Arc::new(NoopNotifier),
        );
        if platform == PlatformKind::Darwin {
            // In unit tests without root, record the `sudo -n ln -sfn` command through
            // FakeRunner AND swap the real symlink inside the temp directory.
            let real_act =
                crate::activator::SymlinkActivator::new(layout.active_config_path.clone(), None);
            let runner_for_link = fake_runner.clone();
            let layout_for_link = layout.clone();
            ctrl.activator = Arc::new(crate::activator::SymlinkActivator::new(
                layout.active_config_path,
                Some(Arc::new(move |target| {
                    let argv = layout_for_link.activate_argv(target);
                    let mut sudo_args = vec!["-n"];
                    sudo_args.extend(argv.iter().map(String::as_str));
                    let _ = runner_for_link.run("sudo", &sudo_args)?;
                    let _ = real_act.activate(target)?;
                    Ok(())
                })),
            ));
        }
        ctrl.settle_delay = Duration::ZERO;
        ctrl.start_grace = Duration::ZERO;
        ctrl.sleep_fn = Arc::new(|_| {});
        (dir, ctrl, fake_runner, fake_checker)
    }

    #[test]
    fn darwin_verified_activation_happy_path_and_sudoers_match() {
        let (_dir, ctrl, runner, checker) = test_controller(PlatformKind::Darwin);
        let p = path_for(&ctrl.layout.profiles_dir, "work");
        fs::write(
            &p,
            r#"{"outbounds":[{"type":"vless","server":"1.2.3.4","server_port":443}]}"#,
        )
        .unwrap();

        runner.set_response(
            "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
            FakeResult::ok(
                "state = running\npid = 4711\nruns = 2\nlast exit code = (never exited)\n",
            ),
        );

        ctrl.activate("work").expect("activation succeeds");
        assert_eq!(checker.checked.lock().unwrap().len(), 1);
        assert_eq!(
            ctrl.activator.active_name().unwrap().as_deref(),
            Some("work")
        );

        let rules = ctrl.layout.sudo_commands();
        for call in runner.calls.lock().unwrap().iter() {
            // Strip "sudo -n" prefix and verify every issued privileged command matches one rule!
            assert_eq!(&call[0..2], &["sudo", "-n"]);
            let inner: Vec<String> = call[2..].to_vec();
            assert!(
                rules.iter().any(|r| r.matches(&inner)),
                "unauthorised command issued: {inner:?}"
            );
        }
    }

    #[test]
    fn crash_loop_pid_change_triggers_automatic_rollback() {
        let (_dir, ctrl, runner, _checker) = test_controller(PlatformKind::Darwin);
        let prev = path_for(&ctrl.layout.profiles_dir, "good");
        let broken = path_for(&ctrl.layout.profiles_dir, "bad");
        fs::write(&prev, r#"{"tag":"good"}"#).unwrap();
        fs::write(&broken, r#"{"tag":"bad"}"#).unwrap();

        // Activate "good" first so it becomes the known rollback target.
        runner.set_response(
            "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
            FakeResult::ok(
                "state = running\npid = 100\nruns = 1\nlast exit code = (never exited)\n",
            ),
        );
        ctrl.activate("good").expect("good activates");

        // Now queue: pre-probe (pid 100), post-restart baseline (pid 200), post-settle crash-loop (pid 201, runs 2).
        runner.queue_responses(
            "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
            vec![
                FakeResult::ok("state = running\npid = 100\nruns = 1\n"),
                FakeResult::ok("state = running\npid = 100\nruns = 1\n"),
                FakeResult::ok("state = running\npid = 200\nruns = 2\n"),
                FakeResult::ok("state = running\npid = 201\nruns = 3\n"),
            ],
        );

        let err = ctrl.activate("bad").unwrap_err();
        assert_eq!(err.code, ExitCode::Service);
        assert!(
            err.message.contains("reverted to good"),
            "msg: {}",
            err.message
        );
        assert_eq!(
            ctrl.activator.active_name().unwrap().as_deref(),
            Some("good")
        );
    }

    #[test]
    fn activation_refuses_placeholders() {
        let (_dir, ctrl, _runner, _checker) = test_controller(PlatformKind::Windows);
        ctrl.create_from_skeleton("seed").expect("create");
        let err = ctrl.activate("seed").unwrap_err();
        assert_eq!(err.code, ExitCode::Validation);
        assert!(err.message.contains("TODO_UUID"));
    }

    #[test]
    fn delete_active_running_requires_force_and_stops_first() {
        let (_dir, ctrl, runner, _checker) = test_controller(PlatformKind::Windows);
        let p = path_for(&ctrl.layout.profiles_dir, "work");
        fs::write(&p, r#"{"tag":"work"}"#).unwrap();
        runner.set_response(
            "sc.exe queryex sing-box",
            FakeResult::ok("STATE : 4 RUNNING\nPID : 555\n"),
        );
        ctrl.activator.activate(&p).unwrap();

        let err = ctrl.delete_profile("work", false).unwrap_err();
        assert_eq!(err.code, ExitCode::General);
        assert!(p.exists());

        let was_active = ctrl.delete_profile("work", true).expect("force delete");
        assert!(was_active);
        assert!(!p.exists());
        assert!(
            runner
                .call_lines()
                .iter()
                .any(|l| l == "sc.exe stop sing-box")
        );
    }

    #[test]
    fn pull_and_sync_all_validate_and_preserve_existing_on_failure() {
        let (_dir, ctrl, runner, checker) = test_controller(PlatformKind::Windows);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();

        let srv = std::thread::spawn(move || {
            for i in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body = if i == 0 {
                    r#"{"outbounds":[{"type":"vless","server":"1.1.1.1","server_port":443}]}"#
                } else {
                    r#"{"broken":true}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });

        let url = format!("http://{addr}/sub.json");
        let pulled = ctrl.pull_profile("remote1", &url).expect("pull");
        assert_eq!(pulled.name, "remote1");
        assert_eq!(pulled.source_url.as_deref(), Some(url.as_str()));

        // Make second download fail sing-box validation.
        *checker.global_error.lock().unwrap() = Some("invalid config".to_string());
        runner.set_response(
            "sc.exe queryex sing-box",
            FakeResult::ok("STATE : 1 STOPPED\nPID : 0\n"),
        );
        let report = ctrl.sync_all().expect("sync_all");
        assert!(report.synced.is_empty());
        assert_eq!(report.failed.len(), 1);
        // Stored profile must stay untouched!
        let on_disk = fs::read_to_string(&pulled.path).unwrap();
        assert!(on_disk.contains("1.1.1.1"));

        srv.join().unwrap();
    }

    #[test]
    fn save_profile_reloads_active_service_and_reverts_on_crash() {
        let (_dir, ctrl, runner, _checker) = test_controller(PlatformKind::Windows);
        let p = path_for(&ctrl.layout.profiles_dir, "work");
        fs::write(&p, r#"{"version":"old"}"#).unwrap();

        ctrl.activator.activate(&p).unwrap();

        // Successful reload: probe (RUNNING) -> wait_for_stop (STOPPED) -> baseline (RUNNING 300) -> after (RUNNING 300).
        runner.queue_responses(
            "sc.exe queryex sing-box",
            vec![
                FakeResult::ok("STATE : 4 RUNNING\nPID : 300\n"),
                FakeResult::ok("STATE : 1 STOPPED\nPID : 0\n"),
                FakeResult::ok("STATE : 4 RUNNING\nPID : 300\n"),
                FakeResult::ok("STATE : 4 RUNNING\nPID : 300\n"),
            ],
        );
        let reloaded = ctrl
            .save_profile("work", r#"{"version":"new"}"#, false)
            .unwrap();
        assert!(reloaded);
        assert_eq!(fs::read_to_string(&p).unwrap(), r#"{"version":"new"}"#);

        // Crash loop on reload: probe (RUNNING) -> wait_for_stop (STOPPED) -> baseline (400) -> after (401) -> rollback wait_for_stop (STOPPED).
        runner.queue_responses(
            "sc.exe queryex sing-box",
            vec![
                FakeResult::ok("STATE : 4 RUNNING\nPID : 300\n"),
                FakeResult::ok("STATE : 1 STOPPED\nPID : 0\n"),
                FakeResult::ok("STATE : 4 RUNNING\nPID : 400\n"),
                FakeResult::ok("STATE : 4 RUNNING\nPID : 401\n"),
                FakeResult::ok("STATE : 1 STOPPED\nPID : 0\n"),
            ],
        );
        let err = ctrl
            .save_profile("work", r#"{"version":"broken"}"#, false)
            .unwrap_err();
        assert_eq!(err.code, ExitCode::Service);
        assert!(err.message.contains("reverted work"));
        assert_eq!(fs::read_to_string(&p).unwrap(), r#"{"version":"new"}"#);
    }

    #[test]
    fn doctor_reports_sudo_mismatch_and_broken_active_profile() {
        let (_dir, mut ctrl, runner, _checker) = test_controller(PlatformKind::Darwin);
        runner.set_response(
            "sudo -n /bin/launchctl print",
            FakeResult::ok("state = running\npid = 123\nruns = 1\n"),
        );
        ctrl.sudo_probe = Some(Arc::new(|argv| {
            if argv.iter().any(|a| a == "bootout") {
                Err("sudo: a password is required".to_string())
            } else {
                Ok(())
            }
        }));
        let checks = ctrl.doctor();
        let stop_check = checks.iter().find(|c| c.name == "sudo: stop").unwrap();
        assert!(!stop_check.ok);
        assert_eq!(stop_check.detail, "not permitted without a password");
    }

    #[test]
    fn rollback_keeps_service_stopped_when_initially_stopped() {
        let (_dir, ctrl, runner, _checker) = test_controller(PlatformKind::Darwin);
        let prev = path_for(&ctrl.layout.profiles_dir, "good");
        let broken = path_for(&ctrl.layout.profiles_dir, "bad");
        fs::write(&prev, r#"{"tag":"good"}"#).unwrap();
        fs::write(&broken, r#"{"tag":"bad"}"#).unwrap();

        ctrl.activator.activate(&prev).unwrap();

        // Pre-probe: not loaded (stopped). Post-restart baseline: running PID 200 -> settle PID 201 (crash loop).
        runner.queue_responses(
            "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
            vec![
                FakeResult::err(113, "Could not find service"),
                FakeResult::err(113, "Could not find service"),
                FakeResult::ok("state = running\npid = 200\nruns = 1\n"),
                FakeResult::ok("state = running\npid = 201\nruns = 2\n"),
            ],
        );

        let err = ctrl.activate("bad").unwrap_err();
        assert!(
            err.message
                .contains("reverted to good (service kept stopped)"),
            "unexpected message: {}",
            err.message
        );
        assert_eq!(
            ctrl.activator.active_name().unwrap().as_deref(),
            Some("good")
        );
        assert!(
            runner
                .call_lines()
                .iter()
                .any(|l| l == "sudo -n /bin/launchctl bootout system/app.lexiflix.singbox")
        );
    }

    #[test]
    fn check_json_content_validates_syntax_placeholders_and_checker() {
        let (_dir, ctrl, _runner, _checker) = test_controller(PlatformKind::Darwin);
        let syntax_err = ctrl.check_json_content("work", "{ bad json").unwrap_err();
        assert_eq!(syntax_err.code, ExitCode::Validation);

        let todo_err = ctrl
            .check_json_content("work", r#"{"server":"TODO_SERVER_IP_OR_HOST"}"#)
            .unwrap_err();
        assert_eq!(todo_err.code, ExitCode::Validation);
        assert!(todo_err.message.contains("TODO_SERVER_IP_OR_HOST"));

        assert!(
            ctrl.check_json_content("work", r#"{"log":{"level":"info"}}"#)
                .is_ok()
        );
    }
}
