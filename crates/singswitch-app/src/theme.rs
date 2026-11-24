use std::fs;
use std::path::PathBuf;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Window, WindowAppearance, rgb};
use serde::{Deserialize, Serialize};
use singswitch_core::error::{Result, SingSwitchError};

pub const MIN_WINDOW_WIDTH: f32 = 740.0;
pub const MIN_WINDOW_HEIGHT: f32 = 440.0;
pub const DEFAULT_WINDOW_WIDTH: f32 = 860.0;
// The guided editor's three section cards measure about 600 logical px before
// the save bar; at 480 the second card was cut mid-input with no scroll cue,
// because macOS reveals scrollbars only on hover. First-run and capture sizes
// now show the whole form.
pub const DEFAULT_WINDOW_HEIGHT: f32 = 620.0;
pub const WINDOW_SIZE_EPSILON: f32 = 2.0;

/// Shortest interval that still counts as "on", in minutes.
///
/// Every auto-sync re-downloads each remote profile, and a changed active
/// profile restarts the daemon, so a one-minute interval would keep dropping
/// the tunnel. Five minutes is the shortest that cannot do that constantly.
pub const MIN_AUTO_SYNC_INTERVAL_MINUTES: u32 = 5;
/// Longest interval, in minutes: once a day.
pub const MAX_AUTO_SYNC_INTERVAL_MINUTES: u32 = 24 * 60;

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
    /// Minutes between automatic remote-profile syncs. `0` means off.
    pub auto_sync_interval_minutes: u32,
    /// Unix seconds of the last automatic sync.
    ///
    /// Persisted so a relaunch cannot re-run a sync that just happened: the
    /// interval has to hold across restarts, not only across ticks.
    pub last_auto_sync_at: Option<u64>,
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
            auto_sync_interval_minutes: 0,
            last_auto_sync_at: None,
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
            Ok(raw) => {
                let mut settings: Self = serde_json::from_str(&raw).unwrap_or_default();
                settings.clamp_auto_sync_interval();
                settings
            }
            Err(_) => Self::default(),
        }
    }

    /// Hold the interval to the range the rest of the app assumes.
    ///
    /// The file is hand-editable, so a persisted `1` (constant syncing) or a
    /// huge value (never syncing again) has to be corrected on the way in
    /// rather than trusted.
    fn clamp_auto_sync_interval(&mut self) {
        let minutes = self.auto_sync_interval_minutes;
        if minutes != 0 {
            self.auto_sync_interval_minutes = minutes.clamp(
                MIN_AUTO_SYNC_INTERVAL_MINUTES,
                MAX_AUTO_SYNC_INTERVAL_MINUTES,
            );
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
        theme.focus_ring = false;
        theme.radius = gpui_kit::px(6.0);
        theme.radius_lg = gpui_kit::px(10.0);
        theme.font_size = gpui_kit::px(14.0);
        theme.font_family = crate::fonts::ui_family();
        theme.mono_font_family = crate::fonts::mono_family();
        if theme.mode.is_dark() {
            theme.background = rgb(0x0B0C10).into();
            theme.foreground = rgb(0xF8FAFC).into();
            theme.muted = rgb(0x171922).into();
            theme.muted_foreground = rgb(0x94A3B8).into();
            theme.border = rgb(0x272B38).into();
            theme.input = rgb(0x2E3344).into();
            theme.secondary = rgb(0x1A1C24).into();
            theme.secondary_foreground = rgb(0xF1F5F9).into();
            theme.secondary_hover = rgb(0x242834).into();
            theme.secondary_active = rgb(0x2E3240).into();
            theme.warning_foreground = rgb(0x0B0C10).into();
            theme.popover = rgb(0x13151B).into();
            theme.popover_foreground = rgb(0xF8FAFC).into();
            theme.title_bar = rgb(0x0E0F14).into();
            theme.title_bar_border = rgb(0x222530).into();
            theme.accent = rgb(0x1E222D).into();
            theme.accent_foreground = rgb(0xF8FAFC).into();
            theme.primary = rgb(0xF8FAFC).into();
            theme.primary_hover = rgb(0xE2E8F0).into();
            theme.primary_active = rgb(0xCBD5E1).into();
            theme.primary_foreground = rgb(0x0B0C10).into();
            theme.success = rgb(0x10B981).into();
            theme.warning = rgb(0xF59E0B).into();
            theme.danger = rgb(0xF43F5E).into();
            theme.button_danger = rgb(0x4C121E).into();
            theme.button_danger_hover = rgb(0x601828).into();
            theme.button_danger_active = rgb(0x731E31).into();
            theme.button_danger_foreground = rgb(0xFECDD3).into();
            theme.ring = rgb(0x3B82F6).into();
            theme.selection = rgb(0x1E293B).into();
            theme.caret = rgb(0x60A5FA).into();
            // Same as the light theme's override, for the same reason: the
            // default #404040 track is 1.76:1 on a surface card.
            theme.switch = rgb(0x6B7280).into();
        } else {
            theme.background = rgb(0xF8FAFC).into();
            theme.foreground = rgb(0x0F172A).into();
            theme.muted = rgb(0xF1F5F9).into();
            theme.muted_foreground = rgb(0x64748B).into();
            theme.border = rgb(0xE2E8F0).into();
            theme.input = rgb(0xCBD5E1).into();
            theme.secondary = rgb(0xF1F5F9).into();
            theme.secondary_foreground = rgb(0x0F172A).into();
            theme.secondary_hover = rgb(0xE2E8F0).into();
            theme.secondary_active = rgb(0xCBD5E1).into();
            theme.warning_foreground = rgb(0xFFFFFF).into();
            theme.popover = rgb(0xFFFFFF).into();
            theme.popover_foreground = rgb(0x0F172A).into();
            theme.title_bar = rgb(0xFFFFFF).into();
            theme.title_bar_border = rgb(0xE2E8F0).into();
            theme.accent = rgb(0xF1F5F9).into();
            theme.accent_foreground = rgb(0x0F172A).into();
            theme.primary = rgb(0x0F172A).into();
            theme.primary_hover = rgb(0x1E293B).into();
            theme.primary_active = rgb(0x334155).into();
            theme.primary_foreground = rgb(0xFFFFFF).into();
            theme.success = rgb(0x059669).into();
            theme.warning = rgb(0xB45309).into();
            theme.danger = rgb(0xBE123C).into();
            theme.button_danger = rgb(0xFFE4E6).into();
            theme.button_danger_hover = rgb(0xFECDD3).into();
            theme.button_danger_active = rgb(0xFDA4AF).into();
            theme.button_danger_foreground = rgb(0xBE123C).into();
            theme.ring = rgb(0x2563EB).into();
            theme.selection = rgb(0xDBEAFE).into();
            theme.caret = rgb(0x2563EB).into();
            // The switch's off track defaulted to #D4D4D4, 1.48:1 on a card:
            // the one part of a row a user must find to turn it on was its
            // faintest element. Non-text UI needs 3:1.
            theme.switch = rgb(0x7E8894).into();
        }
    });
}

