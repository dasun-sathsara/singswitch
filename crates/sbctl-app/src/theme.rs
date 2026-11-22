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
    pub show_profile_in_menubar: bool,
    pub auto_refresh_ip: bool,
    pub desktop_notifications: bool,
    pub onboarding_dismissed: bool,
    pub sidebar_collapsed: bool,
    pub window_width: f32,
    pub window_height: f32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Light,
            launch_at_login: false,
            show_profile_in_menubar: true,
            auto_refresh_ip: true,
            desktop_notifications: true,
            onboarding_dismissed: false,
            sidebar_collapsed: false,
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
        fs::write(&path, body)
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
        theme.font_size = gpui_kit::px(13.0);
        if theme.mode.is_dark() {
            theme.background = rgb(0x1C1C1E).into();
            theme.foreground = rgb(0xF2F2F7).into();
            theme.muted = rgb(0x2C2C2E).into();
            theme.muted_foreground = rgb(0x98989D).into();
            theme.border = rgb(0x38383A).into();
            theme.title_bar = rgb(0x232325).into();
            theme.title_bar_border = rgb(0x38383A).into();
            theme.sidebar = rgb(0x202022).into();
            theme.sidebar_border = rgb(0x38383A).into();
            theme.sidebar_accent = rgb(0x3A3A3C).into();
            theme.sidebar_accent_foreground = rgb(0xFFFFFF).into();
            theme.primary = rgb(0x0A84FF).into();
            theme.primary_hover = rgb(0x409CFF).into();
            theme.primary_active = rgb(0x0060DF).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x30D158).into();
            theme.warning = rgb(0xFF9F0A).into();
            theme.danger = rgb(0xFF453A).into();
        } else {
            theme.background = rgb(0xF2F2F7).into();
            theme.foreground = rgb(0x1D1D1F).into();
            theme.muted = rgb(0xE8E8ED).into();
            theme.muted_foreground = rgb(0x6E6E73).into();
            theme.border = rgb(0xDCDCE0).into();
            theme.title_bar = rgb(0xF7F7F9).into();
            theme.title_bar_border = rgb(0xDCDCE0).into();
            theme.sidebar = rgb(0xEDEDF0).into();
            theme.sidebar_border = rgb(0xDCDCE0).into();
            theme.sidebar_accent = rgb(0xDEDEE3).into();
            theme.sidebar_accent_foreground = rgb(0x1D1D1F).into();
            theme.primary = rgb(0x007AFF).into();
            theme.primary_hover = rgb(0x0068D9).into();
            theme.primary_active = rgb(0x0057B8).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x248A3D).into();
            theme.warning = rgb(0xB25000).into();
            theme.danger = rgb(0xD70015).into();
        }
    });
}

/// Background for raised content surfaces: white in light mode, a lifted grey in dark.
pub fn surface_card_bg(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x2A2A2C).into()
    } else {
        rgb(0xFFFFFF).into()
    }
}

/// Background for recessed pills, chips, and inline code.
pub fn subtle_pill_bg(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x3A3A3C).into()
    } else {
        rgb(0xEFEFF2).into()
    }
}
