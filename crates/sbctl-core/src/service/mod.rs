pub mod launchd;

pub mod win_rpc;
pub mod winsw;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Coarse operational state of the sing-box service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Running,
    Stopped,
    Error,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for RunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single liveness sample of the sing-box daemon/service.
///
/// Comparing two post-restart samples separated by a settle delay distinguishes a
/// healthy start from a KeepAlive crash loop (where `launchctl print` still reports
/// `running` while the process cycles underneath).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub state: RunState,
    pub pid: Option<u32>,
    pub restarts: Option<u64>,
    pub last_exit: Option<String>,
    pub detail: String,
}

impl Health {
    pub fn unknown(state: RunState) -> Self {
        Self {
            state,
            pid: None,
            restarts: None,
            last_exit: None,
            detail: String::new(),
        }
    }

    /// Reports whether `self` shows the service died or restarted since `baseline`.
    ///
    /// Both samples must be captured after the intentional restart request so the
    /// restart itself is excluded from platform run/restart counters.
    pub fn crashed_since(&self, baseline: &Health) -> Option<String> {
        if self.state != RunState::Running {
            return Some(format!("service is {}", self.state));
        }
        if let (Some(base_r), Some(curr_r)) = (baseline.restarts, self.restarts)
            && curr_r > base_r
        {
            let count = curr_r - base_r;
            let noun = if count == 1 { "time" } else { "times" };
            return Some(format!(
                "service restarted {count} {noun} while starting up"
            ));
        }
        if let (Some(base_pid), Some(curr_pid)) = (baseline.pid, self.pid)
            && base_pid > 0
            && curr_pid > 0
            && curr_pid != base_pid
        {
            return Some(format!(
                "service was replaced by a new process (pid {base_pid} became {curr_pid})"
            ));
        }
        if let Some(curr_exit) = &self.last_exit
            && !curr_exit.is_empty()
            && baseline.last_exit.as_deref() != Some(curr_exit.as_str())
        {
            return Some(format!("service exited with {curr_exit}"));
        }
        None
    }
}

/// Lifecycle control over the platform's sing-box service.
pub trait Manager: Send + Sync {
    fn restart(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn probe(&self) -> Result<Health>;
}

/// Reports whether command output indicates `sudo -n` declined for lack of a NOPASSWD rule.
pub fn is_sudo_refusal(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    [
        "a password is required",
        "a terminal is required",
        "no tty present",
        "sudo: a password",
        "is not allowed to execute",
        "may not run",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}
