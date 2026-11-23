use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use singswitch_core::layout::Layout;

use crate::fonts;
use crate::state::{AppStore, BannerTone, NoticeOrigin};
use crate::theme::{ThemePreference, surface};

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
        let notice = crate::views::notice_strip(&self.store, NoticeOrigin::Settings, cx);
        let s = self.store.read(cx);
        let settings = s.settings.clone();
        let layout = s.layout();

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .px(px(14.0))
            .py(px(10.0))
            .gap(px(8.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(fonts::EMPHASIS)
                            .child("Settings"),
                    )
                    .child(div().h(px(14.0)).w(px(1.0)).bg(cx.theme().border))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(cx.theme().muted_foreground)
                            .child("Preferences and daemon layout"),
                    ),
            )
            .when_some(notice, |el, notice| el.child(notice))
            .child(render_appearance_card(settings.theme, &self.store, cx))
            .child(render_behavior_card(&settings, &self.store, cx))
            .child(render_paths_card(layout, &self.store, cx))
    }
}

fn render_appearance_card(
    current: ThemePreference,
    store: &Entity<AppStore>,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .p(px(12.0))
        .gap(px(8.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(surface(cx))
        .child(
            div()
                .text_xs()
                .font_weight(fonts::EMPHASIS)
                .text_color(cx.theme().muted_foreground)
                .child("APPEARANCE"),
        )
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap(px(2.0))
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
                                .child("Light, dark, or follow system appearance."),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(2.0))
                        .p(px(2.0))
                        .rounded(px(8.0))
                        .bg(cx.theme().muted.opacity(0.6))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.5))
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
                            Some(IconName::Monitor),
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
        .h(px(26.0))
        .when_some(icon, Button::icon)
        .when(current == target, Button::secondary)
        .when(current != target, Button::ghost)
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
    let store_ip = store.clone();
    let store_notify = store.clone();

    v_flex()
        .w_full()
        .p(px(12.0))
        .gap(px(8.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(surface(cx))
        .child(
            div()
                .text_xs()
                .font_weight(fonts::EMPHASIS)
                .text_color(cx.theme().muted_foreground)
                .child("STARTUP AND NOTIFICATIONS"),
        )
        .child(setting_switch_row(
            "set-launch-login",
            "Open at login",
            "Start SingSwitch in the menu bar or tray when you sign in.",
            settings.launch_at_login,
            false,
            false,
            move |checked, _, cx| {
                store_login.update(cx, |s, cx| s.toggle_launch_at_login(*checked, cx));
            },
            cx,
        ))
        .child(setting_switch_row(
            "set-auto-ip",
            "Refresh the public IP after connect",
            "Look up the public IP at launch and after each switch.",
            settings.auto_refresh_ip,
            false,
            true,
            move |checked, _, cx| {
                store_ip.update(cx, |s, cx| s.set_auto_refresh_ip(*checked, cx));
            },
            cx,
        ))
        .child(setting_switch_row(
            "set-notifications",
            "Desktop notifications",
            "Show a system notification when the service switches or fails.",
            settings.desktop_notifications,
            false,
            true,
            move |checked, _, cx| {
                store_notify.update(cx, |s, cx| s.set_desktop_notifications(*checked, cx));
            },
            cx,
        ))
}

fn setting_switch_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    checked: bool,
    disabled: bool,
    border_top: bool,
    on_change: impl Fn(&bool, &mut Window, &mut gpui_kit::App) + 'static,
    cx: &gpui_kit::App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .pt(if border_top { px(10.0) } else { px(0.0) })
        .when(border_top, |row| {
            row.border_t_1().border_color(cx.theme().border.opacity(0.5))
        })
        .child(
            v_flex()
                .gap(px(2.0))
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
                .color(cx.theme().success)
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
            "Service",
            if layout.launchd_label.is_empty() {
                layout.service_name.clone()
            } else {
                layout.launchd_label.clone()
            },
        ),
        (
            "Control pipe",
            if layout.pipe_name.is_empty() {
                "—".to_string()
            } else {
                layout.pipe_name.clone()
            },
        ),
    ];

    v_flex()
        .w_full()
        .p(px(12.0))
        .gap(px(6.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(surface(cx))
        .child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    Icon::new(IconName::FolderCog)
                        .small()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(fonts::EMPHASIS)
                        .text_color(cx.theme().muted_foreground)
                        .child("SYSTEM PATHS"),
                ),
        )
        .children(paths.into_iter().enumerate().map(|(ix, (label, value))| {
            let val_copy = value.clone();
            let store_copy = store.clone();
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap(px(12.0))
                .py(px(4.0))
                .border_t_1()
                .border_color(cx.theme().border.opacity(0.4))
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
                        .text_size(px(12.0))
                        .font_family(mono_font.clone())
                        .text_color(cx.theme().foreground)
                        .truncate()
                        .child(value),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-path-{ix}")))
                        .ghost()
                        .h(px(24.0))
                        .w(px(24.0))
                        .icon(IconName::Copy)
                        .tooltip("Copy path")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(val_copy.clone()));
                            store_copy.update(cx, |s, cx| {
                                s.set_notice(
                                    NoticeOrigin::Settings,
                                    BannerTone::Success,
                                    "Copied to clipboard",
                                    cx,
                                );
                            });
                        }),
                )
        }))
}
