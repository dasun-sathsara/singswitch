use std::cell::Cell;

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

use crate::fonts;
use crate::state::{AppStore, BannerTone, NoticeOrigin};
use crate::views::editor::ProfileEditorPanel;

pub struct ProfilesView {
    store: Entity<AppStore>,
    editor: Entity<ProfileEditorPanel>,
    _search_input: Entity<InputState>,
    /// Whether the technical detail rows are expanded on the Connections stage.
    details_open: bool,
    _sub: Subscription,
}

impl ProfilesView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| ProfileEditorPanel::new(store.clone(), window, cx));
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter profiles...")
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
            _search_input: search_input,
            details_open: false,
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
                        .child(
                            Input::new(&name_input)
                                .focus_bordered(false)
                                .cleanable(true),
                        ),
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
                                    let res = store_for_ok
                                        .update(cx, |s, cx| s.create_profile(&raw_name, cx));
                                    match res {
                                        Ok(()) => window.close_dialog(cx),
                                        Err(err) => {
                                            store_for_ok.update(cx, |s, cx| {
                                                s.set_notice(
                                                    NoticeOrigin::Stage,
                                                    BannerTone::Error,
                                                    err.to_string(),
                                                    cx,
                                                );
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
                        .child(
                            Input::new(&name_input)
                                .focus_bordered(false)
                                .cleanable(true),
                        )
                        .child(Input::new(&url_input).focus_bordered(false).cleanable(true)),
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
                                .label("Import")
                                .on_click(move |_, window, cx| {
                                    let name = n_ok.read(cx).value().trim().to_string();
                                    let url = u_ok.read(cx).value().trim().to_string();
                                    if name.is_empty() || url.is_empty() {
                                        s_ok.update(cx, |s, cx| {
                                            s.set_notice(
                                                NoticeOrigin::Stage,
                                                BannerTone::Warning,
                                                "Enter a profile name and URL.",
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

    /// The Connections stage: one profile, stated honestly.
    ///
    /// Everything in the provenance block comes from the profile's own JSON.
    /// Service health and the observed public IP are deliberately absent here;
    /// they belong to the machine and live in the toolbar and status strip.
    fn render_stage(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let snap = &s.snapshot;
        let busy = s.busy_phase.is_some();
        let init_err = s.init_error.is_some();

        let setup = s.setup_condition();
        let details_open = self.details_open;

        let Some(prof) = s
            .selected_profile
            .as_ref()
            .and_then(|n| snap.profiles.iter().find(|p| &p.name == n).cloned())
        else {
            return render_setup(&setup, &self.store, cx).into_any_element();
        };

        // Facts are parsed once when the snapshot is built; reading and parsing
        // the profile here meant one file read per drawn frame.
        let facts = prof.facts.clone();
        let parsed_ok = prof.config_parsed;
        let is_live = snap.running && snap.active.as_deref() == Some(prof.name.as_str());
        let ready = prof.ready();
        let name_act = prof.name.clone();
        let name_del = prof.name.clone();
        let name_sync = prof.name.clone();
        let store_act = self.store.clone();
        let store_del = self.store.clone();
        let store_edit = self.store.clone();
        let store_ip = self.store.clone();
        let ip_loading = s.ip_loading;
        let ip_error = s.ip_error.clone();
        let ip_text = if s.ip_loading {
            Some("Checking…".to_string())
        } else {
            s.ip_info.as_ref().map(|i| {
                let loc = i.formatted_location();
                if loc.is_empty() {
                    i.ip.clone()
                } else {
                    format!("{} · {loc}", i.ip)
                }
            })
        };
        let store_dismiss = self.store.clone();
        let stage_notice = s.notice_for(crate::state::NoticeOrigin::Stage).cloned();
        let store_sync = self.store.clone();
        let cfg_path = prof.path.display().to_string();
        let path_for_copy = cfg_path.clone();

        let endpoint = facts.endpoint.clone().or_else(|| prof.server.clone());
        let transport = facts
            .outbound_type
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        v_flex()
            .id("connections-stage")
            .size_full()
            .overflow_y_scroll()
            .p(px(24.0))
            .gap(px(0.0))
            // Head
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(px(16.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(24.0))
                                    .font_bold()
                                    .child(prof.name.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .font_family(fonts::mono_family())
                                            .text_size(px(12.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(endpoint.clone().unwrap_or_else(|| "—".into())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("· {transport}")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if prof.source_url.is_some() {
                                                "· source saved"
                                            } else {
                                                "· no source"
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                Button::new("conn-switch")
                                    .primary()
                                    .small()
                                    .label(if is_live { "Reconnect" } else { "Switch" })
                                    .disabled(busy || init_err || !ready)
                                    .tooltip(if ready {
                                        format!("Switch sing-box to {}", prof.name)
                                    } else {
                                        format!("{} TODO values remaining", prof.placeholders.len())
                                    })
                                    .on_click(move |_, _, cx| {
                                        let n = name_act.clone();
                                        store_act.update(cx, |s, cx| {
                                            s.activate_profile(n, NoticeOrigin::Stage, cx)
                                        });
                                    }),
                            )
                            .child(
                                Button::new("conn-edit")
                                    .outline()
                                    .small()
                                    .label("Edit")
                                    .disabled(busy)
                                    .on_click(move |_, _, cx| {
                                        store_edit.update(cx, |s, cx| s.begin_edit(cx));
                                    }),
                            )
                            .child(self.render_overflow(&prof, is_live, busy, cx))
                            .child(
                                Button::new("conn-delete")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Delete)
                                    .tooltip("Delete profile")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        open_delete_dialog(
                                            name_del.clone(),
                                            is_live,
                                            NoticeOrigin::Stage,
                                            store_del.clone(),
                                            window,
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            )
            .when_some(stage_notice, |el, notice| {
                el.child(
                    h_flex()
                        .mt(px(10.0))
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(match notice.tone {
                                    BannerTone::Error => cx.theme().danger,
                                    BannerTone::Warning => cx.theme().warning,
                                    BannerTone::Success => cx.theme().success,
                                    BannerTone::Info => cx.theme().muted_foreground,
                                })
                                .child(notice.message.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("stage-notice-dismiss")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(move |_, _, cx| {
                                    store_dismiss.update(cx, |s, cx| s.dismiss_notice(cx));
                                }),
                        ),
                )
            })
            // Provenance — one grouped region, configured values only
            .child(
                v_flex()
                    .mt(px(20.0))
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(crate::theme::control_edge(cx))
                    .bg(crate::theme::surface(cx))
                    .child(prov_row(
                        "Outbound",
                        {
                            let f = &facts;
                            match (&f.outbound_type, &f.endpoint) {
                                (Some(kind), Some(ep)) => {
                                    let tag = f
                                        .outbound_tag
                                        .as_deref()
                                        .map(|t| format!(" · {t}"))
                                        .unwrap_or_default();
                                    format!("{kind}{tag} · {ep}")
                                }
                                _ => "none · no outbound declares a server".to_string(),
                            }
                        },
                        parsed_ok,
                        cx,
                    ))
                    .child(prov_row(
                        "Tunnel",
                        {
                            let f = &facts;
                            match f.tun_state() {
                                sbctl_core::profile::TunState::Named => format!(
                                    "{} · from the profile",
                                    f.tun_interface.clone().unwrap_or_default()
                                ),
                                sbctl_core::profile::TunState::Unnamed => {
                                    "OS default · no interface set".to_string()
                                }
                                sbctl_core::profile::TunState::Absent => {
                                    "none · no TUN inbound in this config".to_string()
                                }
                            }
                        },
                        parsed_ok,
                        cx,
                    )),
            )
            // Observed public IP: machine-wide, never presented as the profile's.
            .child(
                h_flex()
                    .w_full()
                    .px(px(14.0))
                    .py(px(11.0))
                    .gap(px(18.0))
                    .items_baseline()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w(px(118.0))
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(cx.theme().muted_foreground)
                            .child("Public IP"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(fonts::mono_family())
                            .text_size(px(12.5))
                            .text_color(if ip_text.is_some() {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(ip_text.clone().unwrap_or_else(|| "Not checked".into())),
                    )
                    .when_some(ip_error, |el, err| {
                        el.child(
                            div()
                                .flex_none()
                                .max_w(px(220.0))
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(cx.theme().danger)
                                .child(err),
                        )
                    })
                    .child(
                        Button::new("conn-refresh-ip")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .label(if ip_loading {
                                "Checking…"
                            } else {
                                "Refresh IP"
                            })
                            .disabled(ip_loading || busy)
                            .on_click(move |_, _, cx| {
                                store_ip.update(cx, |s, cx| s.refresh_ip(cx));
                            }),
                    ),
            )
            // Runtime facts — only while running
            .when(is_live, |el| {
                el.child(
                    h_flex()
                        .py(px(14.0))
                        .gap(px(24.0))
                        .items_center()
                        .children(s.connected_since.map(|since| {
                            let secs = since.elapsed().as_secs();
                            info_pair("Up", format_duration(secs), cx)
                        }))
                        .children(
                            snap.health
                                .pid
                                .map(|pid| info_pair("PID", format!("{pid}"), cx)),
                        )
                        .children(
                            snap.health
                                .restarts
                                .filter(|n| *n > 0)
                                .map(|n| info_pair("Restarts", format!("{n}"), cx)),
                        )
                        .children(
                            snap.health
                                .last_exit
                                .clone()
                                .map(|code| info_pair("Last exit", code, cx)),
                        ),
                )
            })
            // Broken config, verbatim
            .when_some(snap.broken.clone(), |el, reason| {
                el.child(
                    h_flex().gap(px(10.0)).items_center().py(px(10.0)).child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(format!("Config broken: {reason}")),
                    ),
                )
            })
            // Advanced details stay available but collapsed: the daily task is
            // knowing which profile is live and switching, not reading paths.
            .child(
                h_flex()
                    .id("conn-details-toggle")
                    .mt(px(6.0))
                    .py(px(8.0))
                    .gap(px(6.0))
                    .items_center()
                    .cursor_pointer()
                    .child(
                        Icon::new(if details_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(cx.theme().muted_foreground)
                            .child("Advanced details"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.details_open = !this.details_open;
                        cx.notify();
                    })),
            )
            .when(details_open, |el| {
                el.child(
                    v_flex()
                        .mt(px(4.0))
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(fact_row("Config", path_for_copy.clone(), true, cx))
                        .when_some(prof.source_url.clone(), |el, url| {
                            let n = name_sync.clone();
                            let st = store_sync.clone();
                            el.child(
                                h_flex()
                                    .min_h(px(32.0))
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .gap(px(16.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(132.0))
                                            .flex_none()
                                            .text_size(px(12.0))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Source"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .font_family(fonts::mono_family())
                                            .text_size(px(12.5))
                                            .child(url),
                                    )
                                    .child(
                                        Button::new("conn-sync")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::RefreshCw)
                                            .tooltip("Sync from source")
                                            .disabled(busy)
                                            .on_click(move |_, _, cx| {
                                                let n = n.clone();
                                                st.update(cx, |s, cx| s.sync_profile(n, cx));
                                            }),
                                    ),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

impl ProfilesView {
    /// Operations that used to live on the Dashboard. They stay reachable from
    /// the stage so nothing the old app could do became hidden.
    fn render_overflow(
        &self,
        prof: &sbctl_core::profile::Profile,
        is_live: bool,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

        let name = prof.name.clone();
        let has_source = prof.source_url.is_some();
        let store_restart = self.store.clone();
        let store_refresh = self.store.clone();
        let store_sync = self.store.clone();
        let store_sync_all = self.store.clone();
        let store_ip = self.store.clone();
        let cfg_path = prof.path.display().to_string();
        let public_ip =
            std::rc::Rc::new(self.store.read(cx).ip_info.as_ref().map(|i| i.ip.clone()));
        Button::new("conn-overflow")
            .ghost()
            .small()
            .icon(IconName::Ellipsis)
            .tooltip("More actions")
            .dropdown_menu(move |menu, _, _| {
                let n = name.clone();
                let r = store_restart.clone();
                let rf = store_refresh.clone();
                let sy = store_sync.clone();
                let sa = store_sync_all.clone();
                let ip = store_ip.clone();
                let path = cfg_path.clone();
                let original_ip = public_ip.clone();
                menu.item(
                    PopupMenuItem::new("Restart service")
                        .disabled(busy || !is_live)
                        .on_click(move |_, _, cx| {
                            r.update(cx, |s, cx| s.restart_active(cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Refresh status").on_click(move |_, _, cx| {
                        rf.update(cx, |s, cx| {
                            s.refresh_snapshot(true, cx);
                            s.refresh_ip(cx);
                        });
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Sync from source")
                        .disabled(busy || !has_source)
                        .on_click(move |_, _, cx| {
                            let n = n.clone();
                            sy.update(cx, |s, cx| s.sync_profile(n, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Sync all profiles")
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            sa.update(cx, AppStore::sync_all);
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Copy public IP")
                        .disabled(original_ip.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(value) = (*original_ip).clone() {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value));
                                ip.update(cx, |s, cx| {
                                    s.set_notice(
                                        NoticeOrigin::Stage,
                                        BannerTone::Success,
                                        "Public IP copied",
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy config path").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
                    }),
                )
            })
    }
}

/// The setup region. Shown whenever a condition holds, independent of the old
/// `onboarding_dismissed` flag: dismissal can hide welcome copy, never a failure.
fn render_setup(
    condition: &crate::state::SetupCondition,
    store: &Entity<AppStore>,
    cx: &Context<ProfilesView>,
) -> impl IntoElement {
    use crate::state::SetupCondition as C;

    let (title, body): (String, String) = match condition {
        C::None => (
            "Select a profile".into(),
            "Pick a profile on the left to inspect it.".into(),
        ),
        C::InitFailed(err) => ("sbctl cannot reach the service".into(), err.clone()),
        C::Broken(reason) => ("The active configuration is broken".into(), reason.clone()),
        C::EmptyLibrary => (
            "No profiles yet".into(),
            "Add one, or import a sing-box config from a URL.".into(),
        ),
        C::Unfinished(names) => (
            "Every profile still has TODO values".into(),
            format!("Finish {} before connecting.", names.join(", ")),
        ),
    };

    let store_new = store.clone();
    let store_import = store.clone();
    let store_edit = store.clone();

    v_flex()
        .size_full()
        .p(px(24.0))
        .gap(px(14.0))
        .child(div().text_size(px(20.0)).font_bold().child(title))
        .child(
            div()
                .max_w(px(520.0))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(body),
        )
        .child(
            h_flex()
                .gap(px(8.0))
                .children(
                    matches!(condition, C::EmptyLibrary | C::Unfinished(_)).then(|| {
                        Button::new("setup-new")
                            .primary()
                            .small()
                            .label("New profile")
                            .on_click(move |_, window, cx| {
                                ProfilesView::open_new_profile_dialog(
                                    store_new.clone(),
                                    window,
                                    cx,
                                );
                            })
                    }),
                )
                .children(matches!(condition, C::EmptyLibrary).then(|| {
                    Button::new("setup-import")
                        .outline()
                        .small()
                        .label("Import from URL")
                        .on_click(move |_, window, cx| {
                            ProfilesView::open_import_url_dialog(store_import.clone(), window, cx);
                        })
                }))
                .children(
                    // A broken config is fixed where it is edited, not in a
                    // diagnostics screen.
                    matches!(condition, C::Broken(_)).then(|| {
                        Button::new("setup-fix-config")
                            .primary()
                            .small()
                            .label("Fix configuration")
                            .on_click(move |_, _, cx| {
                                store_edit.update(cx, |s, cx| s.begin_edit(cx));
                            })
                    }),
                ),
        )
}

fn prov_row(
    label: &'static str,
    value: String,
    known: bool,
    cx: &Context<ProfilesView>,
) -> impl IntoElement {
    h_flex()
        .px(px(14.0))
        .py(px(11.0))
        .gap(px(18.0))
        .items_baseline()
        .child(
            div()
                .w(px(118.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(fonts::mono_family())
                .text_size(px(12.5))
                .text_color(if known {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .child(value),
        )
}

fn fact_row(
    label: &'static str,
    value: String,
    mono: bool,
    cx: &Context<ProfilesView>,
) -> impl IntoElement {
    h_flex()
        .min_h(px(30.0))
        .gap(px(16.0))
        .items_baseline()
        .child(
            div()
                .w(px(128.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .when(mono, |el| el.font_family(fonts::mono_family()))
                .text_size(px(12.5))
                .child(value),
        )
}

fn info_pair(label: &'static str, value: String, cx: &Context<ProfilesView>) -> impl IntoElement {
    h_flex()
        .gap(px(8.0))
        .items_baseline()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .font_family(fonts::mono_family())
                .text_size(px(12.0))
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
                .p(px(16.0))
                .child(self.editor.clone())
                .into_any_element()
        } else {
            self.render_stage(cx).into_any_element()
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
