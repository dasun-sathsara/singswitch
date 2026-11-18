use gpui_kit::{App, Entity, Menu, MenuItem, Subscription};
use gpui_tray::{Icon, Tray};
use sbctl_core::profile::Profile;
use sbctl_core::service::RunState;

use crate::actions::{
    ConnectActive, DisconnectService, Noop, OpenLogsView, OpenMainWindow, OpenSettingsView,
    QuitApp, RefreshIp, RunDoctor, SwitchProfile, SyncAll,
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
    busy_phase: Option<String>,
    broken: Option<String>,
    active: Option<String>,
    tun: Option<String>,
    pid: Option<u32>,
    ip_line: Option<String>,
    ip_loading: bool,
    profiles: Vec<(String, bool, Option<String>, Option<String>)>,
}

impl MenuFingerprint {
    fn from_store(store: &AppStore) -> Self {
        let ip_line = store.ip_info.as_ref().map(|ip| {
            let loc = ip.formatted_location();
            if loc.is_empty() {
                ip.ip.clone()
            } else {
                format!("{} ({loc})", ip.ip)
            }
        });
        let profiles = store
            .snapshot
            .profiles
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.ready(),
                    p.server.clone(),
                    p.source_url.clone(),
                )
            })
            .collect();
        Self {
            running: store.snapshot.running,
            health_state: store.snapshot.health.state,
            busy_phase: store.busy_phase.clone(),
            broken: store.snapshot.broken.clone(),
            active: store.snapshot.active.clone(),
            tun: store.snapshot.status_view.tun.clone(),
            pid: store.snapshot.status_view.pid,
            ip_line,
            ip_loading: store.ip_loading,
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

fn menubar_title(store: &AppStore) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if !store.settings.show_profile_in_menubar {
            return None;
        }
        if store.snapshot.running {
            store.snapshot.active.clone()
        } else {
            None
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = store;
        None
    }
}

fn format_status_header(store: &AppStore) -> String {
    if let Some(phase) = &store.busy_phase {
        return format!("Switching: {phase}");
    }
    if let Some(broken) = &store.snapshot.broken {
        return format!("Attention needed: {broken}");
    }
    if store.snapshot.running {
        match &store.snapshot.active {
            Some(name) => format!("Connected: {name}"),
            None => "Connected".to_string(),
        }
    } else if let Some(name) = &store.snapshot.active {
        format!("Disconnected, {name} selected")
    } else {
        "Disconnected".to_string()
    }
}

fn push_connection_telemetry(items: &mut Vec<MenuItem>, store: &AppStore) {
    if store.snapshot.running {
        let mut parts = Vec::new();
        if let Some(tun) = &store.snapshot.status_view.tun {
            parts.push(tun.clone());
        }
        if let Some(pid) = store.snapshot.status_view.pid {
            parts.push(format!("PID {pid}"));
        }
        if !parts.is_empty() {
            items.push(
                MenuItem::action(format!("Tunnel: {}", parts.join(" | ")), Noop).disabled(true),
            );
        }
    }

    if let Some(ip) = &store.ip_info {
        let loc = ip.formatted_location();
        let line = if loc.is_empty() {
            format!("IP: {}", ip.ip)
        } else {
            format!("IP: {}, {loc}", ip.ip)
        };
        items.push(MenuItem::action(line, Noop).disabled(true));
    }
}

fn build_tray_menu(store: &AppStore) -> Vec<MenuItem> {
    let mut items = Vec::new();
    items.push(MenuItem::action(format_status_header(store), Noop).disabled(true));
    push_connection_telemetry(&mut items, store);

    let needs_doctor =
        store.snapshot.broken.is_some() || store.snapshot.health.state == RunState::Error;
    if needs_doctor {
        items.push(MenuItem::action("Run Doctor...", RunDoctor));
    }

    items.push(MenuItem::separator());

    let is_busy = store.busy_phase.is_some();
    if store.snapshot.running {
        items.push(MenuItem::action("Disconnect", DisconnectService).disabled(is_busy));
    } else {
        let can_connect = store.snapshot.profiles.iter().any(Profile::ready);
        items.push(MenuItem::action("Connect", ConnectActive).disabled(is_busy || !can_connect));
    }

    if !store.snapshot.profiles.is_empty() {
        items.push(MenuItem::separator());
        for profile in &store.snapshot.profiles {
            let is_active = store.snapshot.active.as_deref() == Some(profile.name.as_str());
            let mut label = if profile.ready() {
                profile.name.clone()
            } else {
                format!("{}: needs setup", profile.name)
            };
            if is_active && !store.snapshot.running {
                label.push_str(", selected");
            }
            items.push(
                MenuItem::action(
                    label,
                    SwitchProfile {
                        name: profile.name.clone(),
                    },
                )
                .checked(is_active && store.snapshot.running)
                .disabled(is_busy || !profile.ready()),
            );
        }
    }

    let has_remote = store
        .snapshot
        .profiles
        .iter()
        .any(|p| p.source_url.is_some());
    let tools_items = vec![
        MenuItem::action("Sync Remote Profiles", SyncAll).disabled(is_busy || !has_remote),
        MenuItem::action("Refresh Public IP", RefreshIp).disabled(store.ip_loading),
        MenuItem::separator(),
        MenuItem::action("Run Doctor...", RunDoctor),
    ];

    items.push(MenuItem::separator());
    items.push(MenuItem::submenu(Menu {
        name: "Tools".into(),
        items: tools_items,
        disabled: false,
    }));

    items.push(MenuItem::separator());
    items.push(MenuItem::action("Open sbctl", OpenMainWindow));
    items.push(MenuItem::action("View Logs", OpenLogsView));
    items.push(MenuItem::action("Settings...", OpenSettingsView));
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Quit sbctl", QuitApp));

    items
}
