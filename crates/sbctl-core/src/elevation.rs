use std::ffi::OsStr;

use crate::error::{Result, SbctlError};

/// Escape one argument so Windows `CommandLineToArgvW` reconstructs it verbatim.
pub fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg
        .bytes()
        .any(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | b'"'))
    {
        return arg.to_string();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                backslashes = 0;
                out.push('"');
            }
            _ => {
                if backslashes > 0 {
                    out.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                }
                out.push(ch);
            }
        }
    }
    if backslashes > 0 {
        out.push_str(&"\\".repeat(backslashes * 2));
    }
    out.push('"');
    out
}

/// Join multiple arguments into a Windows command-line string.
pub fn quote_args<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|s| quote_arg(s.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reports whether the current process is already sufficiently privileged to run operations
/// directly (always `true` on macOS, which elevates per command via `sudo -n`, and checks
/// the token elevation status on Windows).
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        windows_is_elevated()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Re-launch the given executable with `args` under Windows UAC elevation.
///
/// When `hidden` is true (used by GUI operations that exchange JSON via a temp file),
/// the elevated child runs with `SW_HIDE` so no console window flashes; CLI callers can
/// pass `hidden = false` to show output in an elevated console.
#[cfg(windows)]
pub fn run_elevated_executable(
    exe: &std::path::Path,
    args: &[String],
    hidden: bool,
) -> Result<i32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_CANCELLED, WAIT_FAILED,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

    fn to_wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = to_wide(OsStr::new("runas"));
    let file = to_wide(exe.as_os_str());
    let params = to_wide(OsStr::new(&quote_args(args)));

    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        hwnd: std::ptr::null_mut(),
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        lpDirectory: std::ptr::null(),
        nShow: if hidden { SW_HIDE } else { SW_SHOWNORMAL },
        hInstApp: std::ptr::null_mut(),
        lpIDList: std::ptr::null_mut(),
        lpClass: std::ptr::null(),
        hkeyClass: std::ptr::null_mut(),
        dwHotKey: 0,
        Anonymous: unsafe { std::mem::zeroed() },
        hProcess: std::ptr::null_mut(),
    };

    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return match code {
            ERROR_CANCELLED => Err(SbctlError::permission("elevation was declined")
                .with_hint("accept the Windows UAC prompt or install the sbctl helper service")),
            ERROR_ACCESS_DENIED => Err(SbctlError::permission("elevation was denied by policy")),
            _ => Err(SbctlError::permission(format!(
                "could not request Windows elevation (error {code})"
            ))),
        };
    }

    if sei.hProcess.is_null() {
        return Err(SbctlError::general(
            "elevated process started without a handle",
        ));
    }

    // Unbounded on purpose. One request is SyncAll, which fetches and validates
    // every sourced profile (30s + 20s each, uncapped), so any fixed deadline
    // here would abort real work; and killing it mid-write would strand a
    // two-step config replacement. The GUI holds a busy state and reports the
    // outcome, which is the signal the user acts on.
    let wait_res = unsafe { WaitForSingleObject(sei.hProcess, INFINITE) };
    if wait_res == WAIT_FAILED {
        unsafe { CloseHandle(sei.hProcess) };
        return Err(SbctlError::general("waiting for elevated process failed"));
    }

    let mut exit_code: u32 = 1;
    let code_ok = unsafe { GetExitCodeProcess(sei.hProcess, &mut exit_code) };
    unsafe { CloseHandle(sei.hProcess) };
    if code_ok == 0 {
        return Err(SbctlError::general(
            "could not read the elevated process exit code",
        ));
    }

    Ok(exit_code as i32)
}

#[cfg(not(windows))]
pub fn run_elevated_executable(
    _exe: &std::path::Path,
    _args: &[String],
    _hidden: bool,
) -> Result<i32> {
    let _ = OsStr::new("");
    Err(SbctlError::general(
        "Windows elevation is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn windows_is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}
