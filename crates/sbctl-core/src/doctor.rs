use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::activator::Activator;
use crate::checker::Checker;
use crate::error::first_line;
use crate::layout::{Layout, PlatformKind};
use crate::profile::{list_profiles, validate_name};
use crate::runner::Runner;
use crate::service::win_rpc::{HELPER_SERVICE_NAME, PipeRequest, try_send_pipe_request};
use crate::service::{Manager, RunState, is_sudo_refusal};

/// One-click remediation action offered by the desktop Doctor view for a diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixAction {
    CreateProfilesDir,
    RepairMacosSudoers,
    RepairWindowsHelper,
    OpenProfiles,
}

/// Outcome of a single diagnostic check reported by `sbctl doctor` and the Doctor GUI view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub name: String,
    pub ok: bool,
    pub warning: bool,
    pub detail: String,
    pub hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixAction>,
}

pub type SudoProbeFn = Arc<dyn Fn(&[String]) -> std::result::Result<(), String> + Send + Sync>;

/// Run `sudo -n -l <argv>` to confirm whether a command rule matches without executing it.
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

/// Run the full diagnostic suite across dependencies, profiles, privileges, service, and lock.
pub fn run_doctor_checks(
    layout: &Layout,
    checker: &dyn Checker,
    manager: &dyn Manager,
    activator: &dyn Activator,
    runner: &dyn Runner,
    sudo_probe: Option<&SudoProbeFn>,
) -> Vec<DiagnosticCheck> {
    let mut checks = Vec::new();
    checks.push(check_singbox(checker));
    checks.push(check_profiles_dir(layout));

    if layout.uses_sudo() {
        checks.extend(check_sudo_rules(layout, sudo_probe));
    } else if layout.platform == PlatformKind::Windows {
        checks.push(check_windows_helper_service(layout, runner));
    }

    checks.push(check_service(layout, manager));
    checks.push(check_active_config(layout, activator));
    checks.push(check_concurrency());
    checks
}

fn check_singbox(checker: &dyn Checker) -> DiagnosticCheck {
    match checker.version() {
        Ok(version) => DiagnosticCheck {
            name: "sing-box".to_string(),
            ok: true,
            warning: false,
            detail: format!("version {version}"),
            hint: String::new(),
            fix: None,
        },
        Err(_) => DiagnosticCheck {
            name: "sing-box".to_string(),
            ok: false,
            warning: false,
            detail: "not found on PATH or standard install directories".to_string(),
            hint: "install sing-box (Homebrew on macOS, WinGet on Windows) or run the installer"
                .to_string(),
            fix: None,
        },
    }
}

fn check_profiles_dir(layout: &Layout) -> DiagnosticCheck {
    let Ok(meta) = std::fs::metadata(&layout.profiles_dir) else {
        return DiagnosticCheck {
            name: "profiles".to_string(),
            ok: false,
            warning: false,
            detail: format!("{} does not exist", layout.profiles_dir.display()),
            hint: "create it with the platform installer".to_string(),
            fix: Some(FixAction::CreateProfilesDir),
        };
    };
    if !meta.is_dir() {
        return DiagnosticCheck {
            name: "profiles".to_string(),
            ok: false,
            warning: false,
            detail: format!("{} is not a directory", layout.profiles_dir.display()),
            hint: "move it aside and re-run the platform installer".to_string(),
            fix: None,
        };
    }

    let profiles = match list_profiles(&layout.profiles_dir) {
        Ok(list) => list,
        Err(err) => {
            return DiagnosticCheck {
                name: "profiles".to_string(),
                ok: false,
                warning: false,
                detail: err.to_string(),
                hint: "repair directory ownership with the platform installer".to_string(),
                fix: None,
            };
        }
    };

    let incomplete = profiles.iter().filter(|p| !p.ready()).count();
    let detail = format!("{} in {}", profiles.len(), layout.profiles_dir.display());
    if incomplete > 0 {
        DiagnosticCheck {
            name: "profiles".to_string(),
            ok: true,
            warning: true,
            detail: format!("{detail} ({incomplete} still have placeholders)"),
            hint: "replace all TODO_ placeholders before activating".to_string(),
            fix: Some(FixAction::OpenProfiles),
        }
    } else {
        DiagnosticCheck {
            name: "profiles".to_string(),
            ok: true,
            warning: false,
            detail,
            hint: String::new(),
            fix: None,
        }
    }
}

