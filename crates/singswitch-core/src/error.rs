use std::fmt;
use std::ops::{Deref, DerefMut};

/// Process exit codes grouped by failure cause so scripts and GUIs can react differently
/// to validation errors, service failures, and missing permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Success, including user cancellation of an interactive prompt.
    Ok = 0,
    /// Generic failure: bad usage, missing profile, I/O, or network trouble.
    General = 1,
    /// Configuration rejected: required fields are missing or `sing-box check` failed.
    Validation = 2,
    /// Service failure: failed to start, stop, or stay running after activation.
    Service = 3,
    /// Missing privileges: scoped sudo rules absent or Windows elevation declined.
    Permission = 4,
}

impl ExitCode {
    /// Numeric exit status for the process.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Reconstruct an `ExitCode` from a child process exit status.
    pub fn from_i32(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            2 => Self::Validation,
            3 => Self::Service,
            4 => Self::Permission,
            _ => Self::General,
        }
    }
}

/// Sentinel error types for precise classification across core and UI layers.
#[derive(Debug, thiserror::Error)]
pub enum CoreErrorKind {
    #[error("sing-box was not found on PATH or standard install locations")]
    SingBoxMissing,

    #[error("singswitch does not have permission to manage the sing-box service{detail}")]
    SudoNotConfigured { detail: String },

    #[error("invalid profile name: {0}")]
    InvalidName(String),

    #[error("sing-box rejected {path}: {detail}")]
    InvalidConfig { path: String, detail: String },
}

#[derive(Debug)]
pub struct SingSwitchErrorInner {
    pub code: ExitCode,
    pub message: String,
    pub hints: Vec<String>,
    pub cause: Option<String>,
    pub reported: bool,
    pub kind: Option<CoreErrorKind>,
}

/// User-facing failure carrying a structured exit code, plain message, and concrete next steps.
/// Boxed internally so `Result<T, SingSwitchError>` stays pointer-sized on the stack.
#[derive(Debug)]
pub struct SingSwitchError(Box<SingSwitchErrorInner>);

impl Deref for SingSwitchError {
    type Target = SingSwitchErrorInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SingSwitchError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl SingSwitchError {
    pub fn new(code: ExitCode, message: impl Into<String>) -> Self {
        Self(Box::new(SingSwitchErrorInner {
            code,
            message: message.into(),
            hints: Vec::new(),
            cause: None,
            reported: false,
            kind: None,
        }))
    }

    pub fn general(message: impl Into<String>) -> Self {
        Self::new(ExitCode::General, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ExitCode::Validation, message)
    }

    pub fn service(message: impl Into<String>) -> Self {
        Self::new(ExitCode::Service, message)
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::new(ExitCode::Permission, message)
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.0.hints.push(hint.into());
        self
    }

    pub fn with_hints<I, S>(mut self, hints: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.hints.extend(hints.into_iter().map(Into::into));
        self
    }

    pub fn with_cause(mut self, cause: impl fmt::Display) -> Self {
        self.0.cause = Some(cause.to_string());
        self
    }

    pub fn with_kind(mut self, kind: CoreErrorKind) -> Self {
        self.0.kind = Some(kind);
        self
    }

    pub fn mark_reported(mut self) -> Self {
        self.0.reported = true;
        self
    }

    pub fn sing_box_missing() -> Self {
        Self::general("sing-box is not installed, or could not be found")
            .with_hint(
                "reinstall singswitch so its bundled sing-box lands in the privileged location, or set SINGSWITCH_SINGBOX_BIN",
            )
            .with_kind(CoreErrorKind::SingBoxMissing)
    }

    pub fn sudo_not_configured(detail: impl Into<String>) -> Self {
        let detail_str = detail.into();
        let suffix = if detail_str.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", first_line(detail_str.trim()))
        };
        Self::permission(
            "singswitch is not allowed to manage the sing-box service without a password",
        )
        .with_hints([
            "repair the scoped sudo rules with: sudo bash scripts/install-macos.sh",
            "then confirm with: singswitch status",
        ])
        .with_cause(detail_str)
        .with_kind(CoreErrorKind::SudoNotConfigured { detail: suffix })
    }

