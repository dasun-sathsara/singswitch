//! Startup diagnostics for the desktop app.
//!
//! A bundled app has no terminal attached, so a panic or a failed tray install
//! would otherwise leave nothing behind. Every line lands in
//! `~/Library/Logs/sbctl/app.log` on macOS and `%LOCALAPPDATA%\sbctl\logs\app.log`
//! on Windows.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Path of the diagnostic log for this platform.
pub fn log_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(
            dirs::home_dir()?
                .join("Library")
                .join("Logs")
                .join("sbctl")
                .join("app.log"),
        )
    }
    #[cfg(windows)]
    {
        Some(
            dirs::data_local_dir()?
                .join("sbctl")
                .join("logs")
                .join("app.log"),
        )
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// Append one timestamped line. Failures are ignored: diagnostics must never
/// take the app down.
pub fn log(message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = writeln!(file, "{} {}", utc_timestamp(), message);
}

/// Record panics before the process dies.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(&format!("panic: {info}"));
        previous(info);
    }));
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Convert days since 1970-01-01 into a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_epoch_days_to_calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
