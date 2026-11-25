use std::io::Read as _;
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

        let child = cmd.spawn().map_err(|err| {
            SingSwitchError::general(format!("could not run {program}: {err}")).with_cause(err)
        })?;

        let program_for_error = program.to_string();
        let (status, combined) = wait_with_output(child, self.timeout, move || {
            SingSwitchError::service(format!(
                "{program_for_error} timed out after {}s",
                self.timeout.as_secs()
            ))
            .with_hint("check the command is not waiting on stdin")
        })?;

        Ok(CommandOutput {
            success: status.success(),
            combined,
        })
    }
}

/// Cap on captured pipe output, shared with the checker's own `read_pipe`.
/// None of the commands run through either capture path legitimately emit
/// megabytes — this guards a wedged child (e.g. a `launchctl print` over a
/// huge domain) from holding its whole output in resident memory.
pub(crate) const MAX_CAPTURE_BYTES: u64 = 64 * 1024;

/// Run `child` to completion under a deadline, capturing stdout and stderr
/// (bounded by `MAX_CAPTURE_BYTES`) into one combined string.
///
/// Shared by `ExecRunner` and `ExecChecker` so the two capture paths cannot
/// drift apart again — they already had (one capped its reads, only one
/// drained its reader threads on timeout). On timeout the child is killed,
/// both reader threads are joined (draining the pipes), and the timed-out
/// error the caller supplies is augmented with whatever the child printed.
pub(crate) fn wait_with_output(
    mut child: std::process::Child,
    timeout: Duration,
    timed_out: impl Fn() -> SingSwitchError,
) -> Result<(std::process::ExitStatus, String)> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || read_pipe(stdout));
    let err_handle = std::thread::spawn(move || read_pipe(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // Kill closes the pipes; joining here (rather than only on the
            // success path) drains the reader threads so the timeout does not
            // leak two join handles holding the captured output.
            let out = out_handle.join().unwrap_or_default();
            let err = err_handle.join().unwrap_or_default();
            let tail = combine_output(out, err);
            // The tail belongs in the message, not the cause: the GUI banners
            // print `message` and `hints` only, and the child's last words are
            // the only clue to a wedge.
            let mut error = timed_out();
            if !tail.is_empty() {
                let mut message = error.to_string();
                message.push_str(":\n");
                message.push_str(&tail);
                error = error.with_message(message);
            }
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let out = out_handle.join().unwrap_or_default();
    let err = err_handle.join().unwrap_or_default();
    Ok((status, combine_output(out, err)))
}

/// Join captured stdout/stderr into the single string callers print.
fn combine_output(out: String, err: String) -> String {
    match (out.trim().is_empty(), err.trim().is_empty()) {
        (false, false) => format!("{}\n{}", out.trim(), err.trim()),
        (false, true) => out,
        (true, false) => err,
        (true, true) => String::new(),
    }
}

fn read_pipe(pipe: Option<impl std::io::Read>) -> String {
    let Some(reader) = pipe else {
        return String::new();
    };
    // Lossy decode, never `read_to_string`: that fails on any invalid UTF-8
    // and leaves the buffer empty, so one stray byte in a child's diagnostic
    // turned the whole message into nothing.
    let mut bytes = Vec::new();
    let _ = reader.take(MAX_CAPTURE_BYTES).read_to_end(&mut bytes);
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    }
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
