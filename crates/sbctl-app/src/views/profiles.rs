use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::icons::TrayVisualState;
use crate::state::{AppStore, NoticeOrigin};
use crate::views::editor::ProfileEditorPanel;
use sbctl_core::logs::contains_ignore_ascii_case;
use sbctl_core::profile::Profile;

pub struct ProfilesView {
    store: Entity<AppStore>,
    editor: Entity<ProfileEditorPanel>,
    search_input: Entity<InputState>,
    details_open: bool,
    _subs: [Subscription; 2],
}

impl ProfilesView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| ProfileEditorPanel::new(store.clone(), window, cx));
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search profiles")
                .default_value(store.read(cx).profile_search_query.clone())
        });

        let store_sub = store.clone();
        let sub = cx.subscribe_in(&search_input, window, move |_, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let q = state.read(cx).value().to_string();
                store_sub.update(cx, |s, cx| {
                    s.profile_search_query = q;
                    cx.notify();
                });
            }
        });

        let store_observer = cx.observe(&store, |_, _, cx| cx.notify());

        Self {
            store,
            editor,
            search_input,
            details_open: false,
            _subs: [sub, store_observer],
        }
    }

    pub fn trigger_save(
        &mut self,
        and_activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |ed, cx| {
            ed.trigger_save(and_activate, window, cx);
        });
    }

    pub fn open_new_profile_dialog(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. home-vless"));
        let initial_focus = Cell::new(true);
        let error = Rc::new(RefCell::new(None::<String>));

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let input_for_ok = name_input.clone();
            let store_for_ok = store.clone();
            let error_for_ok = error.clone();

            dialog
                .title("New profile")
                .width(px(420.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Create a profile from the VLESS + Reality template."),
                        )
                        .child(Input::new(&name_input).cleanable(true)),
                )
                .when_some(error.borrow().clone(), |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("new-prof-cancel")
                                .secondary()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-prof-create")
                                .primary()
                                .label("Create")
                                .on_click(move |_, window, cx| {
                                    if store_for_ok.read(cx).busy_phase.is_some() {
                                        *error_for_ok.borrow_mut() = Some(
                                            "Wait for the current operation to finish.".into(),
                                        );
                                        window.refresh();
                                        return;
                                    }
                                    let raw_name = input_for_ok.read(cx).value().trim().to_string();
                                    let res = store_for_ok
                                        .update(cx, |s, cx| s.create_profile(&raw_name, cx));
                                    match res {
                                        Ok(()) => window.close_dialog(cx),
                                        Err(err) => {
                                            *error_for_ok.borrow_mut() = Some(err.to_string());
                                            window.refresh();
                                        }
                                    }
                                }),
                        ),
                )
        });
    }

    pub fn open_import_url_dialog(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Profile name"));
        let url_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("https://example.com/sing-box.json"));
        let initial_focus = Cell::new(true);
        let error = Rc::new(RefCell::new(None::<String>));

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let n_ok = name_input.clone();
            let u_ok = url_input.clone();
            let s_ok = store.clone();
            let error_for_ok = error.clone();

            dialog
                .title("Import profile")
                .width(px(480.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Fetch a sing-box JSON config over HTTP(S) and validate it.",
                                ),
                        )
                        .child(Input::new(&name_input).cleanable(true))
                        .child(Input::new(&url_input).cleanable(true)),
                )
                .when_some(error.borrow().clone(), |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("import-cancel")
                                .secondary()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("import-submit")
                                .primary()
                                .label("Import")
                                .on_click(move |_, window, cx| {
                                    let name = n_ok.read(cx).value().trim().to_string();
                                    let url = u_ok.read(cx).value().trim().to_string();
                                    let error = if s_ok.read(cx).busy_phase.is_some() {
                                        Some("Wait for the current operation to finish.".into())
                                    } else if url.is_empty() {
                                        Some("Enter the configuration URL.".into())
                                    } else {
                                        sbctl_core::profile::validate_name(&name)
                                            .and_then(|()| {
                                                sbctl_core::remote::normalise_profile_url(&url)
                                                    .map(|_| ())
                                            })
                                            .err()
                                            .map(|err| err.to_string())
                                    };
                                    if let Some(error) = error {
                                        *error_for_ok.borrow_mut() = Some(error);
                                        window.refresh();
                                        return;
                                    }
                                    s_ok.update(cx, |s, cx| {
                                        s.pull_profile(name, url, NoticeOrigin::Stage, cx)
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn render_connections(&self, cx: &Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let busy = store.busy_phase.is_some();
        let mut content = v_flex()
            .size_full()
            .px(px(28.0))
            .pt(px(22.0))
            .pb(px(24.0))
            .gap(px(16.0))
            .child(self.render_connection(cx))
            .when_some(
                super::notice_strip(&self.store, NoticeOrigin::Stage, cx),
                |page, notice| page.child(notice),
            );

        if store.snapshot.profiles.is_empty() {
            let create = self.store.clone();
            let import = self.store.clone();
            return content.child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .justify_center()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(14.0))
                    .bg(crate::theme::surface(cx))
                    .child(div().text_size(px(22.0)).font_bold().child("Add your first profile"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Create a VLESS + Reality profile or import a sing-box configuration."),
                    )
                    .child(
                        h_flex()
                            .mt(px(6.0))
                            .gap(px(8.0))
                            .child(
                                Button::new("first-profile-new")
                                    .primary()
                                    .label("New profile")
                                    .disabled(busy || store.init_error.is_some())
                                    .on_click(move |_, window, cx| {
                                        Self::open_new_profile_dialog(create.clone(), window, cx);
                                    }),
                            )
                            .child(
                                Button::new("first-profile-import")
                                    .secondary()
                                    .label("Import from URL")
                                    .disabled(busy || store.init_error.is_some())
                                    .on_click(move |_, window, cx| {
                                        Self::open_import_url_dialog(import.clone(), window, cx);
                                    }),
                            ),
                    ),
            );
        }

        content = content.child(
            h_flex()
                .w_full()
                .flex_none()
                .justify_between()
                .items_center()
                .child(div().text_size(px(20.0)).font_bold().child("Profiles"))
                .child(self.render_add_actions(store)),
        );
        let selected = store.selected_profile.as_deref();
        let profile = store
            .snapshot
            .profiles
            .iter()
            .find(|p| Some(p.name.as_str()) == selected);

        content.child(
            h_flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .gap(px(12.0))
                .items_stretch()
                .child(self.render_library(store, cx))
                .child(
                    v_flex()
                        .id("profile-details")
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_y_scroll()
                        .rounded(px(14.0))
                        .bg(crate::theme::surface(cx))
                        .p(px(20.0))
                        .child(match profile {
                            Some(profile) => self.render_profile(profile, cx).into_any_element(),
                            None => div()
                                .text_sm()
                                .child("Select a profile to view its settings.")
                                .into_any_element(),
                        }),
                ),
        )
    }

    fn render_connection(&self, cx: &Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let snap = &store.snapshot;
        let visual = store.tray_visual_state();
        let (title, tone, icon) = match visual {
            TrayVisualState::Transitioning => (
                "Updating connection",
                cx.theme().warning,
                IconName::LoaderCircle,
            ),
            TrayVisualState::Broken => ("Connection issue", cx.theme().danger, IconName::Info),
            TrayVisualState::Connected => ("Connected", cx.theme().success, IconName::Check),
            TrayVisualState::Disconnected => {
                ("Disconnected", cx.theme().muted_foreground, IconName::Minus)
            }
        };
        let issue = store
            .init_error
            .as_deref()
            .or(snap.broken.as_deref())
            .or_else(|| {
                (snap.health.state == sbctl_core::service::RunState::Error
                    && !snap.health.detail.is_empty())
                .then_some(snap.health.detail.as_str())
            });
        let subtitle = if let Some(phase) = &store.busy_phase {
            phase.clone()
        } else if let Some(issue) = issue {
            issue.to_owned()
        } else if snap.running {
            snap.active
                .clone()
                .unwrap_or_else(|| "sing-box is running".into())
        } else if snap.profiles.is_empty() {
            "Add a profile to connect.".into()
        } else {
            "Choose a profile below to connect.".into()
        };

        let disconnect = self.store.clone();
        let show_logs = self.store.clone();
        let refresh = self.store.clone();
        let network = self.store.clone();
        let ip = if store.ip_loading {
            "Checking…".into()
        } else if let Some(error) = &store.ip_error {
            format!("Lookup failed. {error}")
        } else if let Some(info) = &store.ip_info {
            let location = info.formatted_location();
            if location.is_empty() {
                info.ip.clone()
            } else {
                format!("{} · {location}", info.ip)
            }
        } else {
            "Not checked".into()
        };
        let ip_clipboard = store
            .ip_info
            .as_ref()
            .filter(|_| !store.ip_loading && store.ip_error.is_none())
            .map(|info| info.ip.clone());

        v_flex()
            .w_full()
            .flex_none()
            .p(px(18.0))
            .gap(px(14.0))
            .rounded(px(14.0))
            .bg(crate::theme::connection_surface(cx, tone))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(14.0))
                    .child(
                        h_flex()
                            .flex_none()
                            .size(px(44.0))
                            .justify_center()
                            .items_center()
                            .rounded(px(12.0))
                            .bg(tone.opacity(0.13))
                            .child(Icon::new(icon).text_color(tone)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(2.0))
                            .child(div().text_size(px(25.0)).font_bold().child(title))
                            .child(
                                div()
                                    .id("connection-subtitle")
                                    .text_sm()
                                    .when(visual == TrayVisualState::Broken, |detail| {
                                        detail.max_h(px(56.0)).overflow_y_scroll()
                                    })
                                    .when(visual != TrayVisualState::Broken, |detail| {
                                        detail.truncate()
                                    })
                                    .text_color(cx.theme().muted_foreground)
                                    .child(subtitle),
                            ),
                    )
                    .when(visual == TrayVisualState::Broken, |row| {
                        row.child(
                            Button::new("connection-logs")
                                .secondary()
                                .label("View logs")
                                .on_click(move |_, _, cx| {
                                    show_logs.update(cx, |s, cx| {
                                        s.set_tab(crate::actions::NavTab::Logs, cx)
                                    });
                                }),
                        )
                    })
                    .when(snap.running, |row| {
                        row.child(
                            Button::new("connection-disconnect")
                                .secondary()
                                .label("Disconnect")
                                .disabled(store.busy_phase.is_some() || store.init_error.is_some())
                                .on_click(move |_, _, cx| {
                                    disconnect
                                        .update(cx, |s, cx| s.disconnect(NoticeOrigin::Stage, cx));
                                }),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.0))
                    .pt(px(10.0))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Public IP"),
                    )
                    .child(
                        div()
                            .id("public-ip-state")
                            .flex_1()
                            .min_w(px(0.0))
                            .text_sm()
                            .when(store.ip_error.is_some() && !store.ip_loading, |line| {
                                line.max_h(px(40.0))
                                    .overflow_y_scroll()
                                    .text_color(cx.theme().danger)
                            })
                            .when(store.ip_error.is_none(), |line| line.truncate())
                            .child(ip),
                    )
                    .when_some(ip_clipboard, |row, ip| {
                        row.child(
                            Button::new("copy-ip")
                                .ghost()
                                .small()
                                .label("Copy IP")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                        ip.clone(),
                                    ));
                                }),
                        )
                    })
                    .child(
                        Button::new("refresh-network")
                            .ghost()
                            .small()
                            .label("Refresh")
                            .disabled(store.ip_loading)
                            .on_click(move |_, _, cx| {
                                refresh.update(cx, |s, cx| {
                                    s.refresh_snapshot(true, cx);
                                    s.refresh_ip(cx);
                                });
                            }),
                    )
                    .when(snap.running, |row| {
                        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
                        let busy = store.busy_phase.is_some();
                        row.child(
                            Button::new("connection-options")
                                .ghost()
                                .small()
                                .icon(IconName::Ellipsis)
                                .accessibility_label("Connection options")
                                .tooltip("Connection options")
                                .dropdown_menu(move |menu, _, _| {
                                    let network = network.clone();
                                    menu.item(
                                        PopupMenuItem::new("Restart service")
                                            .disabled(busy)
                                            .on_click(move |_, _, cx| {
                                                network.update(cx, |s, cx| {
                                                    s.restart_active(NoticeOrigin::Stage, cx)
                                                });
                                            }),
                                    )
                                }),
                        )
                    }),
            )
    }

    fn render_add_actions(&self, store: &AppStore) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let create = self.store.clone();
        let import = self.store.clone();
        let sync = self.store.clone();
        let busy = store.busy_phase.is_some() || store.init_error.is_some();
        let has_remote = store
            .snapshot
            .profiles
            .iter()
            .any(|p| p.source_url.is_some());

        h_flex()
            .gap(px(8.0))
            .when(has_remote, |row| {
                row.child(
                    Button::new("library-sync-all")
                        .ghost()
                        .small()
                        .label("Sync remote profiles")
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            sync.update(cx, |s, cx| s.sync_all(NoticeOrigin::Stage, cx));
                        }),
                )
            })
            .child(
                Button::new("library-add")
                    .secondary()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add profile")
                    .disabled(busy)
                    .dropdown_menu(move |menu, _, _| {
                        let create = create.clone();
                        let import = import.clone();
                        menu.item(PopupMenuItem::new("New profile…").on_click(
                            move |_, window, cx| {
                                Self::open_new_profile_dialog(create.clone(), window, cx);
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Import from URL…").on_click(
                                move |_, window, cx| {
                                    Self::open_import_url_dialog(import.clone(), window, cx);
                                },
                            ),
                        )
                    }),
            )
    }

    fn render_library(&self, store: &AppStore, cx: &Context<Self>) -> impl IntoElement {
        let query = store.profile_search_query.trim();
        let mut profiles = store
            .snapshot
            .profiles
            .iter()
            .filter(|p| {
                contains_ignore_ascii_case(&p.name, query)
                    || p.server
                        .as_deref()
                        .is_some_and(|server| contains_ignore_ascii_case(server, query))
            })
            .peekable();
        let none = profiles.peek().is_none();

        v_flex()
            .w(px(248.0))
            .flex_none()
            .min_h(px(0.0))
            .p(px(9.0))
            .gap(px(6.0))
            .rounded(px(14.0))
            .bg(crate::theme::surface(cx))
            .child(
                Input::new(&self.search_input)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).xsmall()),
            )
            .child(
                v_flex()
                    .id("profile-library")
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(2.0))
                    .overflow_y_scroll()
                    .when(none, |list| {
                        list.child(
                            div()
                                .p(px(10.0))
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No matching profiles"),
                        )
                    })
                    .children(profiles.map(|p| {
                        let selected = store.selected_profile.as_deref() == Some(p.name.as_str());
                        let live = store.snapshot.running
                            && store.snapshot.active.as_deref() == Some(p.name.as_str());
                        let app = self.store.clone();
                        let name = p.name.clone();
                        let text = if live {
                            "Connected".into()
                        } else if !p.ready() {
                            "Needs setup".into()
                        } else {
                            p.server
                                .clone()
                                .unwrap_or_else(|| "Local configuration".into())
                        };
                        Button::new(gpui_kit::SharedString::from(format!("profile-{}", p.name)))
                            .when(selected, Button::secondary)
                            .when(!selected, Button::ghost)
                            .toggled(selected)
                            .accessibility_label(format!("{}: {text}", p.name))
                            .h(px(52.0))
                            .w_full()
                            .flex_none()
                            .justify_start()
                            .rounded(px(8.0))
                            .on_click(move |_, _, cx| {
                                app.update(cx, |s, cx| s.select_profile(Some(name.clone()), cx));
                            })
                            .child(
                                v_flex()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_bold()
                                            .truncate()
                                            .child(p.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(if live {
                                                cx.theme().success
                                            } else if !p.ready() {
                                                cx.theme().warning
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .child(text),
                                    ),
                            )
                    })),
            )
    }

    fn render_profile(&self, profile: &Profile, cx: &Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let running = store.snapshot.running;
        let is_live = running && store.snapshot.active.as_deref() == Some(profile.name.as_str());
        let busy = store.busy_phase.is_some() || store.init_error.is_some();
        let ready = profile.ready();
        let name = profile.name.clone();
        let edit = self.store.clone();
        let activate = self.store.clone();
        let facts = &profile.facts;
        let endpoint = facts.endpoint.as_deref().or(profile.server.as_deref());
        let missing = if !profile.config_parsed {
            "The configuration could not be read as JSON.".to_owned()
        } else {
            let fields = profile
                .missing
                .iter()
                .map(|field| field.label())
                .collect::<Vec<_>>();
            format!("Required fields: {}", fields.join(", "))
        };

        v_flex()
            .w_full()
            .gap(px(12.0))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(22.0))
                            .font_bold()
                            .child(profile.name.clone()),
                    )
                    .child(self.render_profile_menu(profile, is_live, busy)),
            )
            .when(!ready, |panel| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(missing),
                )
            })
            .child(
                h_flex()
                    .gap(px(8.0))
                    .when(ready && !is_live, |row| {
                        row.child(
                            Button::new("profile-connect")
                                .primary()
                                .label(if running {
                                    "Switch to this profile"
                                } else {
                                    "Connect"
                                })
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    activate.update(cx, |s, cx| {
                                        s.activate_profile(name.clone(), NoticeOrigin::Stage, cx)
                                    });
                                }),
                        )
                    })
                    .child(
                        Button::new("profile-edit")
                            .when(ready, Button::secondary)
                            .when(!ready, Button::primary)
                            .label(if ready {
                                "Edit profile"
                            } else {
                                "Finish setup"
                            })
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                edit.update(cx, |s, cx| s.begin_edit(cx));
                            }),
                    ),
            )
            .child(
                v_flex()
                    .pt(px(2.0))
                    .when_some(endpoint, |section, address| {
                        section.child(detail_row("Server", address.to_owned(), cx))
                    })
                    .when_some(facts.outbound_type.as_ref(), |section, kind| {
                        section.child(detail_row("Protocol", kind.clone(), cx))
                    })
                    .when_some(
                        store.connected_since.filter(|_| is_live),
                        |section, since| {
                            section.child(detail_row(
                                "Connected for",
                                format_duration(since.elapsed().as_secs()),
                                cx,
                            ))
                        },
                    ),
            )
            .child(
                Button::new("profile-details-toggle")
                    .ghost()
                    .small()
                    .icon(if self.details_open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label("Configuration details")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.details_open = !this.details_open;
                        cx.notify();
                    })),
            )
            .when(self.details_open, |panel| {
                let tunnel = match facts.tun_state() {
                    sbctl_core::profile::TunState::Named => {
                        facts.tun_interface.clone().unwrap_or_default()
                    }
                    sbctl_core::profile::TunState::Unnamed => "OS default".into(),
                    sbctl_core::profile::TunState::Absent => "No TUN inbound".into(),
                };
                panel.child(
                    v_flex()
                        .child(detail_row("Tunnel", tunnel, cx))
                        .child(detail_row(
                            "Config file",
                            profile.path.display().to_string(),
                            cx,
                        ))
                        .when_some(profile.source_url.as_ref(), |section, url| {
                            section.child(detail_row("Source URL", url.clone(), cx))
                        })
                        .when(is_live, |section| {
                            let health = &store.snapshot.health;
                            section
                                .when_some(health.pid, |section, pid| {
                                    section.child(detail_row("Process ID", pid.to_string(), cx))
                                })
                                .when_some(health.restarts.filter(|n| *n > 0), |section, count| {
                                    section.child(detail_row("Restarts", count.to_string(), cx))
                                })
                                .when_some(health.last_exit.as_ref(), |section, code| {
                                    section.child(detail_row("Last exit", code.clone(), cx))
                                })
                        }),
                )
            })
    }

    fn render_profile_menu(
        &self,
        profile: &Profile,
        is_live: bool,
        busy: bool,
    ) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let app = self.store.clone();
        let name = profile.name.clone();
        let path = profile.path.display().to_string();
        let has_source = profile.source_url.is_some();

        Button::new("profile-options")
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label("Profile options")
            .tooltip("Profile options")
            .dropdown_menu(move |menu, _, _| {
                let sync = app.clone();
                let delete = app.clone();
                let sync_name = name.clone();
                let delete_name = name.clone();
                let path = path.clone();

                menu.when(has_source, |menu| {
                    menu.item(
                        PopupMenuItem::new("Sync from source")
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                sync.update(cx, |s, cx| {
                                    s.sync_profile(sync_name.clone(), NoticeOrigin::Stage, cx)
                                });
                            }),
                    )
                })
                .item(
                    PopupMenuItem::new("Copy config path").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Delete profile…")
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            open_delete_dialog(
                                delete_name.clone(),
                                is_live,
                                NoticeOrigin::Stage,
                                delete.clone(),
                                window,
                                cx,
                            );
                        }),
                )
            })
    }
}

