//! Privileged setup helpers that are not diagnostics.
//!
//! This module used to hold a full Doctor: a check engine, fix actions, and a
//! desktop screen for them. That screen is gone. What remains is the small set
//! of scripts and predicates that other code genuinely needs:
//!
//! - `diagnose_active`, used by `snapshot()` to explain a broken active config.
//! - The macOS log-permission script, used by the Logs repair action.

use crate::layout::Layout;
use crate::profile::validate_name;

/// Explain why the active profile cannot be loaded, if it cannot.
///
/// `None` means the active config looks usable. This is the only diagnostic the
/// app still needs: it turns a broken active symlink into a sentence in the
/// snapshot rather than a silent failure to connect.
pub fn diagnose_active(
    layout: &Layout,
    active: &str,
    profiles: &[crate::profile::Profile],
) -> Option<String> {
    if validate_name(active).is_err() || !profiles.iter().any(|p| p.name == active) {
        return Some(format!(
            "the active profile {active:?} no longer exists; the service has no configuration to load"
        ));
    }
    if let Err(err) = std::fs::metadata(&layout.active_config_path) {
        if err.kind() == std::io::ErrorKind::NotFound {
            return Some(format!(
                "{} is missing or points at a deleted file",
                layout.active_config_path.display()
            ));
        }
        return Some(format!(
            "{} cannot be read: {err}",
            layout.active_config_path.display()
        ));
    }
    None
}

/// Shell command granting console-user read access to `/var/log/sing-box`.
pub fn macos_logs_root_script(layout: &Layout) -> Option<String> {
    let dir = layout.error_log_path.parent()?;
    let access_dir = layout.access_log_path.parent()?;
    if dir != access_dir {
        return None;
    }
    let quoted = dir.display();
    Some(format!(
        "/bin/chmod 0755 '{quoted}' && /bin/chmod 0644 '{quoted}'/error.log '{quoted}'/access.log"
    ))
}
