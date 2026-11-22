use std::fs;
use std::path::PathBuf;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Window, WindowAppearance, rgb};
use sbctl_core::error::{Result, SbctlError};
use serde::{Deserialize, Serialize};

pub const MIN_WINDOW_WIDTH: f32 = 820.0;
pub const MIN_WINDOW_HEIGHT: f32 = 540.0;
pub const DEFAULT_WINDOW_WIDTH: f32 = 1020.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 680.0;
pub const WINDOW_SIZE_EPSILON: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    Light,
    Dark,
    System,
}

impl ThemePreference {
    pub fn resolve_mode(self, appearance: WindowAppearance) -> ThemeMode {
        match self {
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
            Self::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
                _ => ThemeMode::Light,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: ThemePreference,
    pub launch_at_login: bool,
    pub auto_refresh_ip: bool,
    pub desktop_notifications: bool,
    pub window_width: f32,
    pub window_height: f32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Light,
            launch_at_login: false,
            auto_refresh_ip: true,
            desktop_notifications: true,
            window_width: DEFAULT_WINDOW_WIDTH,
            window_height: DEFAULT_WINDOW_HEIGHT,
        }
    }
}

impl UiSettings {
    pub fn settings_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("sbctl").join("ui-settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::settings_path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::settings_path() else {
            return Err(SbctlError::general("could not resolve config directory"));
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SbctlError::general(format!("failed to create {}: {e}", parent.display()))
            })?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| SbctlError::general(format!("failed to serialize UI settings: {e}")))?;
        // Atomic: a crash or kill mid-write would otherwise leave truncated
        // JSON, and `load` silently resets every setting on a parse failure.
        sbctl_core::activator::write_file_atomic(&path, body.as_bytes(), 0o644)
            .map_err(|e| SbctlError::general(format!("failed to write {}: {e}", path.display())))
    }
}

