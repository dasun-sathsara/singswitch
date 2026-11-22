use gpui_kit::{App, Entity, Menu, MenuItem, Subscription};
use gpui_tray::{Icon, Tray};

use crate::actions::{
    DisconnectService, EditProfile, Noop, OpenLogsView, OpenMainWindow, QuitApp, SwitchProfile,
};
use crate::icons::{TrayVisualState, build_tray_icon};
use crate::state::AppStore;

struct TrayIcons {
    connected: Icon,
    disconnected: Icon,
    transitioning: Icon,
    broken: Icon,
}

/// Build one state's icon, falling back to `fallback` when it cannot be built.
///
/// Failing the whole tray for one unavailable SF Symbol downgraded the app to a
/// plain window on an older OS.
fn fallback_icon(state: TrayVisualState, fallback: &Icon) -> Icon {
    build_tray_icon(state).unwrap_or_else(|err| {
        crate::diag::log(&format!("tray icon unavailable for {state:?}: {err}"));
        fallback.clone()
    })
}

impl TrayIcons {
    fn build_all() -> gpui_tray::Result<Self> {
        // A state whose icon cannot be built falls back to the disconnected
        // icon rather than failing the whole tray: an unavailable SF Symbol on
        // an older OS would otherwise downgrade the app to a plain window.
        let disconnected = build_tray_icon(TrayVisualState::Disconnected)?;
        let connected = fallback_icon(TrayVisualState::Connected, &disconnected);
        let transitioning = fallback_icon(TrayVisualState::Transitioning, &disconnected);
        let broken = fallback_icon(TrayVisualState::Broken, &disconnected);
        Ok(Self {
            connected,
            disconnected,
            transitioning,
            broken,
        })
    }

