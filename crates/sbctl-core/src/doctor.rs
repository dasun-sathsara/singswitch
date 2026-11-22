//! Privileged setup helpers that are not diagnostics.
//!
//! This module used to hold a full Doctor: a check engine, fix actions, and a
//! desktop screen for them. That screen is gone. What remains is the small set
//! of scripts and predicates that other code genuinely needs:
//!
//! - `diagnose_active`, used by `snapshot()` to explain a broken active config.
//! - The macOS sudoers/log install scripts, used by the installer and repairs.
//! - `real_sudo_probe`, used to verify a sudoers rule is actually in force.

use std::process::Command;

use crate::layout::Layout;
use crate::profile::validate_name;

/// Probe a sudoers rule with `sudo -n -l <argv>`, returning why it is refused.
///
/// `sudo -n -l` lists what the rule permits without running the command.
pub fn real_sudo_probe(argv: &[String]) -> std::result::Result<(), String> {
    let mut probe_args = Vec::with_capacity(argv.len() + 2);
    probe_args.push("-n");
    probe_args.push("-l");
    for a in argv {
        probe_args.push(a.as_str());
    }
    let out = Command::new("sudo")
        .args(&probe_args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        Err("not permitted".to_string())
    } else {
        Err(trimmed.to_string())
    }
}

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

/// Shell command that stages `/etc/sudoers.d/sbctl`, validates it with
/// `visudo -cf`, and installs it atomically as `0440 root:wheel`.
pub fn macos_sudoers_install_command(layout: &Layout, user: &str) -> crate::error::Result<String> {
    let body = macos_sudoers_root_script(layout, user)?;
    Ok(format!("sudo sh -c '{}'", body.replace('\'', "'\"'\"'")))
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

/// The root script body behind [`macos_sudoers_install_command`].
pub fn macos_sudoers_root_script(layout: &Layout, user: &str) -> crate::error::Result<String> {
    let rules = layout.sudoers(user)?;
    let encoded = rules.replace('\'', "'\"'\"'");
    Ok(format!(
        "tmp=$(mktemp /tmp/sbctl-sudoers.XXXXXX) && trap 'rm -f \"$tmp\"' EXIT && \
         printf '%s' '{encoded}' > \"$tmp\" && chmod 0440 \"$tmp\" && \
         /usr/sbin/visudo -cf \"$tmp\" && \
         /usr/bin/install -m 0440 -o root -g wheel \"$tmp\" /etc/sudoers.d/sbctl"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudoers_commands_stage_and_validate_with_visudo_before_install() {
        let layout = Layout::darwin();
        let root_script = macos_sudoers_root_script(&layout, "alice").unwrap();
        assert!(root_script.contains("mktemp"));
        assert!(root_script.contains("visudo -cf"));
        assert!(root_script.contains("install -m 0440 -o root -g wheel"));
        assert!(!root_script.contains("| sudo tee"));

        let user_cmd = macos_sudoers_install_command(&layout, "alice").unwrap();
        assert!(user_cmd.starts_with("sudo sh -c "));
        assert!(user_cmd.contains("visudo -cf"));
    }

    #[test]
    fn diagnose_active_reports_missing_profile_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = Layout::darwin();
        layout.active_config_path = dir.path().join("config.json");

        // No such profile.
        assert!(diagnose_active(&layout, "gone", &[]).is_some());

        // Profile exists but the active config does not.
        let profiles = vec![crate::profile::Profile {
            name: "work".to_string(),
            path: dir.path().join("work.json"),
            missing: Vec::new(),
            server: None,
            tun: None,
            source_url: None,
            config_parsed: true,
            facts: Default::default(),
        }];
        let msg = diagnose_active(&layout, "work", &profiles).expect("missing config is reported");
        assert!(msg.contains("missing") || msg.contains("deleted"));

        // Now it exists and the active name resolves.
        std::fs::write(&layout.active_config_path, "{}").unwrap();
        assert!(diagnose_active(&layout, "work", &profiles).is_none());
    }
}
