use std::fs;
use std::path::PathBuf;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Window, WindowAppearance, rgb};
use serde::{Deserialize, Serialize};
use singswitch_core::error::{Result, SingSwitchError};

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
        dirs::config_dir().map(|d| d.join("singswitch").join("ui-settings.json"))
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
            return Err(SingSwitchError::general(
                "could not resolve config directory",
            ));
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SingSwitchError::general(format!("failed to create {}: {e}", parent.display()))
            })?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| {
            SingSwitchError::general(format!("failed to serialize UI settings: {e}"))
        })?;
        // Atomic: a crash or kill mid-write would otherwise leave truncated
        // JSON, and `load` silently resets every setting on a parse failure.
        singswitch_core::activator::write_file_atomic(&path, body.as_bytes(), 0o644).map_err(|e| {
            SingSwitchError::general(format!("failed to write {}: {e}", path.display()))
        })
    }
}

/// Apply the desktop palette to the window and GPUI components.
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
            theme.background = rgb(0x151722).into();
            theme.foreground = rgb(0xF1EFFA).into();
            theme.muted = rgb(0x2A2C3C).into();
            theme.muted_foreground = rgb(0xB5B5CC).into();
            theme.border = rgb(0x35374B).into();
            theme.input = rgb(0x72748B).into();
            theme.secondary = rgb(0x36304F).into();
            theme.secondary_foreground = rgb(0xF1EAFF).into();
            theme.secondary_hover = rgb(0x443C65).into();
            theme.secondary_active = rgb(0x504570).into();
            theme.warning_foreground = rgb(0x1D1428).into();
            theme.popover = rgb(0x222432).into();
            theme.popover_foreground = rgb(0xF1EFFA).into();
            theme.title_bar = rgb(0x1C1D2A).into();
            theme.title_bar_border = rgb(0x35374B).into();
            theme.accent = rgb(0x393451).into();
            theme.accent_foreground = rgb(0xF1EAFF).into();
            theme.primary = rgb(0xBCA9FF).into();
            theme.primary_hover = rgb(0xCEBFFF).into();
            theme.primary_active = rgb(0xAB94FC).into();
            theme.primary_foreground = rgb(0x201634).into();
            theme.success = rgb(0x7BDCB2).into();
            theme.warning = rgb(0xEFC57D).into();
            theme.danger = rgb(0xFF959E).into();
            theme.button_danger = rgb(0x60293B).into();
            theme.button_danger_hover = rgb(0x713248).into();
            theme.button_danger_active = rgb(0x803950).into();
            theme.button_danger_foreground = rgb(0xFFB7C0).into();
            theme.ring = rgb(0xBCA9FF).into();
            theme.selection = rgb(0x483963).into();
            theme.caret = rgb(0xBCA9FF).into();
        } else {
            theme.background = rgb(0xF2F2F8).into();
            theme.foreground = rgb(0x222039).into();
            theme.muted = rgb(0xEBEAF3).into();
            theme.muted_foreground = rgb(0x5F5D76).into();
            theme.border = rgb(0xDFDDEA).into();
            theme.input = rgb(0x8884A1).into();
            theme.secondary = rgb(0xE8E3F7).into();
            theme.secondary_foreground = rgb(0x432E74).into();
            theme.secondary_hover = rgb(0xE0D8F4).into();
            theme.secondary_active = rgb(0xDAD0F1).into();
            theme.warning_foreground = rgb(0xFFFFFF).into();
            theme.popover = rgb(0xFFFFFF).into();
            theme.popover_foreground = rgb(0x222039).into();
            theme.title_bar = rgb(0xFFFFFF).into();
            theme.title_bar_border = rgb(0xDFDDEA).into();
            theme.accent = rgb(0xE9E4F6).into();
            theme.accent_foreground = rgb(0x372766).into();
            theme.primary = rgb(0x6344B5).into();
            theme.primary_hover = rgb(0x5536A3).into();
            theme.primary_active = rgb(0x4A308F).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x126747).into();
            theme.warning = rgb(0x875309).into();
            theme.danger = rgb(0xA32C46).into();
            theme.button_danger = rgb(0xFAE5E9).into();
            theme.button_danger_hover = rgb(0xF5D6DD).into();
            theme.button_danger_active = rgb(0xEFC6D0).into();
            theme.button_danger_foreground = rgb(0x8D2440).into();
            theme.ring = rgb(0x6344B5).into();
            theme.selection = rgb(0xE0D7F8).into();
            theme.caret = rgb(0x6344B5).into();
        }
    });
}

/// Fill for cards and panels.
pub fn surface(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x212332).into()
    } else {
        rgb(0xFFFFFF).into()
    }
}

pub fn connection_surface(cx: &App, tone: gpui_kit::Hsla) -> gpui_kit::Hsla {
    surface(cx).blend(tone.opacity(0.09))
}

pub fn subtle_pill_bg(cx: &App) -> gpui_kit::Hsla {
    cx.theme().muted
}
