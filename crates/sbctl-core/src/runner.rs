use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Result, SbctlError};
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
            SbctlError::general(format!("could not run {program}: {err}")).with_cause(err)
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
                return Err(SbctlError::service(format!(
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
/// `SbctlError::sudo_not_configured`.
///
/// Using `-n` guarantees sbctl never hangs waiting for a terminal password inside a GUI
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
            return Err(SbctlError::sudo_not_configured(res.combined));
        }
        Ok(res)
    }
}

/// Canned response for `FakeRunner`.
#[derive(Debug, Clone)]
pub struct FakeResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output: String,
}

impl FakeResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            exit_code: Some(0),
            output: output.into(),
        }
    }

    pub fn err(code: i32, output: impl Into<String>) -> Self {
        Self {
            success: false,
            exit_code: Some(code),
            output: output.into(),
        }
    }
}

/// Test `Runner` matching longest command-line prefix and recording every argv.
pub struct FakeRunner {
    responses: Mutex<HashMap<String, Vec<FakeResult>>>,
    default: FakeResult,
    pub calls: Mutex<Vec<Vec<String>>>,
}

impl Default for FakeRunner {
    fn default() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default: FakeResult::ok(""),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl FakeRunner {
    pub fn new(default: FakeResult) -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Set a single canned response for a command-line prefix.
    pub fn set_response(&self, prefix: impl Into<String>, result: FakeResult) {
        self.responses
            .lock()
            .unwrap()
            .insert(prefix.into(), vec![result]);
    }

    /// Queue multiple sequential responses for repeated calls matching `prefix`.
    /// Once exhausted, the final entry repeats.
    pub fn queue_responses(&self, prefix: impl Into<String>, results: Vec<FakeResult>) {
        self.responses
            .lock()
            .unwrap()
            .insert(prefix.into(), results);
    }

    pub fn call_lines(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|argv| argv.join(" "))
            .collect()
    }
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(program.to_string());
        argv.extend(args.iter().map(|s| (*s).to_string()));
        let line = argv.join(" ");
        self.calls.lock().unwrap().push(argv);

        let mut map = self.responses.lock().unwrap();
        let best_key = map
            .keys()
            .filter(|prefix| line.starts_with(prefix.as_str()))
            .max_by_key(|prefix| prefix.len())
            .cloned();

        let chosen = if let Some(key) = best_key {
            let queue = map.get_mut(&key).expect("key exists");
            if queue.len() > 1 {
                queue.remove(0)
            } else {
                queue
                    .first()
                    .cloned()
                    .unwrap_or_else(|| self.default.clone())
            }
        } else {
            self.default.clone()
        };

        Ok(CommandOutput {
            success: chosen.success,
            exit_code: chosen.exit_code,
            combined: chosen.output,
        })
    }
}
