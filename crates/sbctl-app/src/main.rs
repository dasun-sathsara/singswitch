#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

extern crate gpui_kit as gpui;

mod actions;
mod icons;
mod shell;
mod state;
mod theme;
mod tray;
mod views;

use gpui_kit::component::TitleBar;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Entity, Global, KeyBinding, QuitMode, WindowBounds,
    WindowOptions, open_window, px, size,
};

use actions::{
    CloseWindow, ConnectActive, DisconnectService, ImportUrlDialog, NavTab, NewProfileDialog, Noop,
    OpenLogsView, OpenMainWindow, OpenSettingsView, QuitApp, RefreshIp, RunDoctor,
    SaveAndActivateProfile, SaveCurrentProfile, SelectTab, SwitchProfile, SyncAll, SyncProfile,
    ToggleService,
};
use shell::{install_app_menus, set_dock_visible};
use state::AppStore;
use theme::apply_theme;
use tray::TrayController;
use views::WorkspaceShell;
use views::profiles::ProfilesView;

struct AppGlobalState {
    store: Entity<AppStore>,
    main_window: Option<AnyWindowHandle>,
    main_view: Option<Entity<WorkspaceShell>>,
    _tray: Option<TrayController>,
}

impl Global for AppGlobalState {}

fn main() {
    let start_background = std::env::args().any(|a| a == "--background" || a == "--minimized");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);

            let store = cx.new(AppStore::new);
            let theme_pref = store.read(cx).settings.theme;
            apply_theme(theme_pref, None, cx);
            install_app_menus(cx);
            bind_global_shortcuts(cx);

            let tray = TrayController::install(store.clone(), cx).ok();

            cx.set_global(AppGlobalState {
                store: store.clone(),
                main_window: None,
                main_view: None,
                _tray: tray,
            });

            register_global_actions(cx);

            if start_background {
                set_dock_visible(false);
            } else {
                open_or_focus_main_window(cx);
            }
        });
}

fn bind_global_shortcuts(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new(
            "cmd-1",
            SelectTab {
                tab: NavTab::Dashboard,
            },
            None,
        ),
        KeyBinding::new(
            "cmd-2",
            SelectTab {
                tab: NavTab::Profiles,
            },
            None,
        ),
        KeyBinding::new("cmd-3", SelectTab { tab: NavTab::Logs }, None),
        KeyBinding::new(
            "cmd-4",
            SelectTab {
                tab: NavTab::Doctor,
            },
            None,
        ),
        KeyBinding::new(
            "cmd-5",
            SelectTab {
                tab: NavTab::Settings,
            },
            None,
        ),
        KeyBinding::new("cmd-n", NewProfileDialog, None),
        KeyBinding::new("cmd-i", ImportUrlDialog, None),
        KeyBinding::new("cmd-s", SaveCurrentProfile, None),
        KeyBinding::new("cmd-shift-s", SaveAndActivateProfile, None),
        KeyBinding::new("cmd-r", RefreshIp, None),
        KeyBinding::new("cmd-,", OpenSettingsView, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-q", QuitApp, None),
    ]);

    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new(
            "ctrl-1",
            SelectTab {
                tab: NavTab::Dashboard,
            },
            None,
        ),
        KeyBinding::new(
            "ctrl-2",
            SelectTab {
                tab: NavTab::Profiles,
            },
            None,
        ),
        KeyBinding::new("ctrl-3", SelectTab { tab: NavTab::Logs }, None),
        KeyBinding::new(
            "ctrl-4",
            SelectTab {
                tab: NavTab::Doctor,
            },
            None,
        ),
        KeyBinding::new(
            "ctrl-5",
            SelectTab {
                tab: NavTab::Settings,
            },
            None,
        ),
        KeyBinding::new("ctrl-n", NewProfileDialog, None),
        KeyBinding::new("ctrl-i", ImportUrlDialog, None),
        KeyBinding::new("ctrl-s", SaveCurrentProfile, None),
        KeyBinding::new("ctrl-shift-s", SaveAndActivateProfile, None),
        KeyBinding::new("ctrl-r", RefreshIp, None),
        KeyBinding::new("ctrl-,", OpenSettingsView, None),
        KeyBinding::new("ctrl-w", CloseWindow, None),
        KeyBinding::new("ctrl-q", QuitApp, None),
    ]);
}

