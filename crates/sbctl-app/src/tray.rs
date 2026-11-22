use gpui_kit::{App, Entity, Menu, MenuItem, Subscription};
use gpui_tray::{Icon, Tray};
use sbctl_core::profile::Profile;
use sbctl_core::service::RunState;

use crate::actions::{
    DisconnectService, EditProfile, Noop, OpenDoctorView, OpenMainWindow, QuitApp, SwitchProfile,
};
use crate::icons::{TrayVisualState, build_tray_icon};
use crate::state::AppStore;

struct TrayIcons {
    connected: Icon,
    disconnected: Icon,
    transitioning: Icon,
    broken: Icon,
}

impl TrayIcons {
    fn build_all() -> gpui_tray::Result<Self> {
        Ok(Self {
            connected: build_tray_icon(TrayVisualState::Connected)?,
            disconnected: build_tray_icon(TrayVisualState::Disconnected)?,
            transitioning: build_tray_icon(TrayVisualState::Transitioning)?,
            broken: build_tray_icon(TrayVisualState::Broken)?,
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
    health_state: RunState,
    busy: bool,
    broken: Option<String>,
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
            health_state: store.snapshot.health.state,
            busy: store.busy_phase.is_some(),
            broken: store.snapshot.broken.clone(),
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
        let (initial_visual, initial_title, initial_tooltip, initial_fp) = {
            let s = store.read(cx);
            (
                s.tray_visual_state(),
                menubar_title(s),
                s.tray_tooltip(),
                MenuFingerprint::from_store(s),
            )
        };

        let store_for_menu = store.clone();
        let mut builder = Tray::builder()
            .icon(icons.get(initial_visual))
            .tooltip(initial_tooltip.clone())
            .on_activate(OpenMainWindow)
            .menu(move |cx| build_tray_menu(store_for_menu.read(cx)));

        if let Some(title) = &initial_title {
            builder = builder.title(title.clone());
        }

        let tray = builder.build(cx)?;
        let tray_for_obs = tray.clone();

        let mut prev_visual = initial_visual;
        let mut prev_title = initial_title;
        let mut prev_tooltip = initial_tooltip;
        let mut prev_fp = initial_fp;

        let subscription = cx.observe(store, move |entity, cx| {
            let s = entity.read(cx);
            let visual = s.tray_visual_state();
            let title = menubar_title(s);
            let tooltip = s.tray_tooltip();
            let fp = MenuFingerprint::from_store(s);

            if visual != prev_visual {
                let _ = tray_for_obs.set_icon(icons.get(visual), cx);
                prev_visual = visual;
            }
            if title != prev_title {
                let _ = tray_for_obs.set_title(title.clone(), cx);
                prev_title = title;
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

/// The menu bar carries an icon only. A profile name is too long for that
/// space and competes with the status items the user actually watches; the
/// tooltip carries the detail instead.
fn menubar_title(_store: &AppStore) -> Option<String> {
    None
}

/// Profiles shown inline before collapsing the rest into a submenu.
const INLINE_PROFILE_LIMIT: usize = 8;

/// The tray menu, deliberately small.
///
/// Everything else that used to live here (restart, refresh IP, sync, doctor,
/// logs, settings) is reachable from the window, which is one click away. The
/// menu answers exactly two questions: is the tunnel up, and which profile is
/// carrying it.
fn build_tray_menu(store: &AppStore) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let is_busy = store.busy_phase.is_some();
    let failed = store.snapshot.broken.is_some() || store.snapshot.health.state == RunState::Error;

    // A failure is the only thing that outranks the service control.
    if failed {
        items.push(MenuItem::action("Open Doctor", OpenDoctorView));
        if store.snapshot.running {
            items.push(MenuItem::action("Disconnect", DisconnectService));
        }
        items.push(MenuItem::separator());
    }

    // The connection control, named for what it will act on.
    if !failed {
        if store.snapshot.running {
            items.push(MenuItem::action("Disconnect", DisconnectService).disabled(is_busy));
        } else {
            let ready_target = store
                .selected_profile
                .clone()
                .filter(|n| is_ready(store, n))
                .or_else(|| store.snapshot.active.clone().filter(|n| is_ready(store, n)))
                .or_else(|| {
                    store
                        .snapshot
                        .profiles
                        .iter()
                        .find(|p| p.ready())
                        .map(|p| p.name.clone())
                });
            match ready_target {
                Some(name) => {
                    items.push(
                        MenuItem::action(format!("Connect to {name}"), SwitchProfile { name })
                            .disabled(is_busy),
                    );
                }
                None => {
                    // Nothing can connect: offer the route to fixing it instead
                    // of a dead button.
                    if let Some(unready) = store
                        .selected_profile
                        .clone()
                        .or_else(|| store.snapshot.profiles.first().map(|p| p.name.clone()))
                    {
                        items.push(MenuItem::action(
                            format!("Set up {unready}…"),
                            EditProfile { name: unready },
                        ));
                    }
                }
            }
        }
        items.push(MenuItem::separator());
    }

    // Profiles, stable alphabetical order, bare names.
    let mut sorted: Vec<_> = store.snapshot.profiles.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let (inline, overflow) = if sorted.len() > INLINE_PROFILE_LIMIT {
        sorted.split_at(INLINE_PROFILE_LIMIT)
    } else {
        (&sorted[..], &[][..])
    };

    for profile in inline {
        items.push(profile_item(profile, store, is_busy));
    }
    if !overflow.is_empty() {
        let sub: Vec<MenuItem> = overflow
            .iter()
            .map(|p| profile_item(p, store, is_busy))
            .collect();
        items.push(MenuItem::submenu(Menu {
            name: "Profiles".into(),
            items: sub,
            disabled: false,
        }));
    }

    if !store.snapshot.profiles.is_empty() {
        items.push(MenuItem::separator());
    }
    items.push(MenuItem::action("Open sbctl", OpenMainWindow));
    items.push(MenuItem::action("Quit sbctl", QuitApp));

    items
}

fn is_ready(store: &AppStore, name: &str) -> bool {
    store
        .snapshot
        .profiles
        .iter()
        .any(|p| p.name == name && p.ready())
}

fn profile_item(profile: &Profile, store: &AppStore, is_busy: bool) -> MenuItem {
    let is_live = store.snapshot.running && store.snapshot.active.as_deref() == Some(&profile.name);
    if is_live {
        // Checked and locked: choosing the live profile again would only
        // restart it for no reason.
        return MenuItem::action(profile.name.clone(), Noop)
            .checked(true)
            .disabled(true);
    }
    let label = if profile.ready() {
        profile.name.clone()
    } else {
        format!("{} (needs setup)", profile.name)
    };
    MenuItem::action(
        label,
        SwitchProfile {
            name: profile.name.clone(),
        },
    )
    .disabled(is_busy || !profile.ready())
}