    fn get(&self, state: TrayVisualState) -> Icon {
        match state {
            TrayVisualState::Connected => self.connected.clone(),
            TrayVisualState::Disconnected => self.disconnected.clone(),
            TrayVisualState::Transitioning => self.transitioning.clone(),
            TrayVisualState::Broken => self.broken.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MenuFingerprint {
    running: bool,
    visual: TrayVisualState,
    active: Option<String>,
    selected: Option<String>,
    profiles: Vec<(String, bool)>,
}

impl MenuFingerprint {
    /// Only what changes the rendered menu. Telemetry that used to be listed
    /// here (tunnel, PID, IP, loading flags) no longer appears, so churn in it
    /// must not trigger a rebuild.
    fn from_store(store: &AppStore) -> Self {
        let profiles = store
            .snapshot
            .profiles
            .iter()
            .map(|p| (p.name.clone(), p.ready()))
            .collect();
        Self {
            running: store.snapshot.running,
            visual: store.tray_visual_state(),
            active: store.snapshot.active.clone(),
            selected: store.selected_profile.clone(),
            profiles,
        }
    }
}

pub struct TrayController {
    tray: Tray,
    _subscription: Subscription,
}

impl TrayController {
    pub fn install(store: &Entity<AppStore>, cx: &mut App) -> gpui_tray::Result<Self> {
        let icons = TrayIcons::build_all()?;
        let (initial_visual, initial_tooltip, initial_fp) = {
            let s = store.read(cx);
            (
                s.tray_visual_state(),
                s.tray_tooltip(),
                MenuFingerprint::from_store(s),
            )
        };

        let store_for_menu = store.clone();
        let tray = Tray::builder()
            .icon(icons.get(initial_visual))
            .tooltip(initial_tooltip.clone())
            .on_activate(OpenMainWindow)
            .menu(move |cx| build_tray_menu(store_for_menu.read(cx)))
            .build(cx)?;
        let tray_for_obs = tray.clone();

        let mut prev_visual = initial_visual;
        let mut prev_tooltip = initial_tooltip;
        let mut prev_fp = initial_fp;

        let subscription = cx.observe(store, move |entity, cx| {
            let s = entity.read(cx);
            let visual = s.tray_visual_state();
            let tooltip = s.tray_tooltip();
            let fp = MenuFingerprint::from_store(s);

            if visual != prev_visual {
                let _ = tray_for_obs.set_icon(icons.get(visual), cx);
                prev_visual = visual;
            }
            if tooltip != prev_tooltip {
                let _ = tray_for_obs.set_tooltip(Some(tooltip.clone()), cx);
                prev_tooltip = tooltip;
            }
            if fp != prev_fp {
                let _ = tray_for_obs.refresh_menu(cx);
                prev_fp = fp;
            }
        });

        Ok(Self {
            tray,
            _subscription: subscription,
        })
    }

    pub fn close(&self, cx: &mut App) {
        let _ = self.tray.close(cx);
    }
}

/// Keep everyday switches one click away, and group larger libraries.
const INLINE_PROFILE_LIMIT: usize = 4;

fn build_tray_menu(store: &AppStore) -> Vec<MenuItem> {
    let snap = &store.snapshot;
    let visual = store.tray_visual_state();
    let busy = visual == TrayVisualState::Transitioning;
    let live = snap.active.as_deref().filter(|_| snap.running);
    let status = match visual {
        TrayVisualState::Connected => {
            live.map_or_else(|| "Connected".into(), |name| format!("Connected to {name}"))
        }
        TrayVisualState::Disconnected => "Disconnected".into(),
        TrayVisualState::Transitioning => "Updating connection…".into(),
        TrayVisualState::Broken => "Connection issue".into(),
    };
    let mut items = vec![MenuItem::action(status, Noop).disabled(true)];
    let target = (!snap.running)
        .then(|| store.ready_connection_target())
        .flatten();
    let unavailable = busy || store.init_error.is_some();

    if snap.running {
        items.push(MenuItem::action("Disconnect", DisconnectService).disabled(unavailable));
    } else if let Some(name) = target {
        items.push(connect_item(
            format!("Connect to {name}"),
            name,
            unavailable,
        ));
    } else if let Some(profile) = snap
        .profiles
        .iter()
        .find(|p| store.selected_profile.as_deref() == Some(p.name.as_str()))
        .or_else(|| snap.profiles.first())
    {
        items.push(
            MenuItem::action(
                format!("Set up {}…", profile.name),
                EditProfile {
                    name: profile.name.clone(),
                },
            )
            .disabled(unavailable),
        );
    }

    if visual == TrayVisualState::Broken {
        items.push(MenuItem::action("View logs", OpenLogsView));
    }

    let mut alternatives: Vec<_> = snap
        .profiles
        .iter()
        .filter(|profile| {
            let name = Some(profile.name.as_str());
            profile.ready() && name != live && name != target
        })
        .collect();
    alternatives.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    if !alternatives.is_empty() {
        items.push(MenuItem::separator());
        let verb = if snap.running {
            "Switch to"
        } else {
            "Connect to"
        };
        if alternatives.len() <= INLINE_PROFILE_LIMIT {
            items.extend(alternatives.into_iter().map(|profile| {
                connect_item(
                    format!("{verb} {}", profile.name),
                    &profile.name,
                    unavailable,
                )
            }));
        } else {
            items.push(MenuItem::submenu(Menu {
                name: if snap.running {
                    "Switch profile".into()
                } else {
                    "Connect to another profile".into()
                },
                items: alternatives
                    .into_iter()
                    .map(|profile| connect_item(profile.name.clone(), &profile.name, unavailable))
                    .collect(),
                disabled: unavailable,
            }));
        }
    }

    items.push(MenuItem::separator());
    items.push(MenuItem::action("Open sbctl", OpenMainWindow));
    items.push(MenuItem::action("Quit sbctl", QuitApp));
    items
}

fn connect_item(label: String, name: &str, disabled: bool) -> MenuItem {
    MenuItem::action(
        label,
        SwitchProfile {
            name: name.to_owned(),
        },
    )
    .disabled(disabled)
}
