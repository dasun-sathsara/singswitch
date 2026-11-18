use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};

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
        let mono_font = cx.theme().mono_font_family.clone();

        let store_light = self.store.clone();
        let store_dark = self.store.clone();
        let store_system = self.store.clone();
        let store_login = self.store.clone();
        let store_menubar = self.store.clone();
        let store_ip = self.store.clone();

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
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_5()
            // Appearance card.
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
                                            .child("Choose restrained light, dark, or match system appearance."),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("theme-light")
                                            .small()
                                            .icon(IconName::Sun)
                                            .when(settings.theme == ThemePreference::Light, |b| {
                                                b.primary()
                                            })
                                            .when(settings.theme != ThemePreference::Light, |b| {
                                                b.outline()
                                            })
                                            .label("Light")
                                            .on_click(move |_, _, cx| {
                                                store_light.update(cx, |s, cx| {
                                                    s.set_theme_preference(
                                                        ThemePreference::Light,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("theme-dark")
                                            .small()
                                            .icon(IconName::Moon)
                                            .when(settings.theme == ThemePreference::Dark, |b| {
                                                b.primary()
                                            })
                                            .when(settings.theme != ThemePreference::Dark, |b| {
                                                b.outline()
                                            })
                                            .label("Dark")
                                            .on_click(move |_, _, cx| {
                                                store_dark.update(cx, |s, cx| {
                                                    s.set_theme_preference(
                                                        ThemePreference::Dark,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("theme-system")
                                            .small()
                                            .when(settings.theme == ThemePreference::System, |b| {
                                                b.primary()
                                            })
                                            .when(settings.theme != ThemePreference::System, |b| {
                                                b.outline()
                                            })
                                            .label("System")
                                            .on_click(move |_, _, cx| {
                                                store_system.update(cx, |s, cx| {
                                                    s.set_theme_preference(
                                                        ThemePreference::System,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    ),
                            ),
                    ),
            )
            // Behavior card.
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
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Desktop & Tray"),
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
                                            .child("Launch at login"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Start sbctl in the background at user login using --background."),
                                    ),
                            )
                            .child(
                                Switch::new("set-launch-login")
                                    .checked(settings.launch_at_login)
                                    .on_change(move |checked, _, cx| {
                                        store_login.update(cx, |s, cx| {
                                            s.toggle_launch_at_login(*checked, cx);
                                        });
                                    }),
                            ),
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
                                            .child("Show active profile in macOS menu bar"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Display the running profile name beside the menu bar icon on macOS."),
                                    ),
                            )
                            .child(
                                Switch::new("set-menubar-title")
                                    .checked(settings.show_profile_in_menubar)
                                    .disabled(!cfg!(target_os = "macos"))
                                    .on_change(move |checked, _, cx| {
                                        store_menubar.update(cx, |s, cx| {
                                            s.set_show_profile_in_menubar(*checked, cx);
                                        });
                                    }),
                            ),
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
                                            .child("Auto-refresh public IP"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Query ipinfo.io after connecting or disconnecting."),
                                    ),
                            )
                            .child(
                                Switch::new("set-auto-ip")
                                    .checked(settings.auto_refresh_ip)
                                    .on_change(move |checked, _, cx| {
                                        store_ip.update(cx, |s, cx| {
                                            s.set_auto_refresh_ip(*checked, cx);
                                        });
                                    }),
                            ),
                    ),
            )
            // Resolved paths card.
            .child(
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
                            .child("Resolved System Paths"),
                    )
                    .children(paths.into_iter().enumerate().map(|(ix, (label, value))| {
                        let val_copy = value.clone();
                        let store_copy = self.store.clone();
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px_width())
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
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            val_copy.clone(),
                                        ));
                                        store_copy.update(cx, |s, cx| {
                                            s.set_banner(
                                                BannerTone::Success,
                                                "Copied path to clipboard",
                                                cx,
                                            );
                                        });
                                    }),
                            )
                    })),
            )
    }
}

fn px_width() -> gpui_kit::Pixels {
    gpui_kit::px(160.0)
}
