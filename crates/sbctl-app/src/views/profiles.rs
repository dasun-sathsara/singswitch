use std::cell::Cell;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use sbctl_core::logs::contains_ignore_ascii_case;

use crate::state::{AppStore, BannerTone};
use crate::theme::{subtle_pill_bg, surface_card_bg};
use crate::views::editor::ProfileEditorPanel;

pub struct ProfilesView {
    store: Entity<AppStore>,
    editor: Entity<ProfileEditorPanel>,
    search_input: Entity<InputState>,
    _sub: Subscription,
}

impl ProfilesView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| ProfileEditorPanel::new(store.clone(), window, cx));
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search name or server...")
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

        Self {
            store,
            editor,
            search_input,
            _sub: sub,
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

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let input_for_ok = name_input.clone();
            let store_for_ok = store.clone();

            dialog
                .title("New Profile")
                .width(px(420.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Create a new sing-box profile from the embedded VLESS+Reality skeleton."),
                        )
                        .child(Input::new(&name_input).cleanable(true)),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("new-prof-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-prof-create")
                                .primary()
                                .label("Create")
                                .on_click(move |_, window, cx| {
                                    let raw_name = input_for_ok.read(cx).value().trim().to_string();
                                    let res = store_for_ok.update(cx, |s, cx| {
                                        s.create_profile(raw_name, cx)
                                    });
                                    match res {
                                        Ok(()) => window.close_dialog(cx),
                                        Err(msg) => {
                                            store_for_ok.update(cx, |s, cx| {
                                                s.set_banner(BannerTone::Error, msg, cx);
                                            });
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

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let n_ok = name_input.clone();
            let u_ok = url_input.clone();
            let s_ok = store.clone();

            dialog
                .title("Import Remote Profile")
                .width(px(480.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Download raw sing-box JSON from an HTTP(S) URL and validate before saving."),
                        )
                        .child(Input::new(&name_input).cleanable(true))
                        .child(Input::new(&url_input).cleanable(true)),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("import-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("import-submit")
                                .primary()
                                .label("Download & Import")
                                .on_click(move |_, window, cx| {
                                    let name = n_ok.read(cx).value().trim().to_string();
                                    let url = u_ok.read(cx).value().trim().to_string();
                                    if name.is_empty() || url.is_empty() {
                                        s_ok.update(cx, |s, cx| {
                                            s.set_banner(
                                                BannerTone::Warning,
                                                "Enter both a profile name and URL",
                                                cx,
                                            );
                                        });
                                        return;
                                    }
                                    s_ok.update(cx, |s, cx| s.pull_profile(name, url, cx));
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let snap = s.snapshot.clone();
        let selected_name = s.selected_profile.clone();
        let search = s.profile_search_query.trim();
        let is_busy = s.busy_phase.is_some();
        let init_error = s.init_error.clone();

        let filtered: Vec<_> = snap
            .profiles
            .iter()
            .filter(|p| {
                search.is_empty()
                    || contains_ignore_ascii_case(&p.name, search)
                    || p.server
                        .as_deref()
                        .is_some_and(|srv| contains_ignore_ascii_case(srv, search))
            })
            .cloned()
            .collect();

        let selected_profile = selected_name
            .as_ref()
            .and_then(|n| snap.profiles.iter().find(|p| &p.name == n).cloned());

        let store_new = self.store.clone();
        let store_pull = self.store.clone();

        h_flex()
            .size_full()
            .p_5()
            .gap_4()
            // Left column: Search + profile list.
            .child(
                v_flex()
                    .w(px(290.0))
                    .h_full()
                    .p_3()
                    .gap_3()
                    .rounded(cx.theme().radius_lg)
                    .bg(surface_card_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                Button::new("prof-add-btn")
                                    .primary()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("New")
                                    .disabled(is_busy || init_error.is_some())
                                    .on_click(move |_, window, cx| {
                                        Self::open_new_profile_dialog(
                                            store_new.clone(),
                                            window,
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                Button::new("prof-pull-btn")
                                    .outline()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label("Import URL")
                                    .disabled(is_busy || init_error.is_some())
                                    .on_click(move |_, window, cx| {
                                        Self::open_import_url_dialog(
                                            store_pull.clone(),
                                            window,
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(Input::new(&self.search_input).cleanable(true).small())
                    .child(
                        v_flex()
                            .flex_1()
                            .overflow_y_scrollbar()
                            .gap_1()
                            .when(filtered.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p_3()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matching profiles"),
                                )
                            })
                            .children(filtered.into_iter().enumerate().map(|(ix, prof)| {
                                let is_sel = selected_name.as_deref() == Some(prof.name.as_str());
                                let is_active =
                                    snap.active.as_deref() == Some(prof.name.as_str());
                                let is_live = is_active && snap.running;
                                let store_sel = self.store.clone();
                                let name = prof.name.clone();

                                div()
                                    .id(SharedString::from(format!("prof-row-{ix}")))
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .rounded(cx.theme().radius)
                                    .cursor_pointer()
                                    .when(is_sel, |d| d.bg(subtle_pill_bg(cx)))
                                    .border_1()
                                    .border_color(if is_sel {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().transparent
                                    })
                                    .hover(|d| d.bg(subtle_pill_bg(cx)))
                                    .on_click(move |_, _, cx| {
                                        let target = name.clone();
                                        store_sel.update(cx, |s, cx| {
                                            s.select_profile(Some(target), cx);
                                        });
                                    })
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .text_color(cx.theme().foreground)
                                                            .truncate()
                                                            .child(prof.name),
                                                    )
                                                    .when(is_live, |t| {
                                                        t.child(Tag::success().xsmall().child("Live"))
                                                    })
                                                    .when(is_active && !is_live, |t| {
                                                        t.child(
                                                            Tag::secondary().xsmall().child("Active"),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .truncate()
                                                            .child(
                                                                prof.server.unwrap_or_else(|| {
                                                                    "direct".to_string()
                                                                }),
                                                            ),
                                                    )
                                                    .when(!prof.placeholders.is_empty(), |t| {
                                                        t.child(
                                                            Tag::warning()
                                                                .xsmall()
                                                                .child("Setup"),
                                                        )
                                                    }),
                                            ),
                                    )
                            })),
                    ),
            )
            // Right column: Detail header + editor panel.
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .gap_3()
                    .when_some(selected_profile.clone(), |this, prof| {
                        let is_active = snap.active.as_deref() == Some(prof.name.as_str());
                        let is_connected = is_active && snap.running;
                        let ready = prof.ready();
                        let name_act = prof.name.clone();
                        let name_sync = prof.name.clone();
                        let name_del = prof.name.clone();
                        let store_act = self.store.clone();
                        let store_sync = self.store.clone();
                        let store_del = self.store.clone();

                        this.child(
                            h_flex()
                                .w_full()
                                .p_4()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .rounded(cx.theme().radius_lg)
                                .bg(surface_card_bg(cx))
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_base()
                                                        .font_semibold()
                                                        .text_color(cx.theme().foreground)
                                                        .child(prof.name.clone()),
                                                )
                                                .when(is_connected, |t| {
                                                    t.child(Tag::success().small().child("Connected"))
                                                })
                                                .when(!ready, |t| {
                                                    t.child(
                                                        Tag::warning()
                                                            .small()
                                                            .child("Setup required"),
                                                    )
                                                }),
                                        )
                                        .when_some(prof.source_url.clone(), |t, url| {
                                            t.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .truncate()
                                                    .child(format!("Source: {url}")),
                                            )
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when(prof.source_url.is_some(), |t| {
                                            t.child(
                                                Button::new("prof-sync-one")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::RefreshCw)
                                                    .label("Sync")
                                                    .disabled(is_busy)
                                                    .on_click(move |_, _, cx| {
                                                        let n = name_sync.clone();
                                                        store_sync.update(cx, |s, cx| {
                                                            s.sync_profile(n, cx);
                                                        });
                                                    }),
                                            )
                                        })
                                        .child(
                                            Button::new("prof-activate-btn")
                                                .primary()
                                                .small()
                                                .icon(IconName::Play)
                                                .label(if is_connected {
                                                    "Reconnect"
                                                } else {
                                                    "Activate"
                                                })
                                                .disabled(is_busy || !ready)
                                                .on_click(move |_, _, cx| {
                                                    let n = name_act.clone();
                                                    store_act.update(cx, |s, cx| {
                                                        s.activate_profile(n, cx);
                                                    });
                                                }),
                                        )
                                        .child(
                                            Button::new("prof-delete-btn")
                                                .danger()
                                                .small()
                                                .icon(IconName::Delete)
                                                .label("Delete")
                                                .disabled(is_busy)
                                                .on_click(move |_, window, cx| {
                                                    let n = name_del.clone();
                                                    let sd = store_del.clone();
                                                    window.open_alert_dialog(cx, move |alert, _, _| {
                                                        let n_ok = n.clone();
                                                        let s_ok = sd.clone();
                                                        let desc = if is_connected {
                                                            format!("Profile {n} is currently active and running. Deleting it will stop sing-box first.")
                                                        } else {
                                                            format!("Delete profile {n}? This removes its JSON file and any .url sidecar permanently.")
                                                        };
                                                        alert
                                                            .confirm()
                                                            .title(format!("Delete {n}"))
                                                            .description(desc)
                                                            .ok_text("Delete")
                                                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                                                            .on_ok(move |_, _, cx| {
                                                                s_ok.update(cx, |s, cx| {
                                                                    s.delete_profile(
                                                                        n_ok.clone(),
                                                                        is_connected,
                                                                        cx,
                                                                    );
                                                                });
                                                                true
                                                            })
                                                    });
                                                }),
                                        ),
                                ),
                        )
                    })
                    .child(div().flex_1().w_full().min_h(px(0.0)).child(self.editor.clone())),
            )
    }
}
