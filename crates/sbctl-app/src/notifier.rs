//! Native desktop notifications for the app.
//!
//! macOS goes through `UNUserNotificationCenter`, so a switch notice appears
//! with the app's own name and icon and respects Notification settings. The
//! CLI keeps its own notifier; this one never runs outside a signed bundle.

use sbctl_core::notify::Notifier;

/// Posts notifications through the system notification center.
#[derive(Debug, Default)]
pub struct NativeNotifier;

#[cfg(target_os = "macos")]
impl Notifier for NativeNotifier {
    fn notify(&self, message: &str) {
        post("sbctl", message);
    }

    fn notify_error(&self, title: &str, message: &str) {
        post(title, message);
    }
}

#[cfg(not(target_os = "macos"))]
impl Notifier for NativeNotifier {
    fn notify(&self, message: &str) {
        sbctl_core::notify::default_notifier(sbctl_core::layout::PlatformKind::Windows)
            .notify(message);
    }

    fn notify_error(&self, title: &str, message: &str) {
        sbctl_core::notify::default_notifier(sbctl_core::layout::PlatformKind::Windows)
            .notify_error(title, message);
    }
}

#[cfg(target_os = "macos")]
fn post(title: &str, body: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNUserNotificationCenter,
    };

    // `UNUserNotificationCenter::currentNotificationCenter` raises an
    // Objective-C exception when the process has no bundle identifier, which
    // kills a `cargo run` binary outright. Only bundle launches post.
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return;
    }

    static ASKED: AtomicBool = AtomicBool::new(false);

    let center = UNUserNotificationCenter::currentNotificationCenter();

    if !ASKED.swap(true, Ordering::Relaxed) {
        let handler = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &handler,
        );
    }

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let identifier = NSString::from_str(&format!(
        "sbctl-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let handler = RcBlock::new(|_error: *mut NSError| {});
    center.addNotificationRequest_withCompletionHandler(&request, Some(&handler));
}
