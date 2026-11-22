#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(target_os = "macos", windows))]
extern crate gpui_kit as gpui;

#[cfg(any(target_os = "macos", windows))]
mod actions;
#[cfg(any(target_os = "macos", windows))]
mod diag;
#[cfg(any(target_os = "macos", windows))]
mod fonts;
#[cfg(any(target_os = "macos", windows))]
mod icons;
#[cfg(any(target_os = "macos", windows))]
mod notifier;
#[cfg(any(target_os = "macos", windows))]
mod shell;
#[cfg(any(target_os = "macos", windows))]
mod state;
#[cfg(any(target_os = "macos", windows))]
mod theme;
#[cfg(any(target_os = "macos", windows))]
mod tray;
#[cfg(any(target_os = "macos", windows))]
mod views;

#[cfg(any(target_os = "macos", windows))]
mod desktop {
    use gpui_kit::component::TitleBar;
    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, Entity, Global, KeyBinding, QuitMode, WindowBounds,
        WindowOptions, open_window, px, size,
    };

    use crate::actions::{
        CloseWindow, ConnectActive, DisconnectService, ImportUrlDialog, NavTab, NewProfileDialog,
        Noop, OpenLogsView, OpenMainWindow, OpenSettingsView, QuitApp, RefreshIp, RunDoctor,
        SaveAndActivateProfile, SaveCurrentProfile, SelectTab, SwitchProfile, SyncAll, SyncProfile,
        ToggleService,
    };
    use crate::shell::{install_app_menus, set_dock_visible};
    use crate::state::AppStore;
    use crate::theme::{MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, apply_theme};
    use crate::tray::TrayController;
    use crate::views::WorkspaceShell;
    use crate::views::profiles::ProfilesView;

    pub struct AppGlobalState {
        pub store: Entity<AppStore>,
        pub main_window: Option<AnyWindowHandle>,
        pub main_view: Option<Entity<WorkspaceShell>>,
        pub tray: Option<TrayController>,
    }

    impl Global for AppGlobalState {}

    pub fn main() {
        let start_background = std::env::args().any(|a| a == "--background" || a == "--minimized");

        crate::diag::install_panic_hook();
        crate::diag::log(&format!(
            "starting sbctl-app {} background={start_background}",
            env!("CARGO_PKG_VERSION")
        ));

        gpui_kit::application()
            .with_assets(gpui_kit::assets::Assets)
            .with_quit_mode(QuitMode::Explicit)
            .run(move |cx: &mut App| {
                gpui_kit::init(cx);
                crate::fonts::install(cx);

                // The stock 500ms delay reads as "tooltips don't work" on icons
                // that carry meaning only in their tooltip. 180ms still avoids
                // flicker while feeling immediate; the grace period lets the
                // pointer move between neighbouring icon buttons without
                // re-waiting.
                gpui_kit::component::tooltip::TooltipDefaults::new()
                    .with_show_delay(std::time::Duration::from_millis(180))
                    .with_grace_period(std::time::Duration::from_millis(400))
                    .install(cx);

                let store = cx.new(AppStore::new);
                let theme_pref = store.read(cx).settings.theme;
                apply_theme(theme_pref, None, cx);
                install_app_menus(cx);
                bind_global_shortcuts(cx);

                let tray = TrayController::install(&store, cx).ok();
                match &tray {
                    Some(_) => crate::diag::log("tray installed"),
                    None => crate::diag::log("tray unavailable; app runs as a plain window"),
                }

                cx.set_global(AppGlobalState {
                    store: store.clone(),
                    main_window: None,
                    main_view: None,
                    tray,
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
                    tab: NavTab::Profiles,
                },
                None,
            ),
            KeyBinding::new("cmd-2", SelectTab { tab: NavTab::Logs }, None),
            KeyBinding::new(
                "cmd-3",
                SelectTab {
                    tab: NavTab::Doctor,
                },
                None,
            ),
            KeyBinding::new(
                "cmd-4",
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
            KeyBinding::new("cmd-e", crate::actions::EditSelectedProfile, None),
            KeyBinding::new("cmd-shift-k", ToggleService, None),
            KeyBinding::new("cmd-,", OpenSettingsView, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-q", QuitApp, None),
        ]);

        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new(
                "ctrl-1",
                SelectTab {
                    tab: NavTab::Profiles,
                },
                None,
            ),
            KeyBinding::new("ctrl-2", SelectTab { tab: NavTab::Logs }, None),
            KeyBinding::new(
                "ctrl-3",
                SelectTab {
                    tab: NavTab::Doctor,
                },
                None,
            ),
            KeyBinding::new(
                "ctrl-4",
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
            KeyBinding::new("ctrl-e", crate::actions::EditSelectedProfile, None),
            KeyBinding::new("ctrl-shift-k", ToggleService, None),
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
                s.settings.window_width.max(MIN_WINDOW_WIDTH),
                s.settings.window_height.max(MIN_WINDOW_HEIGHT),
            )
        };

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(width), px(height)), cx)),
            window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
            ..TitleBar::window_options()
        };

        match open_window(options, cx, move |window, cx| {
            cx.new(|cx| WorkspaceShell::new(store.clone(), window, cx))
        }) {
            Ok((handle, view)) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                let g = cx.global_mut::<AppGlobalState>();
                g.main_window = Some(handle);
                g.main_view = Some(view);
                crate::diag::log("main window opened");
            }
            Err(err) => crate::diag::log(&format!("main window failed: {err}")),
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
        register_navigation_actions(cx);
        register_service_actions(cx);
        register_window_actions(cx);
    }

    fn register_navigation_actions(cx: &mut App) {
        cx.on_action(|_: &Noop, _| {});

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
    }

    fn register_service_actions(cx: &mut App) {
        cx.on_action(|_: &SaveCurrentProfile, cx: &mut App| trigger_active_save(false, cx));
        cx.on_action(|_: &SaveAndActivateProfile, cx: &mut App| trigger_active_save(true, cx));

        cx.on_action(|_: &ToggleService, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, AppStore::toggle_service);
        });

        cx.on_action(|_: &ConnectActive, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, AppStore::connect_active);
        });

        cx.on_action(|_: &DisconnectService, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, AppStore::disconnect);
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
            store.update(cx, AppStore::sync_all);
        });

        cx.on_action(|_: &RefreshIp, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| {
                s.refresh_snapshot(false, cx);
                s.refresh_ip(cx);
            });
        });
    }

    fn register_window_actions(cx: &mut App) {
        cx.on_action(|_: &crate::actions::OpenDoctorView, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| {
                s.set_tab(NavTab::Doctor, cx);
                s.run_doctor(cx);
            });
            open_or_focus_main_window(cx);
        });

        cx.on_action(|action: &crate::actions::EditProfile, cx: &mut App| {
            let name = action.name.clone();
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| {
                s.select_profile(Some(name), cx);
                s.begin_edit(cx);
            });
            open_or_focus_main_window(cx);
        });

        cx.on_action(|_: &crate::actions::BackToConnections, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| s.end_edit(cx));
        });

        cx.on_action(|_: &crate::actions::EditSelectedProfile, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| s.begin_edit(cx));
            open_or_focus_main_window(cx);
        });

        cx.on_action(|_: &crate::actions::RestartService, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, AppStore::restart_active);
        });

        cx.on_action(|_: &crate::actions::RefreshStatus, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            store.update(cx, |s, cx| s.refresh_snapshot(true, cx));
        });

        cx.on_action(|_: &crate::actions::DeleteSelectedProfile, cx: &mut App| {
            with_main_window_and_store(cx, |store, window, cx| {
                crate::views::profiles::confirm_delete_selected(store, window, cx);
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

        cx.on_action(|_: &QuitApp, cx: &mut App| {
            if let Some(tray) = cx.global_mut::<AppGlobalState>().tray.take() {
                tray.close(cx);
            }
            cx.quit();
        });
    }
}

#[cfg(any(target_os = "macos", windows))]
fn main() {
    desktop::main();
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("sbctl-app requires macOS or Windows. Run sbctl instead.");
    std::process::exit(1);
}
