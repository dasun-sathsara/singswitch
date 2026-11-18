use std::process::Command;

use crate::layout::PlatformKind;

/// Sends lightweight OS desktop notifications on profile switch or tunnel stop.
pub trait Notifier: Send + Sync {
    fn notify(&self, message: &str);
}

#[derive(Debug, Default)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _message: &str) {}
}

#[derive(Debug, Default)]
pub struct AppleScriptNotifier;

impl Notifier for AppleScriptNotifier {
    fn notify(&self, message: &str) {
        let safe = apple_script_safe(message);
        let script = format!("display notification \"{safe}\" with title \"sing-box\"");
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
}

/// Strip quotes, backslashes, and control characters so text cannot escape an
/// AppleScript string literal.
pub fn apple_script_safe(message: &str) -> String {
    let cleaned: String = message
        .chars()
        .map(|c| {
            if !c.is_ascii() || c.is_ascii_control() || c == '\\' {
                ' '
            } else if c == '"' {
                '\''
            } else {
                c
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Construct the platform notifier for `platform`.
pub fn default_notifier(platform: PlatformKind) -> Box<dyn Notifier> {
    match platform {
        PlatformKind::Darwin => Box::new(AppleScriptNotifier),
        PlatformKind::Windows => Box::new(NoopNotifier),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_script_safe_neutralizes_quotes_and_newlines() {
        let evil = "work\" & do shell script \"id\n\t\\ok";
        let safe = apple_script_safe(evil);
        assert!(!safe.contains('"'));
        assert!(!safe.contains('\\'));
        assert!(!safe.contains('\n'));
        assert_eq!(safe, "work' & do shell script 'id ok");
    }
}
