use gpui_kit::{App, AsyncApp, Entity, Menu, MenuItem, Subscription};
use gpui_tray::{Icon, Tray};
use singswitch_core::checker::{DEFAULT_LATENCY_PROBE_URL, ExecChecker};
use singswitch_core::form_model::ProfileFormModel;

use crate::actions::{
    DisconnectService, EditProfile, Noop, OpenLogsView, OpenMainWindow, QuitApp, SwitchProfile,
    TestLatency,
};
use crate::icons::{TrayVisualState, build_tray_icon};
use crate::state::{AppStore, BannerTone, NoticeOrigin};

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
    let mut items = build_menu_items(store);
    // A probe spawns a process and waits on the network, so it is disabled on
    // exactly the same terms as a switch.
    items.extend(latency_items(store, store.is_busy()));
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Open SingSwitch", OpenMainWindow));
    items.push(MenuItem::action("Quit SingSwitch", QuitApp));
    items
}

fn build_menu_items(store: &AppStore) -> Vec<MenuItem> {
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

    items
}

/// The "Test latency" section: one row per profile, in name order.
///
/// Its own section rather than a row inside the switch list, because the reason
/// to use it is the profile you are *not* connected to: measuring an alternative
/// before switching is the whole point, and it never changes the connection.
/// The result goes to the notification centre, since no tray row can be updated
/// in place.
fn latency_items(store: &AppStore, disabled: bool) -> Vec<MenuItem> {
    let mut profiles: Vec<&str> = store
        .snapshot
        .profiles
        .iter()
        .filter(|profile| profile.config_parsed && profile.facts.endpoint.is_some())
        .map(|profile| profile.name.as_str())
        .collect();
    profiles.sort_unstable();
    if profiles.is_empty() {
        return Vec::new();
    }
    let mut items = vec![MenuItem::separator()];
    let rows: Vec<MenuItem> = profiles
        .into_iter()
        .map(|name| {
            MenuItem::action(
                format!("Test latency: {name}"),
                TestLatency {
                    name: name.to_owned(),
                },
            )
            .disabled(disabled)
        })
        .collect();
    if rows.len() <= INLINE_PROFILE_LIMIT {
        items.extend(rows);
    } else {
        items.push(MenuItem::submenu(Menu {
            name: "Test latency".into(),
            items: rows,
            disabled,
        }));
    }
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

/// Run a latency test for a profile the tray named, and report it where a tray
/// click can be heard.
///
/// The tray has no rows to update in place, and the window may be hidden when
/// the menu is used, so the answer goes to the notification centre (subject to
/// the existing desktop-notification setting) and to the toolbar strip for the
/// case where the window is open. Testing must never touch the running daemon:
/// `tools fetch` only pre-starts the outbound.
pub fn run_latency_test(store: Entity<AppStore>, name: String, cx: &mut App) {
    if store.read(cx).is_busy() {
        return;
    }
    let Some(path) = store
        .read(cx)
        .snapshot
        .profiles
        .iter()
        .find(|profile| profile.name == name)
        .map(|profile| profile.path.clone())
    else {
        store.update(cx, |store, cx| {
            store.set_notice(
                NoticeOrigin::Toolbar,
                BannerTone::Warning,
                format!("There is no profile named {name}"),
                cx,
            );
        });
        return;
    };

    cx.spawn(async move |cx: &mut AsyncApp| {
        let (name, result) = cx
            .background_executor()
            .spawn(async move {
                // The outbound tag lives in the document, and the store's
                // snapshot does not carry it.
                let tag = std::fs::read_to_string(&path)
                    .map_err(|err| format!("could not read {name}: {err}"))
                    .and_then(|raw| {
                        ProfileFormModel::from_json_str(&raw)
                            .map_err(|err| format!("{name} is not a valid config: {err}"))
                    })
                    .and_then(|model| match model.has_managed_outbound {
                        true => Ok(model.outbound_tag),
                        false => Err(format!("{name} has no outbound to test")),
                    });
                let result = tag.and_then(|tag| {
                    ExecChecker::default()
                        .probe_latency(&path, &tag, DEFAULT_LATENCY_PROBE_URL)
                        .map_err(|err| match err.hints.is_empty() {
                            true => err.message.clone(),
                            false => format!("{} — {}", err.message, err.hints.join("; ")),
                        })
                });
                (name, result)
            })
            .await;

        store.update(cx, |store, cx| {
            match result {
                Ok(elapsed) => {
                    let message =
                        format!("{name}: {}", crate::views::editor::format_latency(elapsed));
                    store.set_notice(
                        NoticeOrigin::Toolbar,
                        BannerTone::Success,
                        message.clone(),
                        cx,
                    );
                    if let Some(ctrl) = &store.ctrl {
                        ctrl.notifier.notify(&format!("Latency {message}"));
                    }
                }
                Err(message) => {
                    store.set_notice(
                        NoticeOrigin::Toolbar,
                        BannerTone::Error,
                        message.clone(),
                        cx,
                    );
                    if let Some(ctrl) = &store.ctrl {
                        ctrl.notifier.notify_error("Latency test failed", &message);
                    }
                }
            };
            cx.notify();
        });
    })
    .detach();
}
