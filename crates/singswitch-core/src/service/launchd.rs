use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::error::{Result, SingSwitchError, first_line};
use crate::runner::Runner;
use crate::service::{Health, Manager, RunState};

// These parse constant literals; failure would be a programming bug caught by
// the first test run, not a runtime condition a user can hit. Synthesizing a
// Result carrier for them adds a branch that can never fire.
fn literal_regex(pattern: &'static str) -> Regex {
    // Panicking here is deliberate: the patterns above are compile-time
    // constants, and a bad one must fail loudly and immediately.
    #[allow(clippy::expect_used, clippy::missing_panics_doc)]
    Regex::new(pattern).expect("constant regex must compile")
}

static STATE_RE: LazyLock<Regex> = LazyLock::new(|| literal_regex(r"(?m)^\s*state\s*=\s*(\S+)"));
static PID_RE: LazyLock<Regex> = LazyLock::new(|| literal_regex(r"(?m)^\s*pid\s*=\s*(\d+)"));
static RUNS_RE: LazyLock<Regex> = LazyLock::new(|| literal_regex(r"(?m)^\s*runs\s*=\s*(\d+)"));
static LAST_EXIT_RE: LazyLock<Regex> =
    LazyLock::new(|| literal_regex(r"(?m)^\s*last exit (?:code|status)\s*=\s*(.+)$"));

const NOT_LOADED_MARKERS: &[&str] = &[
    "could not find service",
    "could not find specified service",
    "no such process",
    "service is not loaded",
    "not find service",
];

fn launchd_not_loaded(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    NOT_LOADED_MARKERS.iter().any(|m| lowered.contains(m))
}

/// Drives a macOS system LaunchDaemon via `launchctl`.
pub struct LaunchdManager {
    pub ctl_bin: PathBuf,
    pub label: String,
    pub plist_path: PathBuf,
    pub runner: Arc<dyn Runner>,
}

impl LaunchdManager {
    pub fn new(
        ctl_bin: PathBuf,
        label: String,
        plist_path: PathBuf,
        runner: Arc<dyn Runner>,
    ) -> Self {
        Self {
            ctl_bin,
            label,
            plist_path,
            runner,
        }
    }

    fn ctl(&self) -> String {
        self.ctl_bin.to_string_lossy().into_owned()
    }

    fn run_ctl(&self, args: &[&str]) -> Result<()> {
        let ctl = self.ctl();
        let out = self.runner.run(&ctl, args)?;
        if out.success {
            return Ok(());
        }
        Err(SingSwitchError::service(format!(
            "launchctl {} failed: {}",
            args.join(" "),
            first_line(out.combined.trim())
        ))
        .with_hint("repair the scoped sudo rules with: sudo bash scripts/install-macos.sh"))
    }

    fn reload(&self) -> Result<()> {
        self.stop()?;
        let plist = self.plist_path.to_string_lossy().into_owned();
        self.run_ctl(&["bootstrap", "system", &plist])
    }
}

impl Manager for LaunchdManager {
    fn probe(&self) -> Result<Health> {
        let ctl = self.ctl();
        let out = self.runner.run(&ctl, &["print", &self.label])?;
        if !out.success {
            if launchd_not_loaded(&out.combined) {
                return Ok(Health::unknown(RunState::Stopped));
            }
            return Err(SingSwitchError::service(format!(
                "could not read the launchd service state: {}",
                first_line(out.combined.trim())
            ))
            .with_hint("repair the scoped sudo rules with: sudo bash scripts/install-macos.sh"));
        }
        Ok(parse_launchd_print(&out.combined))
    }

    fn restart(&self) -> Result<()> {
        // No pre-check for "is loaded": a `kickstart` against an unloaded
        // service fails into the `reload()` fallback below, whose `stop()` leg
        // is a no-op when the service is not loaded — the same outcome as the
        // old explicit `is_loaded` probe, without spawning `sudo launchctl
        // print` on every restart. The bootstrap leg stays: it is the only way
        // to start an unloaded service.
        if let Err(kick_err) = self.run_ctl(&["kickstart", "-k", &self.label])
            && let Err(reload_err) = self.reload()
        {
            return Err(SingSwitchError::service(format!(
                "{kick_err} (reload fallback also failed: {reload_err})"
            )));
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let ctl = self.ctl();
        let out = self.runner.run(&ctl, &["bootout", &self.label])?;
        if out.success || launchd_not_loaded(&out.combined) {
            return Ok(());
        }
        Err(SingSwitchError::service(format!(
            "could not stop the sing-box service: {}",
            first_line(out.combined.trim())
        ))
        .with_hint("repair the scoped sudo rules with: sudo bash scripts/install-macos.sh"))
    }
}

/// Extract a `Health` sample from `launchctl print` output.
pub fn parse_launchd_print(text: &str) -> Health {
    let mut health = Health::unknown(RunState::Stopped);

    if let Some(cap) = STATE_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        let raw = m.as_str();
        // Only `running` is Running. `exited` deliberately stays Stopped: with
        // KeepAlive, launchd respawns through an `exited` window, so mapping it
        // to Error would flash "Service failed" on every normal restart.
        health.state = if raw.eq_ignore_ascii_case("running") {
            RunState::Running
        } else {
            RunState::Stopped
        };
        health.detail = format!("state = {raw}");
    }
    if let Some(cap) = PID_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        health.pid = m.as_str().parse::<u32>().ok();
    }
    if let Some(cap) = RUNS_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        health.restarts = m.as_str().parse::<u64>().ok();
    }
    if let Some(cap) = LAST_EXIT_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        let value = m.as_str().trim();
        if !value.contains("never exited") && value != "0" {
            health.last_exit = Some(value.to_string());
        }
    }
    health
}
