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

/// Build one state without taking down the tray if its image cannot be loaded.
fn fallback_icon(state: TrayVisualState, fallback: &Icon, cx: &App) -> Icon {
    build_tray_icon(state, cx).unwrap_or_else(|err| {
        crate::diag::log(&format!("tray icon unavailable for {state:?}: {err}"));
        fallback.clone()
    })
}

impl TrayIcons {
    fn build_all(cx: &App) -> gpui_tray::Result<Self> {
        let disconnected = build_tray_icon(TrayVisualState::Disconnected, cx)?;
        let connected = fallback_icon(TrayVisualState::Connected, &disconnected, cx);
        let transitioning = fallback_icon(TrayVisualState::Transitioning, &disconnected, cx);
        let broken = fallback_icon(TrayVisualState::Broken, &disconnected, cx);
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
    /// The first answer can equal the placeholder, so nothing else in this
    /// fingerprint changes when "Checking…" must stop being shown.
    probing: bool,
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
            probing: store.is_probing(),
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
        let icons = TrayIcons::build_all(cx)?;
        let (initial_visual, initial_tooltip, initial_fp) = {
            let s = store.read(cx);
            (
                display_visual(s),
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

        #[cfg(target_os = "macos")]
        let mut spin = GlobeSpin::default();
        #[cfg(target_os = "macos")]
        spin.update(
            initial_visual,
            store.read(cx).is_probing(),
            &tray,
            &icons.transitioning,
            cx,
        );

        let tray_for_obs = tray.clone();
        let mut prev_visual = initial_visual;
        let mut prev_tooltip = initial_tooltip;
        let mut prev_fp = initial_fp;

        let subscription = cx.observe(store, move |entity, cx| {
            let s = entity.read(cx);
            let visual = display_visual(s);
            #[cfg(target_os = "macos")]
            let probing = s.is_probing();
            let tooltip = s.tray_tooltip();
            let fp = MenuFingerprint::from_store(s);

            #[cfg(target_os = "macos")]
            spin.update(visual, probing, &tray_for_obs, &icons.transitioning, cx);

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

fn display_visual(store: &AppStore) -> TrayVisualState {
    if store.is_probing() {
        TrayVisualState::Transitioning
    } else {
        store.tray_visual_state()
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct GlobeSpin {
    frames: Option<std::rc::Rc<[Icon]>>,
    task: Option<gpui_kit::Task<()>>,
}

#[cfg(target_os = "macos")]
impl GlobeSpin {
    fn update(
        &mut self,
        visual: TrayVisualState,
        probing: bool,
        tray: &Tray,
        resting: &Icon,
        cx: &mut App,
    ) {
        if visual != TrayVisualState::Transitioning || probing {
            self.task = None;
            return;
        }
        if self.task.as_ref().is_some_and(|task| !task.is_ready()) || reduce_motion() {
            return;
        }
        if self.frames.is_none() {
            self.frames = Some(match crate::icons::build_spin_frames(cx) {
                Ok(frames) => frames.into(),
                Err(err) => {
                    crate::diag::log(&format!(
                        "tray: could not build globe animation frames: {err}"
                    ));
                    Vec::new().into()
                }
            });
        }
        let Some(frames) = self
            .frames
            .as_ref()
            .filter(|frames| !frames.is_empty())
            .cloned()
        else {
            return;
        };
        let tray = tray.clone();
        let resting = resting.clone();
        self.task = Some(cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
            // 42 frames at 52 ms give a 2.18-second sweep.
            for frame in frames.iter().cycle().skip(1) {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(52))
                    .await;
                let reduce = reduce_motion();
                let icon = if reduce { &resting } else { frame };
                if cx.update(|cx| tray.set_icon(icon.clone(), cx)).is_err() || reduce {
                    break;
                }
            }
        }));
    }
}

#[cfg(target_os = "macos")]
fn reduce_motion() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    // SAFETY: NSWorkspace exposes this Boolean property on all supported macOS versions.
    unsafe { objc2::msg_send![&*workspace, accessibilityDisplayShouldReduceMotion] }
}

/// Keep everyday switches one click away, and group larger libraries.
const INLINE_PROFILE_LIMIT: usize = 4;

fn build_tray_menu(store: &AppStore) -> Vec<MenuItem> {
    let snap = &store.snapshot;
    let visual = store.tray_visual_state();
    // Ask the store rather than re-deriving "busy" from the visual state.
    let busy = store.is_busy();
    let live = snap.active.as_deref().filter(|_| snap.running);
    let status = if store.is_probing() {
        // Nothing has been observed yet; naming a state here would be a guess.
        "Checking…".to_string()
    } else {
        match visual {
            TrayVisualState::Connected => {
                live.map_or_else(|| "Connected".into(), |name| format!("Connected to {name}"))
            }
            TrayVisualState::Disconnected => "Disconnected".into(),
            TrayVisualState::Transitioning => "Updating connection…".into(),
            TrayVisualState::Broken => "Connection issue".into(),
        }
    };
    let mut items = vec![MenuItem::action(status, Noop).disabled(true)];
    let target = (!snap.running)
        .then(|| store.ready_connection_target())
        .flatten();
    let unavailable = busy;

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
