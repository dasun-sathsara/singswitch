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
        Noop, OpenLogsView, OpenMainWindow, OpenSettingsView, QuitApp, RefreshIp,
        SaveAndActivateProfile, SaveCurrentProfile, SelectTab, SwitchProfile, SyncAll, SyncProfile,
        ToggleService,
    };
    use crate::shell::{install_app_menus, set_dock_visible};
    use crate::state::{AppStore, NoticeOrigin};
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

        let app = gpui_kit::application()
            .with_assets(gpui_kit::assets::Assets)
            .with_quit_mode(QuitMode::Explicit);

        // Closing the window leaves an Accessory app with no Dock icon, so
        // double-clicking it in Finder or the Dock did nothing at all. The
        // callback runs with a fresh app borrow, which is why this is registered
        // on the builder rather than from inside `run`.
        app.on_reopen(|cx: &mut App| {
            crate::desktop::request_open_window(cx);
        });

        app.run(move |cx: &mut App| {
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
            // Bind first: macOS menu key equivalents are captured once when
            // the menu is built, so building menus before the bindings gave
            // every app-action item no shortcut to display.
            bind_global_shortcuts(cx);
            install_app_menus(cx);

            let tray = TrayController::install(&store, cx).ok();
            let has_tray = tray.is_some();
            store.update(cx, |s, _| s.tray_available = has_tray);
            if has_tray {
                crate::diag::log("tray installed");
            } else {
                crate::diag::log("tray unavailable; app runs as a plain window");
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
            KeyBinding::new("cmd-backspace", crate::actions::DeleteSelectedProfile, None),
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
            KeyBinding::new(
                "ctrl-backspace",
                crate::actions::DeleteSelectedProfile,
                None,
            ),
            KeyBinding::new("ctrl-shift-k", ToggleService, None),
            KeyBinding::new("ctrl-,", OpenSettingsView, None),
            KeyBinding::new("ctrl-w", CloseWindow, None),
            KeyBinding::new("ctrl-q", QuitApp, None),
        ]);
    }

    /// Entry point for the platform reopen callback (Finder/Dock activation).
    pub(crate) fn request_open_window(cx: &mut App) {
        if cx.try_global::<AppGlobalState>().is_none() {
            return;
        }
        open_or_focus_main_window(cx);
    }

    fn open_or_focus_main_window(cx: &mut App) {
        set_dock_visible(true);

        let existing = cx.global::<AppGlobalState>().main_window;
        if let Some(handle) = existing {
            // Re-focus whatever window is still attached to this handle. Ask the
            // app which windows exist rather than inferring it from update's
            // result: a window that exists but is not updatable right now (for
            // example while an action is being dispatched) would otherwise send
            // us down the open path and leave a second window behind.
            if cx
                .windows()
                .iter()
                .any(|w| w.window_id() == handle.window_id())
            {
                if handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_err()
                {
                    crate::diag::log("main window present but not focusable");
                }
                return;
            }
            // Stale handle: the window went away without the close path running.
            let g = cx.global_mut::<AppGlobalState>();
            g.main_window = None;
            g.main_view = None;
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
            store.update(cx, |s, cx| s.disconnect(NoticeOrigin::Toolbar, cx));
        });

        cx.on_action(|action: &SwitchProfile, cx: &mut App| {
            let store = cx.global::<AppGlobalState>().store.clone();
            let name = action.name.clone();
            store.update(cx, |s, cx| {
                s.activate_profile(name, NoticeOrigin::Toolbar, cx)
            });
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
            // Reached from the menu on any tab, so report where the user is.
            let origin = store.read(cx).notice_origin_for_current_tab();
            store.update(cx, |s, cx| s.restart_active(origin, cx));
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
            // Cmd-W and File > Close Window must honour the same rule as the
            // native close button: hiding the window is only safe when a tray
            // can bring it back. window.remove_window() skips
            // on_window_should_close, so without this check a machine whose
            // tray failed to install loses the window and the Dock icon.
            if !store.read(cx).tray_available {
                crate::diag::log("close ignored: no tray to restore the window from");
                return;
            }
            if let Some(handle) = handle {
                // Only drop the handle once the window is actually gone;
                // clearing it on failure would make the next open spawn a
                // second window beside the first.
                let closed = handle
                    .update(cx, |_, window, cx| {
                        let b = window.bounds().size;
                        store.update(cx, |s, cx| {
                            s.update_window_size(b.width.as_f32(), b.height.as_f32(), true);
                            s.set_window_open(false, cx);
                        });
                        window.remove_window();
                        true
                    })
                    .unwrap_or(false);
                if !closed {
                    crate::diag::log("close failed: window handle no longer valid");
                    return;
                }
                // Drop the handle to the removed window's view tree. Keeping it
                // retained the whole shell, editor rope, drafts and both log
                // buffers for as long as the app sat in the tray.
                let g = cx.global_mut::<AppGlobalState>();
                g.main_window = None;
                g.main_view = None;
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
