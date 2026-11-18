use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use sbctl_core::layout::Layout;

use crate::actions::NavTab;
use crate::state::{AppStore, BannerTone};
use crate::theme::{ThemePreference, surface_card_bg};

pub struct SettingsView {
    store: Entity<AppStore>,
}

impl SettingsView {
    pub fn new(store: Entity<AppStore>) -> Self {
        Self { store }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let settings = s.settings.clone();
        let layout = s.layout();

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_5()
            .child(render_appearance_card(settings.theme, &self.store, cx))
            .child(render_behavior_card(&settings, &self.store, cx))
            .child(render_paths_card(&layout, &self.store, cx))
    }
}

fn render_appearance_card(
    current: ThemePreference,
    store: &Entity<AppStore>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .p_5()
        .gap_4()
        .rounded(cx.theme().radius_lg)
        .bg(surface_card_bg(cx))
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_base()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child("Appearance"),
        )
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child("Theme"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Light, dark, or system theme."),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(theme_button(
                            "theme-light",
                            "Light",
                            Some(IconName::Sun),
                            ThemePreference::Light,
                            current,
                            store,
                        ))
                        .child(theme_button(
                            "theme-dark",
                            "Dark",
                            Some(IconName::Moon),
                            ThemePreference::Dark,
                            current,
                            store,
                        ))
                        .child(theme_button(
                            "theme-system",
                            "System",
                            None,
                            ThemePreference::System,
                            current,
                            store,
                        )),
                ),
        )
}

fn theme_button(
    id: &'static str,
    label: &'static str,
    icon: Option<IconName>,
    target: ThemePreference,
    current: ThemePreference,
    store: &Entity<AppStore>,
) -> Button {
    let st = store.clone();
    Button::new(id)
        .small()
        .when_some(icon, Button::icon)
        .when(current == target, Button::primary)
        .when(current != target, Button::outline)
        .label(label)
        .on_click(move |_, _, cx| {
            st.update(cx, |s, cx| s.set_theme_preference(target, cx));
        })
}

fn render_behavior_card(
    settings: &crate::theme::UiSettings,
    store: &Entity<AppStore>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    let store_login = store.clone();
    let store_menubar = store.clone();
    let store_ip = store.clone();
    let store_notify = store.clone();
    let store_guide = store.clone();

    v_flex()
        .w_full()
        .p_5()
        .gap_4()
        .rounded(cx.theme().radius_lg)
        .bg(surface_card_bg(cx))
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_base()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child("Desktop and tray"),
        )
        .child(setting_switch_row(
            "set-launch-login",
            "Launch at login",
            "Start sbctl in the tray when you sign in.",
            settings.launch_at_login,
            false,
            move |checked, _, cx| {
                store_login.update(cx, |s, cx| s.toggle_launch_at_login(*checked, cx));
            },
            cx,
        ))
        .child(setting_switch_row(
            "set-menubar-title",
            "Profile name in menu bar",
            "Show the active profile next to the macOS menu bar icon.",
            settings.show_profile_in_menubar,
            !cfg!(target_os = "macos"),
            move |checked, _, cx| {
                store_menubar.update(cx, |s, cx| s.set_show_profile_in_menubar(*checked, cx));
            },
            cx,
        ))
        .child(setting_switch_row(
            "set-auto-ip",
            "Refresh IP on switch",
            "Fetch public IP from ipinfo.io after connecting or disconnecting.",
            settings.auto_refresh_ip,
            false,
            move |checked, _, cx| {
                store_ip.update(cx, |s, cx| s.set_auto_refresh_ip(*checked, cx));
            },
            cx,
        ))
        .child(setting_switch_row(
            "set-notifications",
            "Notifications",
            "Show system alerts when profiles switch or fail.",
            settings.desktop_notifications,
            false,
            move |checked, _, cx| {
                store_notify.update(cx, |s, cx| s.set_desktop_notifications(*checked, cx));
            },
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child("Setup guide"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Show the first-run setup card on the dashboard."),
                        ),
                )
                .child(
                    Button::new("set-show-onboarding")
                        .outline()
                        .small()
                        .label("Show Guide")
                        .on_click(move |_, _, cx| {
                            store_guide.update(cx, |s, cx| {
                                s.set_onboarding_dismissed(false, cx);
                                s.set_tab(NavTab::Dashboard, cx);
                            });
                        }),
                ),
        )
}

fn setting_switch_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    checked: bool,
    disabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut gpui_kit::App) + 'static,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
        .child(
            Switch::new(id)
                .checked(checked)
                .disabled(disabled)
                .on_change(on_change),
        )
}

fn render_paths_card(
    layout: &Layout,
    store: &Entity<AppStore>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    let mono_font = cx.theme().mono_font_family.clone();
    let paths = [
        ("Platform", layout.platform.as_str().to_string()),
        (
            "Profiles directory",
            layout.profiles_dir.display().to_string(),
        ),
        (
            "Active configuration",
            layout.active_config_path.display().to_string(),
        ),
        ("Error log", layout.error_log_path.display().to_string()),
        ("Access log", layout.access_log_path.display().to_string()),
        (
            "Service target",
            if layout.launchd_label.is_empty() {
                format!("{} ({})", layout.service_name, layout.pipe_name)
            } else {
                layout.launchd_label.clone()
            },
        ),
    ];

    v_flex()
        .w_full()
        .p_5()
        .gap_3()
        .rounded(cx.theme().radius_lg)
        .bg(surface_card_bg(cx))
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_base()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child("System paths"),
        )
        .children(paths.into_iter().enumerate().map(|(ix, (label, value))| {
            let val_copy = value.clone();
            let store_copy = store.clone();
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(160.0))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .font_family(mono_font.clone())
                        .text_color(cx.theme().foreground)
                        .truncate()
                        .child(value),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-path-{ix}")))
                        .ghost()
                        .small()
                        .icon(IconName::Copy)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(val_copy.clone()));
                            store_copy.update(cx, |s, cx| {
                                s.set_banner(BannerTone::Success, "Copied path", cx);
                            });
                        }),
                )
        }))
}
