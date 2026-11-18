pub mod dashboard;
pub mod doctor;
pub mod editor;
pub mod logs;
pub mod profiles;
pub mod settings;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarCollapsible, SidebarFooter, SidebarGroup, SidebarMenu, SidebarMenuItem,
    SidebarToggleButton,
};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar, h_flex, v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use sbctl_core::service::RunState;

use crate::actions::NavTab;
use crate::shell::set_dock_visible;
use crate::state::{AppStore, BannerTone};
use crate::theme::{ThemePreference, apply_theme};
use crate::views::dashboard::DashboardView;
use crate::views::doctor::DoctorView;
use crate::views::logs::LogsView;
use crate::views::profiles::ProfilesView;
use crate::views::settings::SettingsView;

pub struct WorkspaceShell {
    store: Entity<AppStore>,
    dashboard: Entity<DashboardView>,
    profiles: Entity<ProfilesView>,
    logs: Entity<LogsView>,
    doctor: Entity<DoctorView>,
    settings: Entity<SettingsView>,
    _subs: Vec<Subscription>,
}

impl WorkspaceShell {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        store.update(cx, |s, cx| s.set_window_open(true, cx));

        let store_close = store.clone();
        // Keep background tray/menu-bar alive when the user closes the window.
        window.on_window_should_close(cx, move |window, cx| {
            let b = window.bounds().size;
            store_close.update(cx, |s, cx| {
                s.update_window_size(b.width.as_f32(), b.height.as_f32());
                s.set_window_open(false, cx);
            });
            set_dock_visible(false);
            true
        });

        let dashboard = cx.new(|_| DashboardView::new(store.clone()));
        let profiles = cx.new(|cx| ProfilesView::new(store.clone(), window, cx));
        let logs = cx.new(|cx| LogsView::new(store.clone(), window, cx));
        let doctor = cx.new(|_| DoctorView::new(store.clone()));
        let settings = cx.new(|_| SettingsView::new(store.clone()));

        let mut subs = Vec::new();
        subs.push(cx.observe(&store, |_, _, cx| cx.notify()));

        let store_appearance = store.clone();
        subs.push(window.observe_window_appearance(move |window, cx| {
            if store_appearance.read(cx).settings.theme == ThemePreference::System {
                apply_theme(ThemePreference::System, Some(window), cx);
            }
        }));

        let store_bounds = store.clone();
        subs.push(cx.observe_window_bounds(window, move |_, window, cx| {
            let b = window.bounds().size;
            store_bounds.update(cx, |s, _| {
                s.update_window_size(b.width.as_f32(), b.height.as_f32());
            });
        }));

        Self {
            store,
            dashboard,
            profiles,
            logs,
            doctor,
            settings,
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
}

impl Render for WorkspaceShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let tab = s.selected_tab;
        let collapsed = s.settings.sidebar_collapsed;
        let theme_pref = s.settings.theme;
        let banner = s.banner.clone();
        let snap = s.snapshot.clone();
        let is_busy = s.busy_phase.is_some();

        let (dot_color, status_text) = if let Some(phase) = &s.busy_phase {
            (cx.theme().warning, phase.clone())
        } else if snap.broken.is_some() || snap.health.state == RunState::Error {
            (cx.theme().danger, "Attention needed".to_string())
        } else if snap.running {
            let name = snap.active.as_deref().unwrap_or("active");
            (cx.theme().success, format!("Connected: {name}"))
        } else {
            (cx.theme().muted_foreground, "Disconnected".to_string())
        };

        let store_sidebar = self.store.clone();
        let store_theme = self.store.clone();
        let store_quick_toggle = self.store.clone();
        let store_banner = self.store.clone();

