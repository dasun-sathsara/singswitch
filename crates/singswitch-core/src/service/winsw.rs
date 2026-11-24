use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;

use crate::error::{Result, SingSwitchError, first_line};
use crate::runner::Runner;
use crate::service::{Health, Manager, RunState};

static SC_STATE_RE: LazyLock<Regex> = LazyLock::new(|| literal_regex(r"STATE\s*:\s*(\d+)\s+(\S+)"));
static SC_PID_RE: LazyLock<Regex> = LazyLock::new(|| literal_regex(r"PID\s*:\s*(\d+)"));

// Same policy as launchd.rs: constant patterns; a bad one is a program bug.
fn literal_regex(pattern: &'static str) -> Regex {
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("constant regex must compile")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinRawState {
    Running,
    Stopped,
    StartPending,
    StopPending,
    Unknown,
}

fn sc_missing(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    lowered.contains("does not exist") || lowered.contains("1060")
}

/// Drives a Windows WinSW service using `sc.exe`.
pub struct WinSwManager {
    pub service_name: String,
    pub runner: Arc<dyn Runner>,
    pub stop_timeout: Duration,
    pub sleep_fn: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl WinSwManager {
    pub fn new(service_name: impl Into<String>, runner: Arc<dyn Runner>) -> Self {
        let name = service_name.into();
        Self {
            service_name: if name.is_empty() {
                "sing-box".to_string()
            } else {
                name
            },
            runner,
            stop_timeout: Duration::from_secs(10),
            sleep_fn: Arc::new(std::thread::sleep),
        }
    }

    fn wait_for_stop(&self) -> Result<()> {
        let interval = Duration::from_millis(250);
        let attempts = ((self.stop_timeout.as_millis() / interval.as_millis()) as usize).max(1);

        for _ in 0..attempts {
            let out = self
                .runner
                .run("sc.exe", &["queryex", &self.service_name])?;
            if !out.success {
                if sc_missing(&out.combined) {
                    return Ok(());
                }
                return Err(SingSwitchError::service(format!(
                    "could not confirm the sing-box service stopped: {}",
                    first_line(out.combined.trim())
                )));
            }
            let (_, raw) = parse_sc_query(&out.combined);
            // Only STOPPED ends the wait. STOP_PENDING still holds the process
            // (and its TUN adapter), and Windows refuses to start a service that
            // is still stopping, so returning early made `restart` fail with 1061.
            // Unknown waits out the timeout on purpose: the state code is parsed
            // numerically, so an unreadable `STATE` line is a real "cannot tell"
            // and a timeout reports it honestly instead of guessing.
            if raw == WinRawState::Stopped {
                return Ok(());
            }
            (self.sleep_fn)(interval);
        }

        Err(SingSwitchError::service(format!(
            "the sing-box service did not stop within {}s",
            self.stop_timeout.as_secs()
        )))
    }
}

impl Manager for WinSwManager {
    fn probe(&self) -> Result<Health> {
        let out = self
            .runner
            .run("sc.exe", &["queryex", &self.service_name])?;
        if !out.success {
            if sc_missing(&out.combined) {
                return Ok(Health::unknown(RunState::Stopped));
            }
            return Err(SingSwitchError::service(format!(
                "could not read the Windows service state: {}",
                first_line(out.combined.trim())
            )));
        }
        let (health, _) = parse_sc_query(&out.combined);
        Ok(health)
    }

    fn restart(&self) -> Result<()> {
        self.stop()?;
        self.wait_for_stop()?;
        let out = self.runner.run("sc.exe", &["start", &self.service_name])?;
        if !out.success {
            return Err(SingSwitchError::service(format!(
                "could not start the sing-box service: {}",
                first_line(out.combined.trim())
            )));
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let out = self.runner.run("sc.exe", &["stop", &self.service_name])?;
        if out.success {
            return Ok(());
        }
        let lowered = out.combined.to_ascii_lowercase();
        if sc_missing(&lowered)
            || lowered.contains("has not been started")
            || lowered.contains("1062")
        {
            return Ok(());
        }
        Err(SingSwitchError::service(format!(
            "could not stop the sing-box service: {}",
            first_line(out.combined.trim())
        )))
    }
}

/// Extract a `Health` sample and raw Win32 service state from `sc.exe queryex`.
pub fn parse_sc_query(text: &str) -> (Health, WinRawState) {
    let mut health = Health::unknown(RunState::Stopped);
    let mut raw = WinRawState::Unknown;

    if let Some(cap) = SC_STATE_RE.captures(text)
        && let (Some(code), Some(name)) = (cap.get(1), cap.get(2))
    {
        let token = name.as_str().to_ascii_uppercase();
        health.detail = format!("STATE = {}", name.as_str());
        // Match the numeric code first: `sc.exe` translates the state *name*
        // (localised Windows prints its own word) but never the code, which is
        // the documented SERVICE_STATUS.dwCurrentState value.
        raw = match code.as_str() {
            "1" => WinRawState::Stopped,
            "2" => WinRawState::StartPending,
            "3" => WinRawState::StopPending,
            "4" => WinRawState::Running,
            _ => match token.as_str() {
                "RUNNING" => WinRawState::Running,
                "STOPPED" => WinRawState::Stopped,
                "START_PENDING" => WinRawState::StartPending,
                "STOP_PENDING" => WinRawState::StopPending,
                _ => WinRawState::Unknown,
            },
        };
    }

    health.state = match raw {
        WinRawState::Running | WinRawState::StartPending => RunState::Running,
        WinRawState::Stopped | WinRawState::StopPending => RunState::Stopped,
        WinRawState::Unknown => RunState::Error,
    };

    if let Some(cap) = SC_PID_RE.captures(text)
        && let Some(m) = cap.get(1)
    {
        health.pid = m.as_str().parse::<u32>().ok();
    }

    // No pid means the service is not up. `Some(0)` is the explicit "no
    // process" answer; `None` means the PID line was absent or unparseable, which
    // is just as unusable. START_PENDING counts too: post-restart verification
    // returns on any `Running`, so a stuck start would otherwise pass as healthy.
    if matches!(raw, WinRawState::Running | WinRawState::StartPending)
        && health.pid.unwrap_or(0) == 0
    {
        health.state = RunState::Stopped;
    }

    (health, raw)
}
