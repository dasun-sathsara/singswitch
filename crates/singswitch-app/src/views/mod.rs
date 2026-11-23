pub mod editor;
pub mod logs;
pub mod profiles;
pub mod settings;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::actions::NavTab;
use crate::fonts;
use crate::icons::TrayVisualState;
use crate::shell::set_dock_visible;
use crate::state::{AppStore, BannerTone, NoticeOrigin};
use crate::theme::ThemePreference;
use crate::views::logs::LogsView;
use crate::views::profiles::ProfilesView;
use crate::views::settings::SettingsView;

pub struct WorkspaceShell {
    store: Entity<AppStore>,
    profiles: Entity<ProfilesView>,
    logs: Entity<LogsView>,
    settings: Entity<SettingsView>,
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
                // Nothing to reopen it from, and the app is QuitMode::Explicit,
                // so the close button has to mean quit here.
                crate::diag::log("close with no tray: quitting");
                cx.dispatch_action(&crate::actions::QuitApp);
                return false;
            }
            let b = window.bounds().size;
            store_close.update(cx, |s, cx| {
                // Closing is a final write point: persist here, or the size is lost.
                s.update_window_size(b.width.as_f32(), b.height.as_f32(), true);
                s.set_window_open(false, cx);
            });
            set_dock_visible(false);
            // Same release as Cmd-W: without it the closed window's shell, its
            // editor rope and both log buffers stay alive in the tray. Unsaved
            // buffers survive in the store, stashed by forget_main_window.
            crate::desktop::forget_main_window(cx);
            true
        });

        let profiles = cx.new(|cx| ProfilesView::new(store.clone(), window, cx));
        let logs = cx.new(|cx| LogsView::new(store.clone(), window, cx));
        let settings = cx.new(|_| SettingsView::new(store.clone()));

        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];

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
            _subs: subs,
        }
    }

    /// Stash the editor's unsaved buffer before the window is released.
    pub fn stash_editor_draft(&mut self, cx: &mut Context<Self>) {
        self.profiles.update(cx, |p, cx| p.stash_draft(cx));
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

    fn render_navigation(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let (errors, _) = s.log_counts();
        let selected = s.selected_tab;
        let tabs = [
            ("Connections", NavTab::Profiles, IconName::Waypoints),
            ("Logs", NavTab::Logs, IconName::ScrollText),
            ("Settings", NavTab::Settings, IconName::Settings),
        ];

        h_flex()
            .w_full()
            .flex_none()
            .h_full()
            .pr(px(12.0))
            .gap(px(4.0))
            .children(tabs.into_iter().map(|(label, tab, icon)| {
                let store = self.store.clone();
                let label = if tab == NavTab::Logs && errors > 0 {
                    format!("Logs ({errors})")
                } else {
                    label.to_owned()
                };
                Button::new(SharedString::from(format!("nav-{tab:?}")))
                    .when(selected == tab, |button| button.secondary())
                    .when(selected != tab, |button| button.ghost())
                    .toggled(selected == tab)
                    .small()
                    .icon(icon)
                    .label(label)
                    .on_click(move |_, _, cx| {
                        store.update(cx, |s, cx| s.set_tab(tab, cx));
                    })
            }))
            .child(div().flex_1())
            .when(selected != NavTab::Profiles || s.editing, |nav| {
                nav.child(connection_indicator(s, cx))
            })
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
            .child(if cfg!(target_os = "macos") {
                TitleBar::new().child(self.render_navigation(cx))
            } else {
                TitleBar::new().child(div().text_sm().child("SingSwitch"))
            })
            .when(cfg!(target_os = "windows"), |shell| {
                // Windows reserves the title bar for native caption hit testing.
                shell.child(
                    div()
                        .h(px(34.0))
                        .pl(px(12.0))
                        .child(self.render_navigation(cx)),
                )
            })
            .when_some(
                notice_strip(&self.store, NoticeOrigin::Toolbar, cx),
                |el, notice| el.child(div().w_full().px(px(14.0)).py(px(4.0)).child(notice)),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.render_stage(cx)),
            )
    }
}

fn connection_indicator(store: &AppStore, cx: &gpui_kit::App) -> impl IntoElement {
    let label = store.status_label();
    let color = match store.tray_visual_state() {
        TrayVisualState::Transitioning => cx.theme().warning,
        TrayVisualState::Broken => cx.theme().danger,
        TrayVisualState::Connected => cx.theme().success,
        TrayVisualState::Disconnected => cx.theme().muted_foreground,
    };

    h_flex()
        .gap(px(8.0))
        .items_center()
        .min_w(px(0.0))
        .child(div().size(px(7.0)).flex_none().rounded_full().bg(color))
        .child(div().text_sm().truncate().child(label))
        .when_some(
            store
                .snapshot
                .active
                .as_ref()
                .filter(|_| store.snapshot.running),
            |row, name| {
                row.child(
                    div()
                        .max_w(px(160.0))
                        .truncate()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(name.clone()),
                )
            },
        )
}

/// Inline feedback for the screen or control that initiated an operation.
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
            .w_full()
            .rounded(px(8.0))
            .px(px(12.0))
            .py(px(4.0))
            .bg(color.opacity(0.09))
            .child(
                div()
                    .id(SharedString::from(format!("notice-{origin:?}")))
                    .flex_1()
                    .min_w(px(0.0))
                    .max_h(px(62.0))
                    .overflow_y_scroll()
                    .text_sm()
                    .text_color(color)
                    .child(notice.message),
            )
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
