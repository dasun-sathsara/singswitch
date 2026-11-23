use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Result, SbctlError};

/// Exclusive advisory process lock guarding profile switching and deletion.
///
/// Prevents a tray click, CLI command, and GUI action from racing on the active
/// configuration symlink or copy.
#[derive(Debug)]
pub struct AdvisoryLock {
    _file: File,
}

impl AdvisoryLock {
    /// Acquire the lock at `lock_path`, waiting up to `timeout` if another `sbctl`
    /// operation is currently in progress.
    pub fn acquire(lock_path: &Path, timeout: Duration) -> Result<Self> {
        if let Some(parent) = lock_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;

        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => {
                    let _ = file.set_len(0);
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { _file: file });
                }
                Err(fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(SbctlError::service(
                            "another sbctl switch or update is already in progress",
                        )
                        .with_hint("wait for the active operation to finish and try again"));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(fs::TryLockError::Error(err)) => {
                    return Err(SbctlError::general(format!(
                        "could not lock {}: {err}",
                        lock_path.display()
                    ))
                    .with_cause(err));
                }
            }
        }
    }
}
