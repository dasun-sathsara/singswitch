use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{Result, SingSwitchError};
use crate::service::is_sudo_refusal;

/// Default subprocess timeout preventing a hung `launchctl` or `sc.exe` call.
pub const DEFAULT_RUNNER_TIMEOUT: Duration = Duration::from_secs(30);

/// Captured output and exit status of a subprocess execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub combined: String,
}

/// Executes external commands so service managers are testable without a privileged host.
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput>;
}

pub type TraceHook = Arc<dyn Fn(&str, &[&str]) + Send + Sync>;

/// Production `Runner` invoking `std::process::Command` with a hard timeout and optional trace.
#[derive(Clone)]
pub struct ExecRunner {
    pub timeout: Duration,
    pub trace: Option<TraceHook>,
}

impl Default for ExecRunner {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_RUNNER_TIMEOUT,
            trace: None,
        }
    }
}

impl Runner for ExecRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        if let Some(trace) = &self.trace {
            trace(program, args);
        }

        let mut cmd = Command::new(program);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|err| {
            SingSwitchError::general(format!("could not run {program}: {err}")).with_cause(err)
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_handle = std::thread::spawn(move || read_pipe(stdout));
        let err_handle = std::thread::spawn(move || read_pipe(stderr));

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SingSwitchError::service(format!(
                    "{program} timed out after {}s",
                    self.timeout.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let out_str = out_handle.join().unwrap_or_default();
        let err_str = err_handle.join().unwrap_or_default();
        let combined = match (out_str.trim().is_empty(), err_str.trim().is_empty()) {
            (false, false) => format!("{}\n{}", out_str.trim(), err_str.trim()),
            (false, true) => out_str,
            (true, false) => err_str,
            (true, true) => String::new(),
        };

        Ok(CommandOutput {
            success: status.success(),
            exit_code: status.code(),
            combined,
        })
    }
}

fn read_pipe(pipe: Option<impl std::io::Read>) -> String {
    let Some(mut reader) = pipe else {
        return String::new();
    };
    let mut buf = String::new();
    let _ = reader.read_to_string(&mut buf);
    buf
}

/// Wraps a `Runner` to execute commands via `sudo -n`, translating refusals into
/// `SingSwitchError::sudo_not_configured`.
///
/// Using `-n` guarantees singswitch never hangs waiting for a terminal password inside a GUI
/// or background tray process.
#[derive(Clone)]
pub struct SudoRunner {
    pub inner: Arc<dyn Runner>,
}

impl SudoRunner {
    pub fn new(inner: Arc<dyn Runner>) -> Self {
        Self { inner }
    }
}

impl Runner for SudoRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let mut full = Vec::with_capacity(args.len() + 2);
        full.push("-n");
        full.push(program);
        full.extend_from_slice(args);

        let res = self.inner.run("sudo", &full)?;
        if !res.success && is_sudo_refusal(&res.combined) {
            return Err(SingSwitchError::sudo_not_configured(res.combined));
        }
        Ok(res)
    }
}
