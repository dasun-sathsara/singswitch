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

/// Apply the selected theme preference with restrained porcelain-light or slate-dark accents.
pub fn apply_theme(pref: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    let appearance = window
        .as_ref()
        .map(|w| w.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let mode = pref.resolve_mode(appearance);

    Theme::change(mode, window, cx);
    Theme::update(cx, |theme| {
        theme.radius = gpui_kit::px(8.0);
        theme.radius_lg = gpui_kit::px(12.0);
        theme.font_size = gpui_kit::px(14.0);
        if theme.mode.is_dark() {
            theme.background = rgb(0x0B1120).into();
            theme.foreground = rgb(0xF1F5F9).into();
            theme.muted = rgb(0x162033).into();
            theme.muted_foreground = rgb(0x94A3B8).into();
            theme.border = rgb(0x1E293B).into();
            theme.title_bar = rgb(0x0F172A).into();
            theme.title_bar_border = rgb(0x1E293B).into();
            theme.sidebar = rgb(0x0F172A).into();
            theme.sidebar_border = rgb(0x1E293B).into();
            theme.sidebar_accent = rgb(0x1E293B).into();
            theme.sidebar_accent_foreground = rgb(0xF8FAFC).into();
            theme.primary = rgb(0x10B981).into();
            theme.primary_hover = rgb(0x059669).into();
            theme.primary_active = rgb(0x047857).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x10B981).into();
            theme.warning = rgb(0xF59E0B).into();
            theme.danger = rgb(0xEF4444).into();
        } else {
            theme.background = rgb(0xF8FAFC).into();
            theme.foreground = rgb(0x0F172A).into();
            theme.muted = rgb(0xF1F5F9).into();
            theme.muted_foreground = rgb(0x64748B).into();
            theme.border = rgb(0xE2E8F0).into();
            theme.title_bar = rgb(0xFFFFFF).into();
            theme.title_bar_border = rgb(0xE2E8F0).into();
            theme.sidebar = rgb(0xF1F5F9).into();
            theme.sidebar_border = rgb(0xE2E8F0).into();
            theme.sidebar_accent = rgb(0xE2E8F0).into();
            theme.sidebar_accent_foreground = rgb(0x0F172A).into();
            theme.primary = rgb(0x0F766E).into();
            theme.primary_hover = rgb(0x0D9488).into();
            theme.primary_active = rgb(0x115E59).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x059669).into();
            theme.warning = rgb(0xD97706).into();
            theme.danger = rgb(0xDC2626).into();
        }
    });
}

pub fn surface_card_bg(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x111827).into()
    } else {
        rgb(0xFFFFFF).into()
    }
}

pub fn subtle_pill_bg(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x1E293B).into()
    } else {
        rgb(0xF1F5F9).into()
    }
}