/// Apply the selected light, dark, or system theme.
///
/// Colours follow macOS system appearance rather than a web palette: grouped
/// window background, white cards, hairline separators, secondary label greys,
/// and the system blue accent. Windows uses the same neutral scale, which also
/// matches its Fluent surfaces closely enough to look native there.
pub fn apply_theme(pref: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    let appearance = window
        .as_ref()
        .map(|w| w.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let mode = pref.resolve_mode(appearance);

    Theme::change(mode, window, cx);
    Theme::update(cx, |theme| {
        theme.radius = gpui_kit::px(7.0);
        theme.radius_lg = gpui_kit::px(10.0);
        theme.font_size = gpui_kit::px(15.0);
        theme.font_family = crate::fonts::ui_family();
        theme.mono_font_family = crate::fonts::mono_family();
        if theme.mode.is_dark() {
            // Cool neutral grey, the vocabulary macOS utilities use. No warm
            // cream: a tool should look like a system surface, not paper.
            theme.background = rgb(0x1A1A1C).into();
            theme.foreground = rgb(0xEDEDEF).into();
            theme.muted = rgb(0x2A2A2E).into();
            theme.muted_foreground = rgb(0xB0B0B6).into();
            // Hairlines recede (1.32:1); `input` is the *border* colour gpui-component
            // paints on every field and outline button, so it must stay visible
            // (3:1) rather than recede like a divider.
            theme.border = rgb(0x303034).into();
            theme.input = rgb(0x73737B).into();
            theme.secondary = rgb(0x44444B).into();
            theme.secondary_active = rgb(0x4A4A52).into();
            theme.warning_foreground = rgb(0x0D1117).into();
            theme.popover = rgb(0x242427).into();
            theme.popover_foreground = rgb(0xEDEDEF).into();
            theme.title_bar = rgb(0x1F1F22).into();
            theme.title_bar_border = rgb(0x3A3A3E).into();
            theme.sidebar = rgb(0x1F1F22).into();
            theme.sidebar_border = rgb(0x3A3A3E).into();
            theme.sidebar_accent = rgb(0x323238).into();
            theme.sidebar_accent_foreground = rgb(0xEDEDEF).into();
            // `accent` is the hover/selected fill for menu rows and ghost
            // buttons. Left unset it falls back to the stock neutral-800
            // (#262626), which against our popover (#242427) is 1.02:1 -
            // effectively invisible.
            theme.accent = rgb(0x3A3A42).into();
            theme.primary = rgb(0x5AA2FF).into();
            theme.primary_hover = rgb(0x74B1FF).into();
            theme.primary_active = rgb(0x488FE6).into();
            theme.primary_foreground = rgb(0x0D1117).into();
            theme.success = rgb(0x5BC97E).into();
            theme.warning = rgb(0xE0A33E).into();
            theme.danger = rgb(0xFF7B72).into();
            theme.ring = rgb(0x5AA2FF).into();
            theme.selection = rgb(0x323238).into();
            theme.caret = rgb(0x5AA2FF).into();
        } else {
            theme.background = rgb(0xEFEFF1).into();
            theme.foreground = rgb(0x1B1B1E).into();
            theme.muted = rgb(0xE8E8EB).into();
            theme.muted_foreground = rgb(0x5E5E64).into();
            theme.border = rgb(0xD2D2D6).into();
            theme.input = rgb(0x88888F).into();
            theme.secondary = rgb(0xB8B8BE).into();
            theme.secondary_active = rgb(0xA6A6AD).into();
            theme.warning_foreground = rgb(0xFFFFFF).into();
            theme.popover = rgb(0xFFFFFF).into();
            theme.popover_foreground = rgb(0x1B1B1E).into();
            theme.title_bar = rgb(0xF0F0F2).into();
            theme.title_bar_border = rgb(0xE1E1E4).into();
            theme.sidebar = rgb(0xF5F5F7).into();
            theme.sidebar_border = rgb(0xE1E1E4).into();
            theme.sidebar_accent = rgb(0xE4E4E8).into();
            theme.sidebar_accent_foreground = rgb(0x1B1B1E).into();
            theme.accent = rgb(0xD6D6DB).into();
            theme.primary = rgb(0x0B62C4).into();
            theme.primary_hover = rgb(0x0A56AC).into();
            theme.primary_active = rgb(0x084A94).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x1F7A3D).into();
            theme.warning = rgb(0x8A4B00).into();
            theme.danger = rgb(0xB3261E).into();
            theme.ring = rgb(0x0B62C4).into();
            theme.selection = rgb(0xE4E4E8).into();
            theme.caret = rgb(0x0B62C4).into();
        }
    });
}

/// Background for content regions.
///
/// Deliberately identical to the window ground: the redesign removed raised
/// white cards, so content is separated by hairlines and space instead of by a
/// second surface colour. Kept as a named helper so the intent is explicit at
/// each call site rather than implied by `background`.
pub fn surface_card_bg(cx: &App) -> gpui_kit::Hsla {
    cx.theme().background
}

/// Background for recessed rows, hover states, and inline code.
pub fn subtle_pill_bg(cx: &App) -> gpui_kit::Hsla {
    cx.theme().muted
}

/// The edge of a control that must read as a control: a button outline or a
/// field border. Structural hairlines use `theme.border`, which is deliberately
/// faint enough to recede; this is the one place a visible line earns its keep.
/// Fill for inputs and cards. `theme.input` is a *border* colour in
/// gpui-component 0.10, so using it as a background painted a dark slab.
pub fn surface(cx: &gpui_kit::App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        gpui_kit::rgb(0x242427).into()
    } else {
        gpui_kit::rgb(0xFFFFFF).into()
    }
}

pub fn control_edge(cx: &gpui_kit::App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        gpui_kit::rgb(0x45454A).into()
    } else {
        gpui_kit::rgb(0xD2D2D8).into()
    }
}

/// A neutral hover wash for rows and controls, one step above the ground.
pub fn hover_wash(cx: &gpui_kit::App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        gpui_kit::rgb(0x2E2E33).into()
    } else {
        gpui_kit::rgb(0xDEDEE2).into()
    }
}
