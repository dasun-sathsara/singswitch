use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{Result, SingSwitchError};
use crate::layout::resolve_singbox_binary;

/// Default validation timeout preventing a hung `sing-box` child process.
const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Validates sing-box configuration files and reports the `sing-box` binary version.
pub trait Checker: Send + Sync {
    fn check(&self, path: &Path) -> Result<()>;
    fn version(&self) -> Result<String>;
}

/// Validates configurations by executing the real `sing-box` binary.
#[derive(Debug, Clone)]
pub struct ExecChecker {
    pub binary: Option<PathBuf>,
    pub timeout: Duration,
}

impl Default for ExecChecker {
    fn default() -> Self {
        Self {
            binary: None,
            timeout: DEFAULT_CHECK_TIMEOUT,
        }
    }
}

impl ExecChecker {
    fn resolve(&self) -> Result<PathBuf> {
        resolve_singbox_binary(self.binary.as_deref()).ok_or_else(SingSwitchError::sing_box_missing)
    }

    fn run_singbox(&self, args: &[&str]) -> Result<(bool, String)> {
        let bin = self.resolve()?;
        let mut cmd = Command::new(&bin);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    SingSwitchError::sing_box_missing()
                } else {
                    SingSwitchError::general(format!("could not run {}: {err}", bin.display()))
                }
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
                return Err(SingSwitchError::general(format!(
                    "sing-box timed out after {}s",
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
        Ok((status.success(), combined))
    }
}

impl Checker for ExecChecker {
    fn check(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        let (ok, output) = self.run_singbox(&["check", "-c", &path_str])?;
        if ok {
            Ok(())
        } else {
            Err(SingSwitchError::invalid_config(path_str.as_ref(), output))
        }
    }

    fn version(&self) -> Result<String> {
        let (ok, output) = self.run_singbox(&["version"])?;
        if !ok {
            return Err(SingSwitchError::general(format!(
                "sing-box version failed: {}",
                output.trim()
            )));
        }
        let first = output.lines().next().unwrap_or("").trim();
        let parts: Vec<&str> = first.split_whitespace().collect();
        if parts.len() >= 3 && parts[1] == "version" {
            Ok(parts[2].to_string())
        } else {
            Ok(first.to_string())
        }
    }
}

/// Write `bytes` to a temporary `.json` file and validate it through `checker`.
pub(crate) fn check_temp_bytes(checker: &dyn Checker, prefix: &str, bytes: &[u8]) -> Result<()> {
    let mut tmp = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".json")
        .tempfile()?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    checker.check(tmp.path())
}

fn read_pipe(pipe: Option<impl std::io::Read>) -> String {
    let Some(mut reader) = pipe else {
        return String::new();
    };
    let mut buf = String::new();
    let _ = reader.read_to_string(&mut buf);
    buf
}