fn check_sudo_rules(layout: &Layout, custom_probe: Option<&SudoProbeFn>) -> Vec<DiagnosticCheck> {
    let commands = layout.sudo_commands();
    let mut checks = Vec::with_capacity(commands.len());

    for cmd in commands {
        let argv = cmd.probe_argv();
        let result = match custom_probe {
            Some(probe) => probe(&argv),
            None => real_sudo_probe(&argv),
        };
        match result {
            Ok(()) => checks.push(DiagnosticCheck {
                name: format!("sudo: {}", cmd.name),
                ok: true,
                warning: false,
                detail: "permitted".to_string(),
                hint: String::new(),
                fix: None,
            }),
            Err(err) => checks.push(DiagnosticCheck {
                name: format!("sudo: {}", cmd.name),
                ok: false,
                warning: false,
                detail: summarise_sudo(&err),
                hint: "reinstall scoped sudo rules with: sudo bash scripts/install-macos.sh"
                    .to_string(),
                fix: Some(FixAction::RepairMacosSudoers),
            }),
        }
    }
    checks
}

fn summarise_sudo(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "not permitted".to_string();
    }
    if is_sudo_refusal(trimmed) {
        return "not permitted without a password".to_string();
    }
    first_line(trimmed).to_string()
}

fn check_windows_helper_service(layout: &Layout, runner: &dyn Runner) -> DiagnosticCheck {
    if let Ok(Some(_)) = try_send_pipe_request(&layout.pipe_name, &PipeRequest::Probe) {
        return DiagnosticCheck {
            name: "helper service".to_string(),
            ok: true,
            warning: false,
            detail: format!("{HELPER_SERVICE_NAME} pipe ready (no UAC needed for switches)"),
            hint: String::new(),
            fix: None,
        };
    }

    match runner.run("sc.exe", &["queryex", HELPER_SERVICE_NAME]) {
        Ok(out) if out.success && out.combined.to_ascii_uppercase().contains("RUNNING") => {
            DiagnosticCheck {
                name: "helper service".to_string(),
                ok: true,
                warning: true,
                detail: format!("{HELPER_SERVICE_NAME} is running, pipe not reachable"),
                hint: "restart the sbctl-agent service or check permissions".to_string(),
                fix: Some(FixAction::RepairWindowsHelper),
            }
        }
        _ => DiagnosticCheck {
            name: "helper service".to_string(),
            ok: true,
            warning: true,
            detail: format!("{HELPER_SERVICE_NAME} is not running; UAC prompt will be used"),
            hint: "run scripts\\install.ps1 as Administrator for passwordless tray switches"
                .to_string(),
            fix: Some(FixAction::RepairWindowsHelper),
        },
    }
}

fn check_service(layout: &Layout, manager: &dyn Manager) -> DiagnosticCheck {
    let install_hint = match layout.platform {
        PlatformKind::Darwin => {
            "register the LaunchDaemon with: sudo bash scripts/install-macos.sh"
        }
        PlatformKind::Windows => "register the WinSW service with: scripts\\install.ps1",
    };

    match manager.probe() {
        Ok(health) => {
            let mut detail = health.state.to_string();
            if let Some(pid) = health.pid
                && pid > 0
            {
                detail.push_str(&format!(" (pid {pid})"));
            }
            DiagnosticCheck {
                name: "service".to_string(),
                ok: health.state != RunState::Error,
                warning: false,
                detail,
                hint: if health.state == RunState::Error {
                    install_hint.to_string()
                } else {
                    String::new()
                },
                fix: None,
            }
        }
        Err(err) => DiagnosticCheck {
            name: "service".to_string(),
            ok: false,
            warning: false,
            detail: err.to_string(),
            hint: install_hint.to_string(),
            fix: None,
        },
    }
}

