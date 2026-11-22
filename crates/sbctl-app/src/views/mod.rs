//! Application shell: native title bar, a toolbar that carries the service
//! headline and the connection control, a rail of profiles that only inspects,
//! one stage, and a status strip that never leaves.
//!
//! The design rule behind the layout: config facts belong to the inspected
//! profile and live in the stage; service health and the observed public IP
//! belong to the machine and live in the toolbar and status strip. They are
//! never mixed, because only one of them can be attributed to a profile.

pub mod editor;
pub mod logs;
pub mod profiles;
pub mod settings;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar,
    h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use sbctl_core::logs::contains_ignore_ascii_case;
use sbctl_core::profile::Profile;

use crate::actions::NavTab;
use crate::fonts;
use crate::shell::set_dock_visible;
use crate::state::{AppStore, BannerTone, NoticeOrigin};
use crate::theme::ThemePreference;
use crate::views::logs::LogsView;
use crate::views::profiles::ProfilesView;
use crate::views::settings::SettingsView;

/// Height of the toolbar row that sits under the native title bar.
const TOOLBAR_HEIGHT: f32 = 44.0;
/// Width of the profile rail.
const RAIL_WIDTH: f32 = 264.0;

pub struct WorkspaceShell {
    store: Entity<AppStore>,
    profiles: Entity<ProfilesView>,
    logs: Entity<LogsView>,
    settings: Entity<SettingsView>,
    filter: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl WorkspaceShell {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        store.update(cx, |s, cx| s.set_window_open(true, cx));

        let store_close = store.clone();
        // Closing hides the app to the menu bar / tray rather than quitting,
        // but only when there is a tray to come back from: with no tray and no
        // Dock icon the window would be unreachable.
        window.on_window_should_close(cx, move |window, cx| {
            let has_tray = store_close.read(cx).tray_available;
            if !has_tray {
                crate::diag::log("close ignored: no tray to restore the window from");
                return false;
            }
            let b = window.bounds().size;
            store_close.update(cx, |s, cx| {
                // Closing is a final write point: persist here, or the size is lost.
                s.update_window_size(b.width.as_f32(), b.height.as_f32(), true);
                s.set_window_open(false, cx);
            });
            set_dock_visible(false);
            true
        });

        let profiles = cx.new(|cx| ProfilesView::new(store.clone(), window, cx));
        let logs = cx.new(|cx| LogsView::new(store.clone(), window, cx));
        let settings = cx.new(|_| SettingsView::new(store.clone()));

        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter")
                .default_value(store.read(cx).profile_search_query.clone())
        });

        let mut subs = Vec::new();
        subs.push(cx.observe(&store, |_, _, cx| cx.notify()));

        let store_filter = store.clone();
        subs.push(
            cx.subscribe_in(&filter, window, move |_, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    store_filter.update(cx, |s, cx| {
                        s.profile_search_query = q;
                        cx.notify();
                    });
                }
            }),
        );

        let store_appearance = store.clone();
        subs.push(window.observe_window_appearance(move |window, cx| {
            if store_appearance.read(cx).settings.theme == ThemePreference::System {
                crate::theme::apply_theme(ThemePreference::System, Some(window), cx);
            }
        }));

        let store_bounds = store.clone();
        subs.push(cx.observe_window_bounds(window, move |_, window, cx| {
            let b = window.bounds().size;
            store_bounds.update(cx, |s, _| {
                s.update_window_size(b.width.as_f32(), b.height.as_f32(), false);
            });
        }));

        Self {
            store,
            profiles,
            logs,
            settings,
            filter,
            _subs: subs,
        }
    }

    pub fn trigger_save_profile(
        &mut self,
        and_activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.read(cx).selected_tab != NavTab::Profiles {
            self.store
                .update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
        }
        self.profiles.update(cx, |p, cx| {
            p.trigger_save(and_activate, window, cx);
        });
    }

    /// The single service headline. State is a dot plus words, never colour alone.
    fn service_headline(&self, cx: &Context<Self>) -> (Hsla, String) {
        let s = self.store.read(cx);
        let snap = &s.snapshot;

        if let Some(phase) = &s.busy_phase {
            return (cx.theme().warning, phase.clone());
        }
        if let Some(broken) = &snap.broken {
            return (cx.theme().danger, format!("Attention needed: {broken}"));
        }
        if snap.health.state == sbctl_core::service::RunState::Error {
            let detail = if snap.health.detail.is_empty() {
                "service failed".to_string()
            } else {
                snap.health.detail.clone()
            };
            return (cx.theme().danger, format!("Attention needed: {detail}"));
        }
        if snap.running {
            let name = snap
                .active
                .clone()
                .unwrap_or_else(|| "unknown profile".into());
            return (cx.theme().success, format!("Connected: {name}"));
        }
        // Name the profile the connect button will actually act on. Using
        // `active` here put a different name next to the button than the one it
        // would activate.
        let target = self.connect_target(cx).0.or_else(|| snap.active.clone());
        match target {
            Some(name) => (
                cx.theme().muted_foreground,
                format!("Stopped · {name} selected"),
            ),
            None => (cx.theme().muted_foreground, "Stopped".to_string()),
        }
    }

    /// Name of the profile the toolbar's connect action would target, and why
    /// it cannot act when it cannot.
    fn connect_target(&self, cx: &Context<Self>) -> (Option<String>, Option<String>) {
        let s = self.store.read(cx);

        if let Some(selected) = s.selected_profile.clone()
            && let Some(p) = s.snapshot.profiles.iter().find(|p| p.name == selected)
        {
            return if p.ready() {
                (Some(p.name.clone()), None)
            } else {
                (
                    None,
                    Some(format!(
                        "{} has {} TODO values",
                        p.name,
                        p.placeholders.len()
                    )),
                )
            };
        }

        if let Some(active) = s.snapshot.active.clone()
            && let Some(p) = s.snapshot.profiles.iter().find(|p| p.name == active)
        {
            return if p.ready() {
                (Some(p.name.clone()), None)
            } else {
                (
                    None,
                    Some(format!(
                        "{} has {} TODO values",
                        p.name,
                        p.placeholders.len()
                    )),
                )
            };
        }

        match s.snapshot.profiles.iter().find(|p| p.ready()) {
            Some(p) => (Some(p.name.clone()), None),
            None => (None, Some("No ready profile. Finish one first.".into())),
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let (dot, headline) = self.service_headline(cx);
        let (target, blocked) = self.connect_target(cx);

        let running = s.snapshot.running;
        let busy = s.busy_phase.is_some();
        let init_err = s.init_error.is_some();
        let tab = s.selected_tab;
        let screen = match tab {
            NavTab::Profiles => "Connections",
            NavTab::Logs => "Logs",
            NavTab::Settings => "Settings",
        };

        let store_toggle = self.store.clone();
        let store_refresh = self.store.clone();
        let store_theme = self.store.clone();
        let store_settab = self.store.clone();
        let store_dismiss = self.store.clone();
        let toolbar_notice = self
            .store
            .read(cx)
            .notice_for(crate::state::NoticeOrigin::Toolbar)
            .cloned();
        let is_dark = cx.theme().is_dark();

        let disconnect_label = if running { "Disconnect" } else { "Connect" };
        let disabled = busy || init_err || (!running && (blocked.is_some() || target.is_none()));

        h_flex()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .w_full()
            .px(px(14.0))
            .gap(px(10.0))
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(div().text_sm().font_bold().child("sbctl"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("·"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(screen),
            )
            .child(div().flex_1())
            // Service headline: dot plus words, the same vocabulary everywhere.
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .mr(px(6.0))
                    .child(div().size(px(7.0)).rounded_full().bg(dot))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(headline),
                    ),
            )
            .when_some(toolbar_notice, |el, notice| {
                el.child(
                    h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .px(px(8.0))
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
                        .child(
                            Button::new("toolbar-notice-dismiss")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .on_click(move |_, _, cx| {
                                    store_dismiss.update(cx, |s, cx| s.dismiss_notice(cx));
                                }),
                        ),
                )
            })
            .child(
                Button::new("toolbar-connection")
                    .small()
                    .when(running, |b| b.outline())
                    .when(!running, |b| b.primary())
                    .label(disconnect_label)
                    .disabled(disabled)
                    .tooltip(match (&blocked, running) {
                        (Some(reason), false) => reason.clone(),
                        _ => disconnect_label.to_string(),
                    })
                    .on_click(move |_, _, cx| {
                        store_toggle.update(cx, AppStore::toggle_service);
                    }),
            )
            .child(
                Button::new("toolbar-refresh")
                    .ghost()
                    .small()
                    .icon(IconName::LoaderCircle)
                    .tooltip("Refresh status and public IP")
                    .on_click(move |_, _, cx| {
                        store_refresh.update(cx, |s, cx| {
                            s.refresh_snapshot(false, cx);
                            s.refresh_ip(cx);
                        });
                    }),
            )
            .child(
                Button::new("toolbar-theme")
                    .ghost()
                    .small()
                    .icon(if is_dark {
                        IconName::Sun
                    } else {
                        IconName::Moon
                    })
                    .tooltip("Switch appearance")
                    .on_click(move |_, _, cx| {
                        // Cycle through all three, so a one-click toggle can no
                        // longer silently overwrite a System preference that
                        // only Settings can restore.
                        let current = store_theme.read(cx).settings.theme;
                        let next = match current {
                            ThemePreference::System => ThemePreference::Light,
                            ThemePreference::Light => ThemePreference::Dark,
                            ThemePreference::Dark => ThemePreference::System,
                        };
                        store_theme.update(cx, |s, cx| s.set_theme_preference(next, cx));
                    }),
            )
            .child(
                Button::new("toolbar-settings")
                    .ghost()
                    .small()
                    .icon(IconName::Settings)
                    .tooltip("Settings")
                    .on_click(move |_, _, cx| {
                        store_settab.update(cx, |s, cx| s.set_tab(NavTab::Settings, cx));
                    }),
            )
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let tab = s.selected_tab;
        let selected = s.selected_profile.clone();
        let active = s.snapshot.active.clone();
        let running = s.snapshot.running;
        let busy = s.busy_phase.is_some();
        let current_phase = s.busy_phase.clone();
        let query = s.profile_search_query.trim().to_string();
        // Borrow, do not clone: this ran per frame and copied every profile,
        // including its placeholder vec and parsed facts.
        let profiles: Vec<&Profile> = s
            .snapshot
            .profiles
            .iter()
            .filter(|p| {
                query.is_empty()
                    || contains_ignore_ascii_case(&p.name, &query)
                    || p.server
                        .as_deref()
                        .is_some_and(|srv| contains_ignore_ascii_case(srv, &query))
            })
            .collect();
        let (log_errors, _) = s.log_counts();

        let store_select = self.store.clone();
        let store_switch = self.store.clone();
        let store_new = self.store.clone();
        let store_import = self.store.clone();
        let store_tab = self.store.clone();

        v_flex()
            .w(px(RAIL_WIDTH))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .px(px(12.0))
                    .pt(px(14.0))
                    .pb(px(8.0))
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_baseline()
                            .child(div().text_sm().font_bold().child("Profiles"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(fonts::mono_family())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{}", profiles.len())),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("rail-new")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("New profile")
                            .on_click(move |_, window, cx| {
                                store_new.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
                                ProfilesView::open_new_profile_dialog(
                                    store_new.clone(),
                                    window,
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new("rail-import")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowDown)
                            .tooltip("Import from URL")
                            .on_click(move |_, window, cx| {
                                store_import.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
                                ProfilesView::open_import_url_dialog(
                                    store_import.clone(),
                                    window,
                                    cx,
                                );
                            }),
                    ),
            )
            .child(
                // 8px of breathing room so the field's focus treatment is never
                // clipped by the rail edge.
                div().px(px(8.0)).pb(px(10.0)).child(
                    Input::new(&self.filter)
                        .small()
                        .cleanable(true)
                        .focus_bordered(false)
                        .prefix(Icon::new(IconName::Search).xsmall()),
                ),
            )
            .child(
                v_flex()
                    .id("rail-profiles")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(8.0))
                    .gap(px(2.0))
                    .children(profiles.into_iter().map(|p| {
                        let is_selected = selected.as_deref() == Some(p.name.as_str());
                        let is_active = active.as_deref() == Some(p.name.as_str());
                        let is_running = is_active && running;
                        let ready = p.ready();
                        let todo = p.placeholders.len();
                        let name = p.name.clone();
                        let sub = p.server.clone().unwrap_or_else(|| "direct".to_string());
                        let st = store_select.clone();
                        let st_switch = store_switch.clone();
                        let switch_name = p.name.clone();
                        // The row's verb is explicit and differs from the
                        // connected row's, so switching never requires a
                        // disconnect first.
                        let verb = if is_running {
                            None
                        } else if running {
                            Some("Switch")
                        } else {
                            Some("Connect")
                        };
                        let busy_here = busy
                            && current_phase
                                .as_deref()
                                .is_some_and(|ph| ph.contains(&p.name));

                        v_flex()
                            .id(SharedString::from(format!("row-{}", p.name)))
                            .h(px(48.0))
                            .px(px(8.0))
                            .gap(px(1.0))
                            .justify_center()
                            .rounded(px(7.0))
                            .cursor_pointer()
                            // Hover must not repaint the selected row: doing so
                            // made the current profile unreadable under the
                            // cursor, because the two washes differ by 1.04:1.
                            .when(is_selected, |el| {
                                el.bg(cx.theme().sidebar_accent).font_bold()
                            })
                            .when(!is_selected, |el| {
                                el.hover(|el| el.bg(crate::theme::hover_wash(cx)))
                            })
                            .on_click(move |_, _, cx| {
                                // Inspecting always lands on Connections, even
                                // when the click came from Logs or Settings.
                                st.update(cx, |s, cx| {
                                    s.select_profile(Some(name.clone()), cx);
                                    s.set_tab(NavTab::Profiles, cx);
                                });
                            })
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .when(is_selected, |el| el.font_bold())
                                            .text_sm()
                                            .text_color(cx.theme().foreground)
                                            .child(p.name.clone()),
                                    )
                                    .when(is_running, |el| {
                                        el.child(
                                            h_flex()
                                                .flex_none()
                                                .gap(px(4.0))
                                                .items_center()
                                                .child(
                                                    div()
                                                        .size(px(6.0))
                                                        .rounded_full()
                                                        .bg(cx.theme().success),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().success)
                                                        .child("Connected"),
                                                ),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .font_family(fonts::mono_family())
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(sub),
                                    )
                                    .when(!ready, |el| {
                                        el.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(cx.theme().warning)
                                                .child(format!("{todo} TODO")),
                                        )
                                    })
                                    .when(is_running, |el| {
                                        el.child(
                                            h_flex()
                                                .flex_none()
                                                .gap(px(4.0))
                                                .items_center()
                                                .child(
                                                    div()
                                                        .size(px(6.0))
                                                        .rounded_full()
                                                        .bg(cx.theme().success),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().success)
                                                        .child("Connected"),
                                                ),
                                        )
                                    })
                                    .when_some(verb, |el, verb| {
                                        let st = st_switch.clone();
                                        let name = switch_name.clone();
                                        el.child(
                                            Button::new(SharedString::from(format!(
                                                "row-switch-{}",
                                                switch_name
                                            )))
                                            .xsmall()
                                            .when(running, |b| b.outline())
                                            .when(!running, |b| b.outline())
                                            .label(verb)
                                            .disabled(busy_here || busy || !ready)
                                            .tooltip(if ready {
                                                format!("{verb} {}", switch_name)
                                            } else {
                                                format!("{switch_name} still has TODO values")
                                            })
                                            .on_click(move |_, _, cx| {
                                                let n = name.clone();
                                                st.update(cx, |s, cx| {
                                                    s.activate_profile(n, NoticeOrigin::Rail, cx)
                                                });
                                            }),
                                        )
                                    }),
                            )
                    })),
            )
            .child(
                v_flex()
                    .px(px(8.0))
                    .py(px(10.0))
                    .gap(px(1.0))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .children(
                        [
                            ("Connections", NavTab::Profiles, 0usize, true),
                            ("Logs", NavTab::Logs, log_errors, false),
                            ("Settings", NavTab::Settings, 0, true),
                        ]
                        .into_iter()
                        .map(|(label, target, count, muted_count)| {
                            let on = tab == target;
                            let st = store_tab.clone();
                            h_flex()
                                .id(SharedString::from(format!("nav-{label}")))
                                .px(px(10.0))
                                .py(px(6.0))
                                .gap(px(8.0))
                                .items_center()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .when(on, |el| el.bg(cx.theme().sidebar_accent))
                                .hover(|el| el.bg(crate::theme::hover_wash(cx)))
                                .on_click(move |_, _, cx| {
                                    st.update(cx, |s, cx| s.set_tab(target, cx));
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .when(on, |el| el.font_bold())
                                        .text_color(if on {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(label),
                                )
                                .child(div().flex_1())
                                .when(count > 0 && !muted_count, |el| {
                                    el.child(
                                        div()
                                            .font_family(fonts::mono_family())
                                            .text_xs()
                                            .text_color(cx.theme().danger)
                                            .child(format!("{count}")),
                                    )
                                })
                        }),
                    ),
            )
    }

    fn render_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.store.read(cx).selected_tab {
            NavTab::Profiles => self.profiles.clone().into_any_element(),
            NavTab::Logs => self.logs.clone().into_any_element(),
            NavTab::Settings => self.settings.clone().into_any_element(),
        }
    }
}

