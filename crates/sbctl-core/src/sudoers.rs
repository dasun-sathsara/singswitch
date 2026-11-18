use crate::error::{Result, SbctlError};
use crate::layout::Layout;

/// One privileged command pattern sbctl is authorised to execute via `sudo -n`.
///
/// Both the generated `/etc/sudoers.d/sbctl` rules and the runtime argument lists derive
/// from `Layout::sudo_commands`, ensuring they cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SudoCommand {
    pub name: &'static str,
    pub argv: Vec<String>,
}

impl SudoCommand {
    /// Render the command as written in a sudoers rule line.
    pub fn render_rule_command(&self) -> String {
        self.argv.join(" ")
    }

    /// Check whether a concrete argv would be authorised by this sudoers pattern.
    ///
    /// Implements `fnmatch(3)` with `FNM_PATHNAME` semantics so `*` never crosses `/`.
    pub fn matches(&self, argv: &[String]) -> bool {
        if argv.len() != self.argv.len() {
            return false;
        }
        self.argv
            .iter()
            .zip(argv.iter())
            .all(|(pattern, candidate)| fnmatch_pathname(pattern, candidate))
    }

    /// Substitute `*.json` with a concrete probe filename for `sudo -n -l` verification.
    pub fn probe_argv(&self) -> Vec<String> {
        self.argv
            .iter()
            .map(|arg| arg.replace("*.json", "probe.json"))
            .collect()
    }
}

impl Layout {
    /// Return every privileged command required on this layout, in sudoers order.
    pub fn sudo_commands(&self) -> Vec<SudoCommand> {
        if !self.uses_sudo() {
            return Vec::new();
        }
        let profile_glob = format!("{}/{}", slash_path(&self.profiles_dir), "*.json");
        vec![
            SudoCommand {
                name: "activate",
                argv: vec![
                    slash_path(&self.ln_bin),
                    "-sfn".to_string(),
                    profile_glob,
                    slash_path(&self.active_config_path),
                ],
            },
            SudoCommand {
                name: "probe",
                argv: vec![
                    slash_path(&self.ctl_bin),
                    "print".to_string(),
                    self.launchd_label.clone(),
                ],
            },
            SudoCommand {
                name: "restart",
                argv: vec![
                    slash_path(&self.ctl_bin),
                    "kickstart".to_string(),
                    "-k".to_string(),
                    self.launchd_label.clone(),
                ],
            },
            SudoCommand {
                name: "stop",
                argv: vec![
                    slash_path(&self.ctl_bin),
                    "bootout".to_string(),
                    self.launchd_label.clone(),
                ],
            },
            SudoCommand {
                name: "bootstrap",
                argv: vec![
                    slash_path(&self.ctl_bin),
                    "bootstrap".to_string(),
                    "system".to_string(),
                    slash_path(&self.plist_path),
                ],
            },
        ]
    }

    /// Render the complete `/etc/sudoers.d/sbctl` file for `user`.
    pub fn sudoers(&self, user: &str) -> Result<String> {
        if !self.uses_sudo() {
            return Err(SbctlError::general(format!(
                "sudoers is not used on {}",
                self.platform.as_str()
            )));
        }
        validate_sudoers_user(user)?;

        let mut out = String::new();
        out.push_str("# Managed by sbctl. Do not edit by hand.\n");
        out.push_str(&format!(
            "# Regenerate with: sbctl print-sudoers --user {user}\n"
        ));
        out.push_str("#\n");
        out.push_str("# Each rule is scoped to one exact command so that a compromised\n");
        out.push_str("# user account cannot use these grants to run arbitrary code as root.\n");
        for cmd in self.sudo_commands() {
            out.push_str(&format!(
                "{user} ALL=(root) NOPASSWD: {}\n",
                cmd.render_rule_command()
            ));
        }
        Ok(out)
    }
}

/// Reject usernames that could inject extra sudoers directives or whitespace.
pub fn validate_sudoers_user(user: &str) -> Result<()> {
    if user.is_empty() {
        return Err(SbctlError::general("sudoers user must not be empty"));
    }
    if user.len() > 32 {
        return Err(SbctlError::general(format!(
            "sudoers user {user:?} is too long"
        )));
    }
    for ch in user.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.') {
            return Err(SbctlError::general(format!(
                "sudoers user {user:?} contains an unsupported character {ch:?}"
            )));
        }
    }
    Ok(())
}

fn slash_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Match `pattern` against `text` using POSIX `fnmatch(FNM_PATHNAME)` rules:
/// `*` matches zero or more non-`/` characters, `?` matches one non-`/` character.
pub fn fnmatch_pathname(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => {
                // Skip repeated asterisks.
                let mut rest = &p[1..];
                while rest.first() == Some(&b'*') {
                    rest = &rest[1..];
                }
                if inner(rest, t) {
                    return true;
                }
                if let Some(&tc) = t.first()
                    && tc != b'/'
                {
                    return inner(p, &t[1..]);
                }
                false
            }
            (Some(b'?'), Some(&tc)) if tc != b'/' => inner(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) if pc == tc => inner(&p[1..], &t[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::validate_name;

    #[test]
    fn sudoers_blocks_traversal_and_matches_valid_profiles() {
        let layout = Layout::darwin();
        let activate = &layout.sudo_commands()[0];

        let valid = layout.activate_argv(std::path::Path::new(
            "/usr/local/etc/sing-box/profiles/aws-sg.json",
        ));
        assert!(activate.matches(&valid));

        let traversal = layout.activate_argv(std::path::Path::new(
            "/usr/local/etc/sing-box/profiles/../../../etc/shadow.json",
        ));
        assert!(!activate.matches(&traversal));

        let wrong_dest = vec![
            layout.ln_bin.to_string_lossy().into_owned(),
            "-sfn".to_string(),
            "/usr/local/etc/sing-box/profiles/aws-sg.json".to_string(),
            "/etc/hosts".to_string(),
        ];
        assert!(!activate.matches(&wrong_dest));
    }

    #[test]
    fn sudoers_renders_expected_five_rules() {
        let layout = Layout::darwin();
        let rendered = layout.sudoers("dasun").expect("sudoers should render");
        let lines: Vec<_> = rendered
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("dasun ALL=(root) NOPASSWD: "));
        assert!(lines[0].contains("/usr/local/etc/sing-box/profiles/*.json"));
        assert!(validate_name("aws-sg").is_ok());
    }

    #[test]
    fn sudoers_rejects_malicious_user() {
        let layout = Layout::darwin();
        assert!(layout.sudoers("alice\nroot").is_err());
        assert!(layout.sudoers("alice ALL=(ALL) ALL").is_err());
        assert!(layout.sudoers("").is_err());
    }
}
