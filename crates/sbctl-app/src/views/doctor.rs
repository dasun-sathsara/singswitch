use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use sbctl_core::doctor::{macos_sudoers_install_command, macos_sudoers_root_script};
use sbctl_core::layout::PlatformKind;

use crate::state::{AppStore, BannerTone};
use crate::theme::{subtle_pill_bg, surface_card_bg};

pub struct DoctorView {
    store: Entity<AppStore>,
}

impl DoctorView {
    pub fn new(store: Entity<AppStore>) -> Self {
        Self { store }
    }
}

impl Render for DoctorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let checks = s.doctor_checks.clone();
        let running = s.doctor_running;
        let layout = s.layout();
        let init_error = s.init_error.clone();

        let failed_count = checks.iter().filter(|c| !c.ok && !c.warning).count();
        let warn_count = checks.iter().filter(|c| !c.ok && c.warning).count();

        let current_user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
        let sudoers_rules = layout.sudoers(&current_user).unwrap_or_default();
        let sudoers_cmd = macos_sudoers_install_command(&layout, &current_user).unwrap_or_default();
        let sudoers_root = macos_sudoers_root_script(&layout, &current_user).unwrap_or_default();

        let store_rerun = self.store.clone();
        let store_copy_cmd = self.store.clone();
        let store_repair = self.store.clone();
        let mono_font = cx.theme().mono_font_family.clone();

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_5()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .p_4()
                    .rounded(cx.theme().radius_lg)
                    .bg(surface_card_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("System Diagnostics"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if let Some(err) = &init_error {
                                        err.clone()
                                    } else if running {
                                        "Running checks...".to_string()
                                    } else if checks.is_empty() {
                                        "Click Run Doctor to inspect binaries, profiles, privileges, and service health.".to_string()
                                    } else if failed_count == 0 && warn_count == 0 {
                                        format!("All {} diagnostic checks passed.", checks.len())
                                    } else {
                                        format!(
                                            "{failed_count} error(s) and {warn_count} warning(s) across {} checks.",
                                            checks.len()
                                        )
                                    }),
                            ),
                    )
                    .child(
                        Button::new("doctor-run")
                            .primary()
                            .icon(IconName::RefreshCw)
                            .label("Run Doctor")
                            .disabled(running || init_error.is_some())
                            .on_click(move |_, _, cx| {
                                store_rerun.update(cx, |s, cx| s.run_doctor(cx));
                            }),
                    ),
            )
            .when(running && checks.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(div().text_sm().child("Inspecting environment...")),
                )
            })
            .child(v_flex().w_full().gap_3().children(checks.into_iter().enumerate().map(
                |(ix, chk)| {
                    let (icon_el, badge) = if chk.ok {
                        (
                            Icon::new(IconName::CircleCheck).text_color(cx.theme().success),
                            Tag::success().small().child("OK"),
                        )
                    } else if chk.warning {
                        (
                            Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning),
                            Tag::warning().small().child("Warning"),
                        )
                    } else {
                        (
                            Icon::new(IconName::CircleX).text_color(cx.theme().danger),
                            Tag::danger().small().child("Failed"),
                        )
                    };

                    let hint = chk.hint.clone();
                    let store_hint = self.store.clone();

                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_2()
                        .rounded(cx.theme().radius_lg)
                        .bg(surface_card_bg(cx))
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(icon_el)
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child(chk.name),
                                        ),
                                )
                                .child(badge),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(chk.detail),
                        )
                        .when(!hint.is_empty(), |this| {
                            let hint_copy = hint.clone();
                            this.child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .items_center()
                                    .px_3()
                                    .py_2()
                                    .gap_2()
                                    .rounded(cx.theme().radius)
                                    .bg(subtle_pill_bg(cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .font_family(mono_font.clone())
                                            .text_color(cx.theme().muted_foreground)
                                            .child(hint),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("copy-hint-{ix}")))
                                            .ghost()
                                            .small()
                                            .icon(IconName::Copy)
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    hint_copy.clone(),
                                                ));
                                                store_hint.update(cx, |s, cx| {
                                                    s.set_banner(
                                                        BannerTone::Success,
                                                        "Copied remediation hint to clipboard",
                                                        cx,
                                                    );
                                                });
                                            }),
                                    ),
                            )
                        })
                },
            )))
            .when(layout.platform == PlatformKind::Darwin, |this| {
                let cmd_for_copy = sudoers_cmd.clone();
                let root_for_dialog = sudoers_root.clone();
                let preview_cmd = sudoers_cmd.clone();
                let rules_preview = sudoers_rules.clone();
                let store_after_repair = store_repair.clone();

                this.child(
                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_3()
                        .rounded(cx.theme().radius_lg)
                        .bg(surface_card_bg(cx))
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child("macOS Sudoers Rules (/etc/sudoers.d/sbctl)"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Five scoped rules for ln and launchctl; staged and validated with visudo -cf before atomic install."),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("doctor-copy-sudoers")
                                                .outline()
                                                .small()
                                                .icon(IconName::Copy)
                                                .label("Copy Install Command")
                                                .disabled(sudoers_cmd.is_empty())
                                                .on_click(move |_, _, cx| {
                                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                                        cmd_for_copy.clone(),
                                                    ));
                                                    store_copy_cmd.update(cx, |s, cx| {
                                                        s.set_banner(
                                                            BannerTone::Success,
                                                            "Copied staged visudo install command to clipboard",
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .child(
                                            Button::new("doctor-repair-sudoers")
                                                .primary()
                                                .small()
                                                .label("Repair Sudoers...")
                                                .disabled(!cfg!(target_os = "macos") || sudoers_root.is_empty())
                                                .on_click(move |_, window, cx| {
                                                    let root_script = root_for_dialog.clone();
                                                    let display_cmd = preview_cmd.clone();
                                                    let store_exec = store_after_repair.clone();
                                                    window.open_alert_dialog(cx, move |alert, _, _| {
                                                        let script_run = root_script.clone();
                                                        let store_done = store_exec.clone();
                                                        alert
                                                            .confirm()
                                                            .title("Install /etc/sudoers.d/sbctl")
                                                            .description(format!(
                                                                "This will prompt for an administrator password via macOS Authorization, stage to a temp file, validate via visudo -cf, and install 0440 root:wheel:\n\n{display_cmd}"
                                                            ))
                                                            .ok_text("Authorize & Install")
                                                            .on_ok(move |_, _, cx| {
                                                                let apple_script = format!(
                                                                    "do shell script \"{}\" with administrator privileges",
                                                                    script_run.replace('\\', "\\\\").replace('"', "\\\"")
                                                                );
                                                                match std::process::Command::new("osascript")
                                                                    .args(["-e", &apple_script])
                                                                    .output()
                                                                {
                                                                    Ok(out) if out.status.success() => {
                                                                        store_done.update(cx, |s, cx| {
                                                                            s.set_banner(
                                                                                BannerTone::Success,
                                                                                "Installed /etc/sudoers.d/sbctl",
                                                                                cx,
                                                                            );
                                                                            s.run_doctor(cx);
                                                                        });
                                                                    }
                                                                    Ok(out) => {
                                                                        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                                                                        store_done.update(cx, |s, cx| {
                                                                            s.set_banner(BannerTone::Error, err, cx);
                                                                        });
                                                                    }
                                                                    Err(e) => {
                                                                        store_done.update(cx, |s, cx| {
                                                                            s.set_banner(BannerTone::Error, e.to_string(), cx);
                                                                        });
                                                                    }
                                                                }
                                                                true
                                                            })
                                                    });
                                                }),
                                        ),
                                ),
                        )
                        .when(!rules_preview.is_empty(), |t| {
                            t.child(
                                div()
                                    .w_full()
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .bg(subtle_pill_bg(cx))
                                    .text_xs()
                                    .font_family(mono_font)
                                    .text_color(cx.theme().foreground)
                                    .child(rules_preview),
                            )
                        }),
                )
            })
    }
}