impl Render for WorkspaceShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(fonts::ui_family())
            .text_size(px(15.0))
            .child(TitleBar::new().child(div().w_full()))
            .child(self.render_toolbar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.render_rail(cx))
                    .child(
                        v_flex().flex_1().h_full().min_w(px(0.0)).child(
                            div()
                                .flex_1()
                                .w_full()
                                .min_h(px(0.0))
                                .child(self.render_stage(cx)),
                        ),
                    ),
            )
    }
}

/// Inline feedback for one origin, rendered by the screen that owns it.
///
/// The banner this replaces sat at the top of the window and scrolled out of
/// view; feedback belongs next to the control that produced it.
pub fn notice_strip(
    store: &Entity<AppStore>,
    origin: crate::state::NoticeOrigin,
    cx: &gpui_kit::App,
) -> Option<gpui_kit::AnyElement> {
    use gpui_kit::IntoElement as _;
    let notice = store.read(cx).notice_for(origin)?.clone();
    let color = match notice.tone {
        BannerTone::Error => cx.theme().danger,
        BannerTone::Warning => cx.theme().warning,
        BannerTone::Success => cx.theme().success,
        BannerTone::Info => cx.theme().muted_foreground,
    };
    let store_dismiss = store.clone();
    Some(
        h_flex()
            .gap(px(8.0))
            .items_center()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(color)
                    .child(notice.message),
            )
            .child(div().flex_1())
            .child(
                Button::new(SharedString::from(format!("notice-dismiss-{origin:?}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Dismiss")
                    .on_click(move |_, _, cx| {
                        store_dismiss.update(cx, |s, cx| s.dismiss_notice(cx));
                    }),
            )
            .into_any_element(),
    )
}
