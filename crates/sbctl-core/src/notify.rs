use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout::PlatformKind;

/// Sends lightweight OS desktop notifications on profile switches, stops, and failures.
pub trait Notifier: Send + Sync {
    fn notify(&self, message: &str);

    fn notify_error(&self, title: &str, message: &str) {
        self.notify(&format!("{title}: {message}"));
    }
}

#[derive(Debug, Default)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _message: &str) {}
}

/// Wraps an inner `Notifier` with a live toggle so Settings can disable desktop notifications.
pub struct ToggleableNotifier {
    enabled: Arc<AtomicBool>,
    inner: Box<dyn Notifier>,
}

impl ToggleableNotifier {
    pub fn new(inner: Box<dyn Notifier>, enabled: Arc<AtomicBool>) -> Self {
        Self { enabled, inner }
    }
}

impl Notifier for ToggleableNotifier {
    fn notify(&self, message: &str) {
        if self.enabled.load(Ordering::Relaxed) {
            self.inner.notify(message);
        }
    }

    fn notify_error(&self, title: &str, message: &str) {
        if self.enabled.load(Ordering::Relaxed) {
            self.inner.notify_error(title, message);
        }
    }
}

#[derive(Debug, Default)]
pub struct AppleScriptNotifier;

impl AppleScriptNotifier {
    fn send_with_title(title: &str, message: &str) {
        let safe_title = apple_script_safe(title);
        let safe_msg = apple_script_safe(message);
        let script = format!("display notification \"{safe_msg}\" with title \"{safe_title}\"");
        std::thread::spawn(move || {
            let _ = Command::new("osascript").args(["-e", &script]).status();
        });
    }
}

impl Notifier for AppleScriptNotifier {
    fn notify(&self, message: &str) {
        Self::send_with_title("sing-box", message);
    }

    fn notify_error(&self, title: &str, message: &str) {
        Self::send_with_title(title, message);
    }
}

#[derive(Debug, Default)]
pub struct WindowsToastNotifier;

impl WindowsToastNotifier {
    fn send_toast(title: &str, message: &str) {
        let safe_title = sanitize_notification_text(title);
        let safe_body = sanitize_notification_text(message);
        std::thread::spawn(move || {
            let script = "\
                [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
                [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null; \
                $t = [Security.SecurityElement]::Escape($env:SBCTL_NOTIFY_TITLE); \
                $b = [Security.SecurityElement]::Escape($env:SBCTL_NOTIFY_BODY); \
                $xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
                $xml.LoadXml(\"<toast><visual><binding template='ToastGeneric'><text>$t</text><text>$b</text></binding></visual></toast>\"); \
                $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
                [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('sbctl').Show($toast)";
            let mut cmd = Command::new("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                script,
            ])
            .env("SBCTL_NOTIFY_TITLE", safe_title)
            .env("SBCTL_NOTIFY_BODY", safe_body);

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            let _ = cmd.status();
        });
    }
}

impl Notifier for WindowsToastNotifier {
    fn notify(&self, message: &str) {
        Self::send_toast("sbctl", message);
    }

    fn notify_error(&self, title: &str, message: &str) {
        Self::send_toast(title, message);
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

/// Collapse ASCII control characters and excess whitespace for toast payloads.
pub fn sanitize_notification_text(message: &str) -> String {
    let cleaned: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Construct the platform notifier for `platform`.
pub fn default_notifier(platform: PlatformKind) -> Box<dyn Notifier> {
    match platform {
        PlatformKind::Darwin => Box::new(AppleScriptNotifier),
        PlatformKind::Windows => Box::new(WindowsToastNotifier),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn apple_script_safe_neutralizes_quotes_and_newlines() {
        let evil = "work\" & do shell script \"id\n\t\\ok";
        let safe = apple_script_safe(evil);
        assert!(!safe.contains('"'));
        assert!(!safe.contains('\\'));
        assert!(!safe.contains('\n'));
        assert_eq!(safe, "work' & do shell script 'id ok");
    }

    #[test]
    fn sanitize_notification_text_strips_controls_and_preserves_unicode() {
        let clean = sanitize_notification_text("line1\r\n\tline2 <bad> & café");
        assert!(!clean.contains('\n'));
        assert!(!clean.contains('\t'));
        assert_eq!(clean, "line1 line2 <bad> & café");
    }

    #[derive(Default)]
    struct RecordingNotifier(Arc<Mutex<Vec<String>>>);

    impl Notifier for RecordingNotifier {
        fn notify(&self, message: &str) {
            self.0.lock().unwrap().push(message.to_string());
        }
    }

    #[test]
    fn toggleable_notifier_respects_enabled_flag() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::new(AtomicBool::new(true));
        let notifier =
            ToggleableNotifier::new(Box::new(RecordingNotifier(recorded.clone())), flag.clone());
        notifier.notify("first");
        flag.store(false, Ordering::Relaxed);
        notifier.notify("suppressed");
        notifier.notify_error("err", "suppressed");
        flag.store(true, Ordering::Relaxed);
        notifier.notify_error("Activation failed", "bad config");

        let msgs = recorded.lock().unwrap();
        assert_eq!(msgs.as_slice(), ["first", "Activation failed: bad config"]);
    }
}