/// Fill for cards and panels.
pub fn surface(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x13151B).into()
    } else {
        rgb(0xFFFFFF).into()
    }
}

/// Fill for chips and pills that sit on the secondary surface.
///
/// The strip background is `secondary` itself, so a pill must be a step
/// darker than it or it disappears: light `#F1F5F9` was 1.05:1 against
/// `#F8FAFC`. This keeps the chip visible in both themes (light 1.24:1,
/// dark 1.55:1 against the strip) while staying quiet.
pub fn subtle_pill_bg(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x1A1C24).into()
    } else {
        rgb(0xE8EDF3).into()
    }
}

/// The success colour at text grade.
///
/// `theme().success` is tuned for fills and indicators (light `#059669` is
/// 3.77:1 on white), but several places set small text in it: the profile
/// status word, the connected state words, the "Valid JSON" footer. Small
/// text needs 4.5:1, so light theme answers with a deeper green while dark
/// keeps the fill green, which already clears 7:1 on every surface.
/// Selection tint with an unmistakable hue shift against `subtle_pill_bg`
/// hover, so a selected log row cannot masquerade as a hovered one.
pub fn selection_tint(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        rgb(0x1E293B).into()
    } else {
        rgb(0xDBEAFE).into()
    }
}

pub fn success_text(cx: &App) -> gpui_kit::Hsla {
    if cx.theme().is_dark() {
        cx.theme().success
    } else {
        rgb(0x047857).into()
    }
}
