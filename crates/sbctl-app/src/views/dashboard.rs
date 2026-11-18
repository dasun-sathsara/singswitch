use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use sbctl_core::service::RunState;

use crate::actions::NavTab;
use crate::state::{AppStore, BannerTone};
use crate::theme::{subtle_pill_bg, surface_card_bg};
use crate::views::profiles::ProfilesView;

pub struct DashboardView {
    store: Entity<AppStore>,
}

impl DashboardView {
    pub fn new(store: Entity<AppStore>) -> Self {
        Self { store }
    }
}

impl Render for DashboardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store_snapshot = self.store.read(cx);
        let snap = store_snapshot.snapshot.clone();
        let busy_phase = store_snapshot.busy_phase.clone();
        let ip_info = store_snapshot.ip_info.clone();
        let ip_loading = store_snapshot.ip_loading;
        let ip_error = store_snapshot.ip_error.clone();
        let init_error = store_snapshot.init_error.clone();

        let is_busy = busy_phase.is_some();
        let has_ready = snap.profiles.iter().any(|p| p.ready());
        let has_remote = snap.profiles.iter().any(|p| p.source_url.is_some());

        let (state_label, status_color) = if busy_phase.is_some() {
            ("Transitioning", cx.theme().warning)
        } else if snap.broken.is_some() || snap.health.state == RunState::Error {
            ("Attention needed", cx.theme().danger)
        } else if snap.running {
            ("Connected", cx.theme().success)
        } else {
            ("Disconnected", cx.theme().muted_foreground)
        };

        let store_toggle = self.store.clone();
        let store_sync = self.store.clone();
        let store_ip = self.store.clone();
        let store_copy_ip = self.store.clone();
        let store_doctor = self.store.clone();
        let store_profiles_btn = self.store.clone();

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_5()
            // Hero connection status card.
            .child(
                v_flex()
                    .w_full()
                    .p_5()
                    .gap_4()
                    .rounded(cx.theme().radius_lg)
                    .bg(surface_card_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .size(px(10.0))
                                                    .rounded_full()
                                                    .bg(status_color),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(status_color)
                                                    .child(state_label),
                                            )
                                            .when_some(snap.status_view.tun.clone(), |this, tun| {
                                                this.child(Tag::secondary().small().child(tun))
                                            })
                                            .when_some(snap.status_view.pid, |this, pid| {
                                                this.child(
                                                    Tag::secondary()
                                                        .small()
                                                        .child(format!("PID {pid}")),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(
                                                snap.active
                                                    .clone()
                                                    .unwrap_or_else(|| "No active profile".to_string()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                snap.status_view
                                                    .server
                                                    .clone()
                                                    .unwrap_or_else(|| {
                                                        "Select a ready profile below to connect"
                                                            .to_string()
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .when(has_remote, |this| {
                                        this.child(
                                            Button::new("dash-sync-all")
                                                .outline()
                                                .icon(IconName::RefreshCw)
                                                .label("Sync Remote")
                                                .disabled(is_busy)
                                                .on_click(move |_, _, cx| {
                                                    store_sync.update(cx, |s, cx| s.sync_all(cx));
                                                }),
                                        )
                                    })
                                    .child(
                                        Button::new("dash-toggle")
                                            .when(snap.running, |b| {
                                                b.danger().icon(IconName::Square).label("Disconnect")
                                            })
                                            .when(!snap.running, |b| {
                                                b.primary().icon(IconName::Play).label("Connect")
                                            })
                                            .disabled(
                                                is_busy
                                                    || init_error.is_some()
                                                    || (!snap.running && !has_ready),
                                            )
                                            .on_click(move |_, _, cx| {
                                                store_toggle
                                                    .update(cx, |s, cx| s.toggle_service(cx));
                                            }),
                                    ),
                            ),
                    )
                    .when_some(busy_phase.clone(), |this, phase| {
                        this.child(
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_2()
                                .gap_2()
                                .items_center()
                                .rounded(cx.theme().radius)
                                .bg(subtle_pill_bg(cx))
                                .child(Spinner::new().small())
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().foreground)
                                        .child(phase),
                                ),
                        )
                    })
                    .when_some(snap.broken.clone(), |this, broken| {
                        this.child(
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_2()
                                .justify_between()
                                .gap_3()
                                .items_center()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().warning)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(cx.theme().warning)
                                        .child(broken),
                                )
                                .child(
                                    Button::new("dash-broken-doctor")
                                        .outline()
                                        .small()
                                        .label("Open Doctor")
                                        .on_click(move |_, _, cx| {
                                            store_doctor.update(cx, |s, cx| {
                                                s.set_tab(NavTab::Doctor, cx);
                                                s.run_doctor(cx);
                                            });
                                        }),
                                ),
                        )
                    }),
            )
            // Egress Public IP card.
            .child(
                h_flex()
                    .w_full()
                    .p_4()
                    .justify_between()
                    .items_center()
                    .gap_4()
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
                                    .child(IconName::Globe)
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("PUBLIC EGRESS IP"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(if ip_loading {
                                        "Checking egress IP...".to_string()
                                    } else if let Some(ip) = &ip_info {
                                        ip.ip.clone()
                                    } else if let Some(err) = &ip_error {
                                        format!("Unavailable ({err})")
                                    } else {
                                        "Unknown".to_string()
                                    }),
                            )
                            .when_some(ip_info.clone(), |this, ip| {
                                let loc = ip.formatted_location();
                                let meta = match (loc.is_empty(), ip.network.is_empty()) {
                                    (false, false) => format!("{loc}  |  {}", ip.network),
                                    (false, true) => loc,
                                    (true, false) => ip.network,
                                    (true, true) => ip.timezone,
                                };
                                this.when(!meta.is_empty(), |t| {
                                    t.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(meta),
                                    )
                                })
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when_some(ip_info.as_ref().map(|i| i.ip.clone()), |this, ip_str| {
                                this.child(
                                    Button::new("dash-copy-ip")
                                        .outline()
                                        .small()
                                        .icon(IconName::Copy)
                                        .label("Copy")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                ip_str.clone(),
                                            ));
                                            store_copy_ip.update(cx, |s, cx| {
                                                s.set_banner(
                                                    BannerTone::Success,
                                                    format!("Copied {ip_str} to clipboard"),
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                            })
                            .child(
                                Button::new("dash-refresh-ip")
                                    .outline()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .label("Refresh IP")
                                    .disabled(ip_loading)
                                    .on_click(move |_, _, cx| {
                                        store_ip.update(cx, |s, cx| s.refresh_ip(cx));
                                    }),
                            ),
                    ),
            )
            // Quick Switch profiles section.
            .child(
                v_flex()
                    .w_full()
                    .gap_3()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Profiles"),
                            )
                            .child(
                                Button::new("dash-manage-profiles")
                                    .ghost()
                                    .small()
                                    .label("Open Profiles")
                                    .icon(IconName::ChevronRight)
                                    .on_click(move |_, _, cx| {
                                        store_profiles_btn
                                            .update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
                                    }),
                            ),
                    )
                    .when(snap.profiles.is_empty(), |this| {
                        let store_new = self.store.clone();
                        let store_pull = self.store.clone();
                        this.child(
                            v_flex()
                                .w_full()
                                .p_5()
                                .gap_3()
                                .items_center()
                                .rounded(cx.theme().radius_lg)
                                .bg(surface_card_bg(cx))
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No sing-box profiles found in the profiles directory."),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("dash-create-first")
                                                .primary()
                                                .small()
                                                .icon(IconName::Plus)
                                                .label("New Profile")
                                                .disabled(init_error.is_some())
                                                .on_click(move |_, window, cx| {
                                                    ProfilesView::open_new_profile_dialog(
                                                        store_new.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        )
                                        .child(
                                            Button::new("dash-import-first")
                                                .outline()
                                                .small()
                                                .icon(IconName::ExternalLink)
                                                .label("Import URL")
                                                .disabled(init_error.is_some())
                                                .on_click(move |_, window, cx| {
                                                    ProfilesView::open_import_url_dialog(
                                                        store_pull.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        ),
                                ),
                        )
                    })
                    .when(!snap.profiles.is_empty(), |this| {
                        this.child(div().flex().flex_wrap().gap_3().children(
                            snap.profiles.iter().enumerate().map(|(ix, prof)| {
                                let is_active =
                                    snap.active.as_deref() == Some(prof.name.as_str());
                                let is_connected = is_active && snap.running;
                                let ready = prof.ready();
                                let prof_name = prof.name.clone();
                                let store_card = self.store.clone();
                                let placeholder_count = prof.placeholders.len();

                                div()
                                    .id(SharedString::from(format!("quick-profile-{ix}")))
                                    .w(px(230.0))
                                    .p_4()
                                    .rounded(cx.theme().radius_lg)
                                    .bg(surface_card_bg(cx))
                                    .border_1()
                                    .border_color(if is_connected {
                                        cx.theme().success
                                    } else if is_active {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().border
                                    })
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(cx.theme().primary))
                                    .on_click(move |_, _, cx| {
                                        if is_busy {
                                            return;
                                        }
                                        let target = prof_name.clone();
                                        store_card.update(cx, |s, cx| {
                                            if ready {
                                                s.activate_profile(target, cx);
                                            } else {
                                                s.select_profile(Some(target.clone()), cx);
                                                s.set_tab(NavTab::Profiles, cx);
                                                s.set_banner(
                                                    BannerTone::Info,
                                                    format!(
                                                        "Complete guided setup or edit placeholders for {target} before connecting"
                                                    ),
                                                    cx,
                                                );
                                            }
                                        });
                                    })
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .text_color(cx.theme().foreground)
                                                            .truncate()
                                                            .child(prof.name.clone()),
                                                    )
                                                    .when(is_connected, |t| {
                                                        t.child(
                                                            Tag::success().small().child("Live"),
                                                        )
                                                    })
                                                    .when(is_active && !is_connected, |t| {
                                                        t.child(
                                                            Tag::secondary().small().child("Active"),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .truncate()
                                                    .child(
                                                        prof.server
                                                            .clone()
                                                            .unwrap_or_else(|| {
                                                                "direct / unconfigured".to_string()
                                                            }),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .when(!ready, |t| {
                                                        t.child(
                                                            Tag::warning().small().child(format!(
                                                                "{placeholder_count} TODO"
                                                            )),
                                                        )
                                                    })
                                                    .when(prof.source_url.is_some(), |t| {
                                                        t.child(
                                                            Tag::info().small().child("Remote"),
                                                        )
                                                    }),
                                            ),
                                    )
                            }),
                        ))
                    }),
            )
    }
}
