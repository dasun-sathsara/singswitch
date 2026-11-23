//! Embedded typefaces.
//!
//! Two Geist faces, doing different jobs. Geist Sans is proportional and carries
//! the interface; Geist Mono is fixed-width and carries values, identifiers,
//! logs and paths, where digits and addresses need to line up in columns under
//! other digits and addresses.
//!
//! Three weights each: Regular for body text, Medium for the emphasis that used
//! to be Bold, and Bold for the few places that genuinely need to shout. No
//! weight is requested that is not bundled, so the text system never has to
//! synthesise one.
//!
//! gpui resolves a font by a single family name and then walks its own fallback
//! stack (`.ZedSans`, Helvetica, Segoe UI, ...) for anything it cannot find, so
//! the families are named plainly here rather than as a CSS-style chain. If the
//! bundled files ever fail to register, text still renders in the platform face.

use gpui_kit::App;
use gpui_kit::{FontWeight, SharedString};

/// Weight for emphasis: titles, headings, profile names, log levels.
///
/// Medium rather than Bold. Geist at 700 is heavier than Feixen at 510 and read
/// as shouting at 13-15px, which is the size most of this interface uses.
pub const EMPHASIS: FontWeight = FontWeight::MEDIUM;

/// Proportional face for the interface.
pub const UI_FAMILY: &str = "Geist";
/// Fixed-width face for values, identifiers, logs and paths.
pub const MONO_FAMILY: &str = "Geist Mono";

const UI_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
const UI_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Geist-Medium.ttf");
const UI_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-Bold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf");
const MONO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Medium.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Bold.ttf");

/// Register the bundled faces with the text system.
///
/// Must run before any window opens. A failure is logged, not fatal: the theme
/// still names the families, and gpui falls back to the platform face.
pub fn install(cx: &mut App) {
    let faces: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(UI_REGULAR),
        std::borrow::Cow::Borrowed(UI_MEDIUM),
        std::borrow::Cow::Borrowed(UI_BOLD),
        std::borrow::Cow::Borrowed(MONO_REGULAR),
        std::borrow::Cow::Borrowed(MONO_MEDIUM),
        std::borrow::Cow::Borrowed(MONO_BOLD),
    ];

    if let Err(err) = cx.text_system().add_fonts(faces) {
        crate::diag::log(&format!("font registration failed: {err}"));
    }
}

pub fn ui_family() -> SharedString {
    SharedString::from(UI_FAMILY)
}

pub fn mono_family() -> SharedString {
    SharedString::from(MONO_FAMILY)
}
