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
        theme.font_size = gpui_kit::px(15.0);
        theme.font_family = crate::fonts::ui_family();
        theme.mono_font_family = crate::fonts::mono_family();
        if theme.mode.is_dark() {
            theme.background = rgb(0x161513).into();
            theme.foreground = rgb(0xEFEDE8).into();
            theme.muted = rgb(0x232220).into();
            theme.muted_foreground = rgb(0x9A968C).into();
            theme.border = rgb(0x2E2C29).into();
            theme.title_bar = rgb(0x161513).into();
            theme.title_bar_border = rgb(0x2E2C29).into();
            theme.sidebar = rgb(0x1D1C1A).into();
            theme.sidebar_border = rgb(0x2E2C29).into();
            theme.sidebar_accent = rgb(0x2C2B34).into();
            theme.sidebar_accent_foreground = rgb(0xEFEDE8).into();
            theme.primary = rgb(0x9C97F0).into();
            theme.primary_hover = rgb(0xADA9F5).into();
            theme.primary_active = rgb(0x8A85E6).into();
            theme.primary_foreground = rgb(0x161513).into();
            theme.success = rgb(0x46C97B).into();
            theme.warning = rgb(0xE5A93B).into();
            theme.danger = rgb(0xF07A6E).into();
            theme.ring = rgb(0x9C97F0).into();
            theme.selection = rgb(0x2C2B34).into();
            theme.caret = rgb(0x9C97F0).into();
        } else {
            theme.background = rgb(0xF7F6F3).into();
            theme.foreground = rgb(0x171613).into();
            theme.muted = rgb(0xEFEEE9).into();
            theme.muted_foreground = rgb(0x6E6A62).into();
            theme.border = rgb(0xE2E0D9).into();
            theme.title_bar = rgb(0xF7F6F3).into();
            theme.title_bar_border = rgb(0xE2E0D9).into();
            theme.sidebar = rgb(0xFFFFFF).into();
            theme.sidebar_border = rgb(0xE2E0D9).into();
            theme.sidebar_accent = rgb(0xF2F2F9).into();
            theme.sidebar_accent_foreground = rgb(0x171613).into();
            theme.primary = rgb(0x4A47A3).into();
            theme.primary_hover = rgb(0x3D3A8C).into();
            theme.primary_active = rgb(0x333077).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x1C7A45).into();
            theme.warning = rgb(0x8A5300).into();
            theme.danger = rgb(0xA3261D).into();
            theme.ring = rgb(0x4A47A3).into();
            theme.selection = rgb(0xF2F2F9).into();
            theme.caret = rgb(0x4A47A3).into();
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