        let is_dark_now = cx.theme().is_dark();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Custom title bar.
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .pr_2()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    SidebarToggleButton::new()
                                        .collapsed(collapsed)
                                        .accessibility_label("Toggle sidebar")
                                        .on_click(move |_, _, cx| {
                                            store_sidebar.update(cx, |s, cx| {
                                                s.set_sidebar_collapsed(!collapsed, cx);
                                            });
                                        }),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child("sbctl"),
                                )
                                .child(
                                    h_flex()
                                        .px_2()
                                        .py_1()
                                        .gap_2()
                                        .items_center()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().muted)
                                        .child(div().size(px(8.0)).rounded_full().bg(dot_color))
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(cx.theme().foreground)
                                                .child(status_text),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("title-theme-toggle")
                                        .ghost()
                                        .small()
                                        .icon(if is_dark_now {
                                            IconName::Sun
                                        } else {
                                            IconName::Moon
                                        })
                                        .on_click(move |_, _, cx| {
                                            let next = if theme_pref == ThemePreference::Dark
                                                || is_dark_now
                                            {
                                                ThemePreference::Light
                                            } else {
                                                ThemePreference::Dark
                                            };
                                            store_theme.update(cx, |s, cx| {
                                                s.set_theme_preference(next, cx);
                                            });
                                        }),
                                )
                                .child(
                                    Button::new("title-toggle-conn")
                                        .small()
                                        .when(snap.running, |b| b.danger().label("Disconnect"))
                                        .when(!snap.running, |b| b.primary().label("Connect"))
                                        .disabled(
                                            is_busy
                                                || s.init_error.is_some()
                                                || (!snap.running
                                                    && !snap.profiles.iter().any(|p| p.ready())),
                                        )
                                        .on_click(move |_, _, cx| {
                                            store_quick_toggle
                                                .update(cx, |s, cx| s.toggle_service(cx));
                                        }),
                                ),
                        ),
                ),
            )
            // Body: Sidebar + Main Content Area.
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(
                        Sidebar::new("app-sidebar")
                            .collapsible(SidebarCollapsible::Icon)
                            .collapsed(collapsed)
                            .child(
                                SidebarGroup::new("Workspace").child(
                                    SidebarMenu::new()
                                        .child(nav_item(
                                            "Dashboard",
                                            IconName::LayoutDashboard,
                                            NavTab::Dashboard,
                                            tab,
                                            &self.store,
                                        ))
                                        .child(nav_item(
                                            "Profiles",
                                            IconName::FileText,
                                            NavTab::Profiles,
                                            tab,
                                            &self.store,
                                        ))
                                        .child(nav_item(
                                            "Logs",
                                            IconName::SquareTerminal,
                                            NavTab::Logs,
                                            tab,
                                            &self.store,
                                        ))
                                        .child(nav_item(
                                            "Doctor",
                                            IconName::CircleCheck,
                                            NavTab::Doctor,
                                            tab,
                                            &self.store,
                                        ))
                                        .child(nav_item(
                                            "Settings",
                                            IconName::Settings,
                                            NavTab::Settings,
                                            tab,
                                            &self.store,
                                        )),
                                ),
                            )
                            .footer(
                                SidebarFooter::new().child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().size(px(8.0)).rounded_full().bg(dot_color))
                                        .when(!collapsed, |f| {
                                            f.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "sing-box {}",
                                                        snap.health.state.as_str()
                                                    )),
                                            )
                                        }),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_w(px(0.0))
                            .when_some(banner, |this, (tone, msg)| {
                                let sb = store_banner.clone();
                                let alert = match tone {
                                    BannerTone::Info => Alert::info("global-alert", msg),
                                    BannerTone::Success => Alert::success("global-alert", msg),
                                    BannerTone::Warning => Alert::warning("global-alert", msg),
                                    BannerTone::Error => Alert::error("global-alert", msg),
                                };
                                this.child(div().px_6().pt_4().child(alert.small().on_close(
                                    move |_, _, cx| {
                                        sb.update(cx, |s, cx| s.dismiss_banner(cx));
                                    },
                                )))
                            })
                            .child(div().flex_1().w_full().min_h(px(0.0)).child(match tab {
                                NavTab::Dashboard => self.dashboard.clone().into_any_element(),
                                NavTab::Profiles => self.profiles.clone().into_any_element(),
                                NavTab::Logs => self.logs.clone().into_any_element(),
                                NavTab::Doctor => self.doctor.clone().into_any_element(),
                                NavTab::Settings => self.settings.clone().into_any_element(),
                            })),
                    ),
            )
    }
}

fn nav_item(
    label: &'static str,
    icon: IconName,
    target: NavTab,
    current: NavTab,
    store: &Entity<AppStore>,
) -> SidebarMenuItem {
    let st = store.clone();
    SidebarMenuItem::new(label)
        .icon(icon)
        .active(current == target)
        .on_click(move |_, _, cx| {
            st.update(cx, |s, cx| s.set_tab(target, cx));
        })
}
