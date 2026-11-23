use serde::{Deserialize, Serialize};

use crate::error::{ExitCode, Result, SingSwitchError};
use crate::service::Health;

/// Helper service name registered on Windows so the GUI can switch profiles without UAC prompts.
pub const HELPER_SERVICE_NAME: &str = "singswitch-agent";

#[cfg(windows)]
const PIPE_WAIT_TIMEOUT_MS: u32 = 400;
#[cfg(windows)]
const PIPE_MAX_INSTANCES: u32 = 4;
#[cfg(windows)]
const PIPE_BUFFER_BYTES: u32 = 65_536;
#[cfg(windows)]
const PIPE_DEFAULT_TIMEOUT_MS: u32 = 500;

/// Request sent from an unprivileged Windows GUI/CLI to the local `singswitch-agent` service
/// over `\\.\pipe\singswitch-control`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PipeRequest {
    Probe,
    Activate {
        name: String,
    },
    Stop,
    CreateProfile {
        name: String,
    },
    PullProfile {
        name: String,
        url: String,
    },
    SyncOne {
        name: String,
    },
    SyncAll,
    SaveProfile {
        name: String,
        content: String,
        source_url: Option<String>,
        #[serde(default)]
        allow_invalid: bool,
    },
    DeleteProfile {
        name: String,
        force: bool,
    },
    EnsureProfilesDir,
    RepairWindowsHelper,
}

/// Structured response returned by `singswitch-agent` or `singswitch --elevated-op`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipeResponse {
    pub ok: bool,
    pub code: i32,
    pub message: Option<String>,
    pub hints: Vec<String>,
    pub health: Option<Health>,
    #[serde(default)]
    pub reloaded: bool,
    #[serde(default)]
    pub synced: Vec<String>,
    #[serde(default)]
    pub failed: Vec<(String, String)>,
}

/// Largest request line the privileged helper will read from the pipe.
#[cfg(windows)]
const MAX_PIPE_REQUEST_BYTES: u64 = 1024 * 1024;

impl PipeResponse {
    pub fn success() -> Self {
        Self {
            ok: true,
            code: ExitCode::Ok.as_i32(),
            message: None,
            hints: Vec::new(),
            health: None,
            reloaded: false,
            synced: Vec::new(),
            failed: Vec::new(),
        }
    }

    pub fn with_reloaded(reloaded: bool) -> Self {
        Self {
            reloaded,
            ..Self::success()
        }
    }

    pub fn with_health(health: Health) -> Self {
        Self {
            health: Some(health),
            ..Self::success()
        }
    }

    pub fn with_sync_report(synced: Vec<String>, failed: Vec<(String, String)>) -> Self {
        Self {
            synced,
            failed,
            ..Self::success()
        }
    }

    pub fn from_error(err: &SingSwitchError) -> Self {
        Self {
            ok: false,
            code: err.code.as_i32(),
            message: Some(err.message.clone()),
            hints: err.hints.clone(),
            health: None,
            reloaded: false,
            synced: Vec::new(),
            failed: Vec::new(),
        }
    }

    pub fn into_result(self) -> Result<Option<Health>> {
        if self.ok {
            return Ok(self.health);
        }
        let mut err = SingSwitchError::new(
            ExitCode::from_i32(self.code),
            self.message
                .unwrap_or_else(|| "the Windows helper operation failed".to_string()),
        );
        err = err.with_hints(self.hints);
        Err(err)
    }
}

/// Send one JSON request to the Windows helper Named Pipe.
/// Returns `Ok(None)` when the helper service pipe is not present (allowing UAC fallback).
#[cfg(windows)]
pub fn try_send_pipe_request(pipe_name: &str, req: &PipeRequest) -> Result<Option<PipeResponse>> {
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let wide: Vec<u16> = std::ffi::OsStr::new(pipe_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ready = unsafe { WaitNamedPipeW(wide.as_ptr(), PIPE_WAIT_TIMEOUT_MS) };
    if ready == 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(None);
        }
    }

    let mut file = match OpenOptions::new().read(true).write(true).open(pipe_name) {
        Ok(f) => f,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(SingSwitchError::permission(
                    "not allowed to connect to the singswitch-agent named pipe",
                ));
            }
            return Ok(None);
        }
        Err(err) => return Err(err.into()),
    };

    let mut payload = serde_json::to_vec(req)
        .map_err(|e| SingSwitchError::general(format!("serialize pipe request: {e}")))?;
    payload.push(b'\n');
    file.write_all(&payload)?;
    file.flush()?;

    let mut reader = BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp: PipeResponse = serde_json::from_str(line.trim())
        .map_err(|e| SingSwitchError::service(format!("invalid helper response: {e}")))?;
    Ok(Some(resp))
}

