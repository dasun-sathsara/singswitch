use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::error::{Result, SbctlError, first_line};
use crate::runner::Runner;
use crate::service::{Health, Manager, RunState};

static STATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*state\s*=\s*(\S+)").expect("regex"));
static PID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*pid\s*=\s*(\d+)").expect("regex"));
static RUNS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*runs\s*=\s*(\d+)").expect("regex"));
static LAST_EXIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*last exit (?:code|status)\s*=\s*(.+)$").expect("regex"));

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

    fn is_loaded(&self) -> Result<bool> {
        let ctl = self.ctl();
        let out = self.runner.run(&ctl, &["print", &self.label])?;
        if out.success {
            return Ok(true);
        }
        if launchd_not_loaded(&out.combined) {
            return Ok(false);
        }
        Err(SbctlError::service(format!(
            "could not read the launchd service state: {}",
            first_line(out.combined.trim())
        )))
    }

    fn run_ctl(&self, args: &[&str]) -> Result<()> {
        let ctl = self.ctl();
        let out = self.runner.run(&ctl, args)?;
        if out.success {
            return Ok(());
        }
        Err(SbctlError::service(format!(
            "launchctl {} failed: {}",
            args.join(" "),
            first_line(out.combined.trim())
        )))
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
            return Err(SbctlError::service(format!(
                "could not read the launchd service state: {}",
                first_line(out.combined.trim())
            )));
        }
        Ok(parse_launchd_print(&out.combined))
    }

    fn restart(&self) -> Result<()> {
        if !self.is_loaded()? {
            let plist = self.plist_path.to_string_lossy().into_owned();
            return self.run_ctl(&["bootstrap", "system", &plist]);
        }
        if let Err(kick_err) = self.run_ctl(&["kickstart", "-k", &self.label])
            && let Err(reload_err) = self.reload()
        {
            return Err(SbctlError::service(format!(
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
        Err(SbctlError::service(format!(
            "could not stop the sing-box service: {}",
            first_line(out.combined.trim())
        )))
    }
}

/// Extract a `Health` sample from `launchctl print` output.
pub fn parse_launchd_print(text: &str) -> Health {
    let mut health = Health::unknown(RunState::Stopped);

    if let Some(cap) = STATE_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        let raw = m.as_str();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FakeResult, FakeRunner, SudoRunner};

    #[test]
    fn parses_real_launchctl_print_running_and_waiting() {
        let running = r"
system/app.lexiflix.singbox = {
    active count = 1
    state = running
    pid = 4711
    runs = 3
    last exit code = (never exited)
}
";
        let h = parse_launchd_print(running);
        assert_eq!(h.state, RunState::Running);
        assert_eq!(h.pid, Some(4711));
        assert_eq!(h.restarts, Some(3));
        assert_eq!(h.last_exit, None);

        let waiting = r"
system/app.lexiflix.singbox = {
    state = waiting
    runs = 14
    last exit code = 1
}
";
        let hw = parse_launchd_print(waiting);
        assert_eq!(hw.state, RunState::Stopped);
        assert_eq!(hw.restarts, Some(14));
        assert_eq!(hw.last_exit.as_deref(), Some("1"));
    }

    #[test]
    fn restart_bootstraps_when_not_loaded() {
        let fake = Arc::new(FakeRunner::default());
        fake.set_response(
            "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
            FakeResult::err(
                113,
                "Could not find service \"app.lexiflix.singbox\" in domain for system",
            ),
        );
        let mgr = LaunchdManager::new(
            PathBuf::from("/bin/launchctl"),
            "system/app.lexiflix.singbox".into(),
            PathBuf::from("/Library/LaunchDaemons/app.lexiflix.singbox.plist"),
            Arc::new(SudoRunner::new(fake.clone())),
        );
        mgr.restart().expect("restart should bootstrap");
        assert_eq!(
            fake.call_lines(),
            vec![
                "sudo -n /bin/launchctl print system/app.lexiflix.singbox",
                "sudo -n /bin/launchctl bootstrap system /Library/LaunchDaemons/app.lexiflix.singbox.plist",
            ]
        );
    }

    #[test]
    fn sudo_refusal_surfaces_permission_error() {
        let fake = Arc::new(FakeRunner::default());
        fake.set_response(
            "sudo -n /bin/launchctl print",
            FakeResult::err(1, "sudo: a password is required"),
        );
        let mgr = LaunchdManager::new(
            PathBuf::from("/bin/launchctl"),
            "system/app.lexiflix.singbox".into(),
            PathBuf::from("/Library/LaunchDaemons/app.lexiflix.singbox.plist"),
            Arc::new(SudoRunner::new(fake)),
        );
        let err = mgr.probe().unwrap_err();
        assert!(err.is_permission());
    }
}
