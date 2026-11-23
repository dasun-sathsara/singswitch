//! Embedded typefaces.
//!
//! Feixen Standard for the interface and Geist Standard for values, identifiers,
//! logs, and paths. Both ship only Regular and Bold, so the design uses two
//! weights and never asks the text system for 500 or 600, which would be
//! synthesised and look wrong at 12–15px.
//!
//! gpui resolves a font by a single family name and then walks its own fallback
//! stack (`.ZedSans`, Helvetica, Segoe UI, …) for anything it cannot find, so the
//! families are named plainly here rather than as a CSS-style chain. If the
//! bundled files ever fail to register, text still renders in the platform face.

use gpui_kit::App;
use gpui_kit::SharedString;

/// Interface face.
pub const UI_FAMILY: &str = "Feixen Standard";
/// Face for values, identifiers, logs, and paths.
pub const MONO_FAMILY: &str = "Geist Standard";

const UI_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/FeixenStandard-Regular.ttf");
const UI_BOLD: &[u8] = include_bytes!("../../../assets/fonts/FeixenStandard-Bold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/GeistStandard-Regular.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../../../assets/fonts/GeistStandard-Bold.ttf");

/// Register the bundled faces with the text system.
///
/// Must run before any window opens. A failure is logged, not fatal: the theme
/// still names the families, and gpui falls back to the platform face.
pub fn install(cx: &mut App) {
    let faces: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(UI_REGULAR),
        std::borrow::Cow::Borrowed(UI_BOLD),
        std::borrow::Cow::Borrowed(MONO_REGULAR),
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
