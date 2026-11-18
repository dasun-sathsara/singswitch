use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;

use crate::error::{Result, SbctlError, first_line};
use crate::runner::Runner;
use crate::service::{Health, Manager, RunState};

static SC_STATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"STATE\s*:\s*\d+\s+(\S+)").expect("regex"));
static SC_PID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"PID\s*:\s*(\d+)").expect("regex"));

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
                return Err(SbctlError::service(format!(
                    "could not confirm the sing-box service stopped: {}",
                    first_line(out.combined.trim())
                )));
            }
            let (_, raw) = parse_sc_query(&out.combined);
            if matches!(raw, WinRawState::Stopped | WinRawState::Unknown) {
                return Ok(());
            }
            (self.sleep_fn)(interval);
        }

        Err(SbctlError::service(format!(
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
            return Err(SbctlError::service(format!(
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
            return Err(SbctlError::service(format!(
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
        Err(SbctlError::service(format!(
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
        && let Some(m) = cap.get(1)
    {
        let token = m.as_str().to_ascii_uppercase();
        health.detail = format!("STATE = {}", m.as_str());
        raw = match token.as_str() {
            "RUNNING" => WinRawState::Running,
            "STOPPED" => WinRawState::Stopped,
            "START_PENDING" => WinRawState::StartPending,
            "STOP_PENDING" => WinRawState::StopPending,
            _ => WinRawState::Unknown,
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

    if raw == WinRawState::Running && health.pid == Some(0) {
        health.state = RunState::Stopped;
    }

    (health, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FakeResult, FakeRunner};

    #[test]
    fn parses_sc_queryex_states_including_transient() {
        let running = "SERVICE_NAME: sing-box\n        STATE              : 4  RUNNING\n        PID                : 9184\n";
        let (h, raw) = parse_sc_query(running);
        assert_eq!(raw, WinRawState::Running);
        assert_eq!(h.state, RunState::Running);
        assert_eq!(h.pid, Some(9184));

        let pending =
            "        STATE              : 2  START_PENDING\n        PID                : 0\n";
        let (hp, raw_p) = parse_sc_query(pending);
        assert_eq!(raw_p, WinRawState::StartPending);
        assert_eq!(hp.state, RunState::Running);

        let stopped = "        STATE              : 1  STOPPED\n        PID                : 0\n";
        let (hs, raw_s) = parse_sc_query(stopped);
        assert_eq!(raw_s, WinRawState::Stopped);
        assert_eq!(hs.state, RunState::Stopped);
    }

    #[test]
    fn restart_waits_for_stop_before_start() {
        let fake = Arc::new(FakeRunner::default());
        fake.queue_responses(
            "sc.exe queryex sing-box",
            vec![
                FakeResult::ok("STATE : 3 STOP_PENDING\nPID : 100\n"),
                FakeResult::ok("STATE : 1 STOPPED\nPID : 0\n"),
            ],
        );
        let mut mgr = WinSwManager::new("sing-box", fake.clone());
        mgr.sleep_fn = Arc::new(|_| {});
        mgr.restart().expect("restart succeeds");

        assert_eq!(
            fake.call_lines(),
            vec![
                "sc.exe stop sing-box",
                "sc.exe queryex sing-box",
                "sc.exe queryex sing-box",
                "sc.exe start sing-box",
            ]
        );
    }
}