fn detail_row(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .py(px(8.0))
        .gap(px(12.0))
        .items_baseline()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .w(px(100.0))
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .truncate()
                .child(value),
        )
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        if s.editing {
            v_flex()
                .size_full()
                .px(px(28.0))
                .py(px(18.0))
                .child(self.editor.clone())
                .into_any_element()
        } else {
            self.render_connections(cx).into_any_element()
        }
    }
}

/// Confirm deleting whichever profile is selected. Used by the keyboard action so
/// the warning about stopping a running service is never skipped.
pub fn confirm_delete_selected(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
    let (name, is_live) = {
        let s = store.read(cx);
        let Some(name) = s.selected_profile.clone() else {
            return;
        };
        let live = s.snapshot.running && s.snapshot.active.as_deref() == Some(name.as_str());
        (name, live)
    };
    // The dialog is opened from the editor or the stage; report back to the
    // surface the user is actually looking at.
    let origin = if store.read(cx).editing {
        NoticeOrigin::Editor
    } else {
        NoticeOrigin::Stage
    };
    open_delete_dialog(name, is_live, origin, store, window, cx);
}

fn open_delete_dialog(
    name: String,
    is_connected: bool,
    origin: NoticeOrigin,
    store: Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let n_ok = name.clone();
        let s_ok = store.clone();
        let desc = if is_connected {
            format!("{name} is running. Deleting it stops sing-box first.")
        } else {
            format!("Delete {name}.json and any saved source URL?")
        };
        alert
            .confirm()
            .title(format!("Delete {name}"))
            .description(desc)
            .ok_text("Delete")
            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                // `force` is never derived from the cached snapshot: a stale
                // snapshot would delete a running profile without the warning
                // this dialog exists to give. Core decides, and its refusal
                // comes back as a notice.
                s_ok.update(cx, |s, cx| {
                    s.delete_profile(n_ok.clone(), false, origin, cx);
                });
                true
            })
    });
}