    pub fn invalid_name(reason: impl Into<String>) -> Self {
        let reason_str = reason.into();
        Self::general(format!("invalid profile name: {reason_str}"))
            .with_hint("use letters, digits, dot, dash, or underscore")
            .with_kind(CoreErrorKind::InvalidName(reason_str))
    }

    pub fn invalid_config(path: impl Into<String>, output: impl Into<String>) -> Self {
        let path_str = path.into();
        let raw = output.into();
        // sing-box colours its own stderr, so the escapes land in the message.
        // They are noise in a terminal and corrupt JSON output for scripts, which
        // is where these errors are read most.
        let cleaned = crate::logs::strip_ansi(&raw);
        let trimmed = cleaned.trim();
        let msg = if trimmed.is_empty() {
            format!("sing-box rejected {path_str}")
        } else {
            format!("sing-box rejected {path_str}:\n{}", indent(trimmed, "  "))
        };
        let detail = trimmed.to_string();
        Self::validation(msg)
            .with_hint("fix the configuration, then run: singswitch check <name>")
            .with_kind(CoreErrorKind::InvalidConfig {
                path: path_str,
                detail,
            })
    }

    /// The profile is missing fields this outbound type needs.
    ///
    /// Names the fields rather than the old "replace TODO_ markers", which told
    /// the user nothing they could act on.
    pub fn profile_incomplete(name: &str, missing: &[crate::readiness::Field]) -> Self {
        if missing.is_empty() {
            // Not "needs  before it can connect": an empty list here means the
            // file never parsed, which is a different problem with a different fix.
            return Self::validation(format!("{name} is not a readable sing-box config"))
                .with_hint("validate it with: sing-box check -c <path>");
        }
        let labels = crate::readiness::join_labels(missing);
        Self::validation(format!("{name} needs {labels} before it can connect"))
    }

    pub fn profile_not_found(name: &str, available: &[String]) -> Self {
        let mut err = Self::general(format!("there is no profile named {name:?}"));
        if !available.is_empty() {
            err = err.with_hint(format!("available profiles: {}", join_and(available)));
        }
        err.with_hint(format!("create it with: singswitch add {name}"))
    }

    pub fn is_permission(&self) -> bool {
        self.0.code == ExitCode::Permission
            || matches!(self.0.kind, Some(CoreErrorKind::SudoNotConfigured { .. }))
    }
}

impl fmt::Display for SingSwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl std::error::Error for SingSwitchError {}

impl From<std::io::Error> for SingSwitchError {
    fn from(err: std::io::Error) -> Self {
        Self::general(err.to_string()).with_cause(err)
    }
}

pub type Result<T> = std::result::Result<T, SingSwitchError>;

/// Format an error with permission recovery guidance when manipulating the profile store.
pub fn store_error(
    err: std::io::Error,
    profiles_dir: &std::path::Path,
    action: &str,
) -> SingSwitchError {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        SingSwitchError::permission(format!(
            "not allowed to {action} profile data in {}",
            profiles_dir.display()
        ))
        .with_hint("repair directory ownership with the platform installer")
        .with_cause(err)
    } else {
        SingSwitchError::general(format!(
            "could not {action} profile data in {}: {err}",
            profiles_dir.display()
        ))
        .with_cause(err)
    }
}

/// Join items cleanly as `a, b and c`.
pub fn join_and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        _ => {
            let head = &items[..items.len() - 1];
            format!("{} and {}", head.join(", "), items[items.len() - 1])
        }
    }
}

pub(crate) fn first_line(s: &str) -> &str {
    s.split(['\r', '\n']).next().unwrap_or("")
}

pub(crate) fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