fn open_or_focus_main_window(cx: &mut App) {
    set_dock_visible(true);

    let existing = cx.global::<AppGlobalState>().main_window;
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| {
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }

    let (store, width, height) = {
        let g = cx.global::<AppGlobalState>();
        let s = g.store.read(cx);
        (
            g.store.clone(),
            s.settings.window_width.max(820.0),
            s.settings.window_height.max(540.0),
        )
    };

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(width), px(height)), cx)),
        window_min_size: Some(size(px(820.0), px(540.0))),
        ..TitleBar::window_options()
    };

    if let Ok((handle, view)) = open_window(options, cx, move |window, cx| {
        cx.new(|cx| WorkspaceShell::new(store.clone(), window, cx))
    }) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        let g = cx.global_mut::<AppGlobalState>();
        g.main_window = Some(handle);
        g.main_view = Some(view);
    }
}

fn with_main_window_and_store(
    cx: &mut App,
    f: impl FnOnce(Entity<AppStore>, &mut gpui_kit::Window, &mut App) + 'static,
) {
    open_or_focus_main_window(cx);
    if let Some(handle) = cx.global::<AppGlobalState>().main_window {
        let store = cx.global::<AppGlobalState>().store.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            f(store, window, cx);
        });
    }
}

fn trigger_active_save(and_activate: bool, cx: &mut App) {
    open_or_focus_main_window(cx);
    let (handle_opt, view_opt) = {
        let g = cx.global::<AppGlobalState>();
        (g.main_window, g.main_view.clone())
    };
    if let (Some(handle), Some(view)) = (handle_opt, view_opt) {
        let _ = handle.update(cx, move |_, window, cx| {
            view.update(cx, |shell, cx| {
                shell.trigger_save_profile(and_activate, window, cx);
            });
        });
    }
}

fn register_global_actions(cx: &mut App) {
    cx.on_action(|_: &Noop, _| {});

    cx.on_action(|_: &QuitApp, cx: &mut App| {
        cx.quit();
    });

    cx.on_action(|_: &OpenMainWindow, cx: &mut App| {
        open_or_focus_main_window(cx);
    });

    cx.on_action(|_: &OpenLogsView, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.set_tab(NavTab::Logs, cx));
        open_or_focus_main_window(cx);
    });

    cx.on_action(|_: &OpenSettingsView, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.set_tab(NavTab::Settings, cx));
        open_or_focus_main_window(cx);
    });

    cx.on_action(|action: &SelectTab, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.set_tab(action.tab, cx));
        open_or_focus_main_window(cx);
    });

    cx.on_action(|_: &RunDoctor, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| {
            s.set_tab(NavTab::Doctor, cx);
            s.run_doctor(cx);
        });
        open_or_focus_main_window(cx);
    });

    cx.on_action(|_: &SaveCurrentProfile, cx: &mut App| {
        trigger_active_save(false, cx);
    });

    cx.on_action(|_: &SaveAndActivateProfile, cx: &mut App| {
        trigger_active_save(true, cx);
    });

    cx.on_action(|_: &ToggleService, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.toggle_service(cx));
    });

    cx.on_action(|_: &ConnectActive, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.connect_active(cx));
    });

    cx.on_action(|_: &DisconnectService, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.disconnect(cx));
    });

    cx.on_action(|action: &SwitchProfile, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        let name = action.name.clone();
        store.update(cx, |s, cx| s.activate_profile(name, cx));
    });

    cx.on_action(|action: &SyncProfile, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        let name = action.name.clone();
        store.update(cx, |s, cx| s.sync_profile(name, cx));
    });

    cx.on_action(|_: &SyncAll, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| s.sync_all(cx));
    });

    cx.on_action(|_: &RefreshIp, cx: &mut App| {
        let store = cx.global::<AppGlobalState>().store.clone();
        store.update(cx, |s, cx| {
            s.refresh_snapshot(false, cx);
            s.refresh_ip(cx);
        });
    });

    cx.on_action(|_: &NewProfileDialog, cx: &mut App| {
        with_main_window_and_store(cx, |store, window, cx| {
            store.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
            ProfilesView::open_new_profile_dialog(store, window, cx);
        });
    });

    cx.on_action(|_: &ImportUrlDialog, cx: &mut App| {
        with_main_window_and_store(cx, |store, window, cx| {
            store.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
            ProfilesView::open_import_url_dialog(store, window, cx);
        });
    });

    cx.on_action(|_: &CloseWindow, cx: &mut App| {
        let (handle, store) = {
            let g = cx.global::<AppGlobalState>();
            (g.main_window, g.store.clone())
        };
        if let Some(handle) = handle {
            let _ = handle.update(cx, |_, window, cx| {
                let b = window.bounds().size;
                store.update(cx, |s, cx| {
                    s.update_window_size(b.width.as_f32(), b.height.as_f32());
                    s.set_window_open(false, cx);
                });
                window.remove_window();
            });
        }
        set_dock_visible(false);
    });
}