#[cfg(not(windows))]
pub fn try_send_pipe_request(_pipe_name: &str, _req: &PipeRequest) -> Result<Option<PipeResponse>> {
    Ok(None)
}

#[cfg(windows)]
fn build_pipe_security_descriptor() -> Result<windows_sys::Win32::Security::PSECURITY_DESCRIPTOR> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::PSECURITY_DESCRIPTOR;

    // Grant FullControl to SYSTEM (SY) and Administrators (BA), and Read/Write to Interactive users (IU).
    let sddl: Vec<u16> = OsStr::new("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(SingSwitchError::general(format!(
            "could not build pipe security descriptor: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(sd)
}

/// Serve `\\.\pipe\singswitch-control` with an explicit SDDL DACL allowing `SYSTEM`,
/// `BUILTIN\Administrators`, and `BUILTIN\Users` (Interactive Console users can
/// request narrow profile switches without full admin rights).
#[cfg(windows)]
pub fn run_pipe_server<F>(
    pipe_name: &str,
    stop_signal: &std::sync::atomic::AtomicBool,
    handler: F,
) -> Result<()>
where
    F: Fn(PipeRequest) -> PipeResponse + Send + Sync,
{
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
        LocalFree,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let wide_pipe: Vec<u16> = OsStr::new(pipe_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let sd = build_pipe_security_descriptor()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };

    while !stop_signal.load(Ordering::Relaxed) {
        let handle = unsafe {
            CreateNamedPipeW(
                wide_pipe.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_MAX_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                PIPE_DEFAULT_TIMEOUT_MS,
                &sa,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            unsafe { LocalFree(sd) };
            return Err(SingSwitchError::service(format!(
                "CreateNamedPipeW failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
        let last_err = unsafe { GetLastError() };
        if connected == 0 && last_err != ERROR_PIPE_CONNECTED && last_err != ERROR_IO_PENDING {
            unsafe { CloseHandle(handle) };
            if stop_signal.load(Ordering::Relaxed) {
                break;
            }
            continue;
        }

        let mut file = unsafe { File::from_raw_handle(handle) };
        let mut line = String::new();
        {
            // Bounded: this pipe is served as SYSTEM, so an unbounded read lets
            // any interactive user grow privileged memory by streaming a line.
            let mut reader = BufReader::new(&file).take(MAX_PIPE_REQUEST_BYTES);
            let _ = reader.read_line(&mut line);
        }

        let resp = if line.trim().is_empty() {
            PipeResponse::from_error(&SingSwitchError::general("empty pipe request"))
        } else {
            match serde_json::from_str::<PipeRequest>(line.trim()) {
                Ok(req) => handler(req),
                Err(e) => PipeResponse::from_error(&SingSwitchError::general(format!(
                    "malformed pipe request: {e}"
                ))),
            }
        };

        if let Ok(mut out) = serde_json::to_vec(&resp) {
            out.push(b'\n');
            let _ = file.write_all(&out);
            let _ = file.flush();
        }
        unsafe { DisconnectNamedPipe(handle) };
        drop(file);
    }

    unsafe { LocalFree(sd) };
    Ok(())
}

#[cfg(not(windows))]
pub fn run_pipe_server<F>(
    _pipe_name: &str,
    _stop_signal: &std::sync::atomic::AtomicBool,
    _handler: F,
) -> Result<()>
where
    F: Fn(PipeRequest) -> PipeResponse + Send + Sync,
{
    Err(SingSwitchError::general(
        "Windows pipe server is only available on Windows",
    ))
}