fn check_active_config(layout: &Layout, activator: &dyn Activator) -> DiagnosticCheck {
    let active = match activator.active_name() {
        Ok(name) => name,
        Err(err) => {
            return DiagnosticCheck {
                name: "active config".to_string(),
                ok: false,
                warning: false,
                detail: err.to_string(),
                hint: "select a valid profile with: sbctl use <name>".to_string(),
                fix: Some(FixAction::OpenProfiles),
            };
        }
    };

    let Some(active_name) = active else {
        return DiagnosticCheck {
            name: "active config".to_string(),
            ok: true,
            warning: true,
            detail: "no profile selected".to_string(),
            hint: "choose a profile with: sbctl use <name>".to_string(),
            fix: Some(FixAction::OpenProfiles),
        };
    };

    let profiles = list_profiles(&layout.profiles_dir).unwrap_or_default();
    if let Some(problem) = diagnose_active(layout, &active_name, &profiles) {
        return DiagnosticCheck {
            name: "active config".to_string(),
            ok: false,
            warning: false,
            detail: problem,
            hint: "point it at a valid profile with: sbctl use <name>".to_string(),
            fix: Some(FixAction::OpenProfiles),
        };
    }

    DiagnosticCheck {
        name: "active config".to_string(),
        ok: true,
        warning: false,
        detail: active_name,
        hint: String::new(),
        fix: None,
    }
}

fn check_concurrency() -> DiagnosticCheck {
    DiagnosticCheck {
        name: "concurrency lock".to_string(),
        ok: true,
        warning: false,
        detail: "advisory file lock serializes switches across GUI, tray, and CLI".to_string(),
        hint: String::new(),
        fix: None,
    }
}

/// Diagnose whether the configured active profile points at a deleted or missing file.
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

/// Generate a shell command that stages `/etc/sudoers.d/sbctl` to a temporary file,
/// validates it with `visudo -cf`, and installs it atomically with `0440` root:wheel.
pub fn macos_sudoers_install_command(layout: &Layout, user: &str) -> crate::error::Result<String> {
    let body = macos_sudoers_root_script(layout, user)?;
    Ok(format!("sudo sh -c '{}'", body.replace('\'', "'\"'\"'")))
}

/// Generate a self-contained root shell script suitable for `osascript ... with administrator privileges`
/// that stages rules to a temp file, validates with `visudo -cf`, and installs `/etc/sudoers.d/sbctl`.
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
    use crate::activator::CopyActivator;
    use crate::checker::FakeChecker;
    use crate::runner::{FakeResult, FakeRunner};
    use crate::service::Health;

    struct StaticManager;
    impl Manager for StaticManager {
        fn probe(&self) -> crate::error::Result<Health> {
            Ok(Health::unknown(RunState::Stopped))
        }
        fn restart(&self) -> crate::error::Result<()> {
            Ok(())
        }
        fn stop(&self) -> crate::error::Result<()> {
            Ok(())
        }
    }

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
    fn doctor_assigns_expected_fix_actions() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = Layout::windows();
        layout.profiles_dir = dir.path().join("missing-profiles");
        layout.active_config_path = dir.path().join("missing-active.json");
        layout.active_name_path = Some(dir.path().join("active-name"));
        layout.pipe_name = r"\\.\pipe\sbctl-test-nonexistent".to_string();

        let checker = FakeChecker::default();
        let runner = FakeRunner::new(FakeResult::err(1060, "service does not exist"));
        let activator = CopyActivator::new(
            layout.active_config_path.clone(),
            layout.active_name_path.clone().unwrap(),
        );

        let checks =
            run_doctor_checks(&layout, &checker, &StaticManager, &activator, &runner, None);
        let prof_chk = checks.iter().find(|c| c.name == "profiles").unwrap();
        assert_eq!(prof_chk.fix, Some(FixAction::CreateProfilesDir));

        let helper_chk = checks.iter().find(|c| c.name == "helper service").unwrap();
        assert_eq!(helper_chk.fix, Some(FixAction::RepairWindowsHelper));

        let active_chk = checks.iter().find(|c| c.name == "active config").unwrap();
        assert_eq!(active_chk.fix, Some(FixAction::OpenProfiles));
    }
}
