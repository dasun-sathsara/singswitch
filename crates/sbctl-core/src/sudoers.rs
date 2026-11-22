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
        // No `activate` rule: swapping the active symlink needs no root, because
        // the installer hands the whole config directory to the target user
        // (they already write the profile contents it points at). The old rule
        // granted `ln -sfn <profiles>/*.json <config>`, and sudo matches the
        // whole argv as one string, so the `*` crossed `/` and the grant covered
        // `../` traversal to any file on the machine.
        vec![
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
    fn no_rule_grants_a_globbed_or_traversable_path() {
        let layout = Layout::darwin();
        let rendered = layout.sudoers("dasun").expect("sudoers should render");

        // sudo matches the whole argv as one space-joined string, so a `*` in a
        // rule also matches `/` and `..`. No rule may contain a wildcard, and
        // the activate grant is gone entirely rather than made "safe".
        assert!(
            !rendered.contains("*.json"),
            "a globbed rule permits traversal:\n{rendered}"
        );
        assert!(
            !rendered.contains(".."),
            "a rule may not name a parent path:\n{rendered}"
        );
        assert!(
            !layout.sudo_commands().iter().any(|c| c.name == "activate"),
            "activating needs no root: the installer hands over the config dir"
        );
    }

    #[test]
    fn sudoers_renders_the_expected_rules() {
        let layout = Layout::darwin();
        let rendered = layout.sudoers("dasun").expect("sudoers should render");
        let lines: Vec<_> = rendered
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert_eq!(lines.len(), layout.sudo_commands().len());
        assert_eq!(lines.len(), 4, "activate is no longer privileged");
        assert!(
            lines
                .iter()
                .all(|l| l.starts_with("dasun ALL=(root) NOPASSWD: "))
        );
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
