use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use sbctl_core::doctor::{
    DiagnosticCheck, FixAction, macos_sudoers_install_command, macos_sudoers_root_script,
};
use sbctl_core::layout::{Layout, PlatformKind};

use crate::actions::NavTab;
use crate::state::{AppStore, BannerTone, NoticeOrigin, count_noun};
use crate::theme::{subtle_pill_bg, surface_card_bg};

pub struct DoctorView {
    store: Entity<AppStore>,
    /// Passing checks are collapsed by default so problems lead the screen.
    show_passed: bool,
}

impl DoctorView {
    pub fn new(store: Entity<AppStore>) -> Self {
        Self {
            store,
            show_passed: false,
        }
    }

    /// True when a check needs the user's attention.
    ///
    /// Core sets `warning: true` only alongside `ok: true` (a check that passed
    /// but has something worth saying, such as an unmanaged sing-box binary),
    /// so bucketing on `!ok` alone silently hides every warning.
    fn needs_attention(check: &DiagnosticCheck) -> bool {
        !check.ok || check.warning
    }

    /// A check that genuinely passed with nothing to report.
    fn is_clean_pass(check: &DiagnosticCheck) -> bool {
        check.ok && !check.warning
    }

    fn render_summary_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let checks = &s.doctor_checks;
        let running = s.doctor_running;
        let init_error = &s.init_error;

        let failed_count = checks.iter().filter(|c| !c.ok).count();
        let warn_count = checks.iter().filter(|c| c.warning).count();
        let passed_count = checks.iter().filter(|c| Self::is_clean_pass(c)).count();
        let store_rerun = self.store.clone();

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
                            .text_size(px(20.0))
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child("Doctor"),
                    )
                    .when_some(
                        crate::views::notice_strip(&self.store, NoticeOrigin::Doctor, cx),
                        |el, notice| el.child(notice),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if let Some(err) = init_error {
                                err.clone()
                            } else if running {
                                "Checking...".to_string()
                            } else if checks.is_empty() {
                                "Check sing-box, profiles, permissions, and service state."
                                    .to_string()
                            } else if failed_count == 0 && warn_count == 0 {
                                format!("All {passed_count} checks passed.")
                            } else {
                                // Passes, warnings, and failures are exclusive buckets; a
                                // warning is never counted as a pass.
                                let mut parts = vec![format!("{passed_count} passed")];
                                if warn_count > 0 {
                                    parts.push(count_noun(warn_count, "warning", "warnings"));
                                }
                                if failed_count > 0 {
                                    parts.push(count_noun(failed_count, "failure", "failures"));
                                }
                                parts.join(", ")
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
                        store_rerun.update(cx, AppStore::run_doctor);
                    }),
            )
    }
}

impl Render for DoctorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let checks = s.doctor_checks.clone();
        let running = s.doctor_running;
        let is_busy = s.busy_phase.is_some();
        let layout = s.layout();

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_5()
            .child(self.render_summary_bar(cx))
            .when(running && checks.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(div().text_sm().child("Checking system...")),
                )
            })
            .child(
                v_flex()
                    .w_full()
                    .children(
                        checks
                            .iter()
                            .filter(|c| Self::needs_attention(c))
                            .cloned()
                            .enumerate()
                            .map(|(ix, chk)| {
                                render_check_card(ix, chk, &layout, is_busy, &self.store, cx)
                            }),
                    )
                    .when(
                        !checks.is_empty() && !checks.iter().any(Self::needs_attention),
                        |el| {
                            el.child(
                                div()
                                    .py(px(10.0))
                                    .text_sm()
                                    .text_color(cx.theme().success)
                                    .child("Everything is working."),
                            )
                        },
                    ),
            )
            .when(!checks.is_empty(), |el| {
                let passed = checks.iter().filter(|c| Self::is_clean_pass(c)).count();
                el.child(
                    h_flex()
                        .id("doctor-passed-toggle")
                        .w_full()
                        .gap(px(6.0))
                        .py(px(8.0))
                        .items_center()
                        .cursor_pointer()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Icon::new(if self.show_passed {
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
                                .child(format!("Show passed checks ({passed})")),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_passed = !this.show_passed;
                            cx.notify();
                        })),
                )
            })
            .when(self.show_passed, |el| {
                el.child(
                    v_flex().w_full().children(
                        checks
                            .iter()
                            .filter(|c| Self::is_clean_pass(c))
                            .cloned()
                            .enumerate()
                            .map(|(ix, chk)| {
                                render_check_card(ix + 100, chk, &layout, is_busy, &self.store, cx)
                            }),
                    ),
                )
            })
            .when(layout.platform == PlatformKind::Darwin, |this| {
                this.child(render_macos_sudoers_card(&layout, is_busy, &self.store, cx))
            })
            .when(layout.platform == PlatformKind::Windows, |this| {
                this.child(render_windows_helper_card(
                    &layout,
                    is_busy,
                    &self.store,
                    cx,
                ))
            })
    }
}

fn render_check_card(
    ix: usize,
    chk: DiagnosticCheck,
    layout: &Layout,
    is_busy: bool,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let (icon_el, status_word, status_color) = if chk.ok && !chk.warning {
        (
            Icon::new(IconName::CircleCheck).text_color(cx.theme().success),
            "OK",
            cx.theme().muted_foreground,
        )
    } else if chk.warning {
        (
            Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning),
            "Warning",
            cx.theme().warning,
        )
    } else {
        (
            Icon::new(IconName::CircleX).text_color(cx.theme().danger),
            "Failed",
            cx.theme().danger,
        )
    };

    let hint = chk.hint;
    let fix = chk.fix;
    let store_hint = store.clone();
    let store_detail = store.clone();
    let mono_font = cx.theme().mono_font_family.clone();

    v_flex()
        .w_full()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .w_full()
                .min_h(px(44.0))
                .py(px(6.0))
                .gap(px(12.0))
                .items_center()
                // fixed slot keeps every glyph on one optical line
                .child(div().w(px(20.0)).flex_none().child(icon_el))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_sm()
                                .font_bold()
                                .text_color(cx.theme().foreground)
                                .child(chk.name),
                        )
                        .child(
                            h_flex()
                                .gap(px(4.0))
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_size(px(12.0))
                                        .font_family(crate::fonts::mono_family())
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(chk.detail.clone()),
                                )
                                .when(!chk.detail.is_empty(), |el| {
                                    let detail = chk.detail.clone();
                                    let st = store_detail.clone();
                                    el.child(
                                        Button::new(SharedString::from(format!(
                                            "copy-detail-{ix}"
                                        )))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .tooltip("Copy detail")
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    detail.clone(),
                                                ));
                                                st.update(cx, |s, cx| {
                                                    s.set_notice(
                                                        NoticeOrigin::Doctor,
                                                        BannerTone::Success,
                                                        "Detail copied",
                                                        cx,
                                                    );
                                                });
                                            },
                                        ),
                                    )
                                }),
                        ),
                )
                .when_some(fix, |row, action| {
                    row.child(render_fix_button(ix, action, layout, is_busy, store))
                })
                .child(
                    div()
                        .w(px(72.0))
                        .flex_none()
                        .text_right()
                        .text_size(px(12.0))
                        .text_color(status_color)
                        .child(status_word),
                ),
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
                            .font_family(mono_font)
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-hint-{ix}")))
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .tooltip("Copy command")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(hint_copy.clone()));
                                store_hint.update(cx, |s, cx| {
                                    s.set_notice(
                                        NoticeOrigin::Doctor,
                                        BannerTone::Success,
                                        "Copied hint",
                                        cx,
                                    );
                                });
                            }),
                    ),
            )
        })
}

fn render_fix_button(
    ix: usize,
    action: FixAction,
    layout: &Layout,
    is_busy: bool,
    store: &Entity<AppStore>,
) -> Button {
    let st = store.clone();
    let label = match action {
        FixAction::CreateProfilesDir => "Create Folder",
        FixAction::RepairMacosSudoers => "Repair Sudoers...",
        FixAction::RepairMacosLogs => "Fix Permissions...",
        FixAction::RepairWindowsHelper => "Start Helper",
        FixAction::OpenProfiles => "Open Profiles",
    };

    let current_user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let root_script = macos_sudoers_root_script(layout, &current_user).unwrap_or_default();
    let preview_cmd = macos_sudoers_install_command(layout, &current_user).unwrap_or_default();
    let has_logs_script = sbctl_core::doctor::macos_logs_root_script(layout).is_some();

    let disabled = is_busy
        || matches!(
            action,
            FixAction::RepairMacosSudoers if !cfg!(target_os = "macos") || root_script.is_empty()
        )
        || matches!(
            action,
            FixAction::RepairMacosLogs if !cfg!(target_os = "macos") || !has_logs_script
        );

    Button::new(SharedString::from(format!("doctor-fix-{ix}")))
        .outline()
        .xsmall()
        .label(label)
        .disabled(disabled)
        .on_click(move |_, window, cx| match action {
            FixAction::CreateProfilesDir => st.update(cx, AppStore::ensure_profiles_dir),
            FixAction::RepairMacosSudoers => open_sudoers_confirm_dialog(
                root_script.clone(),
                preview_cmd.clone(),
                st.clone(),
                window,
                cx,
            ),
            FixAction::RepairMacosLogs => st.update(cx, AppStore::repair_macos_logs),
            FixAction::RepairWindowsHelper => st.update(cx, AppStore::repair_windows_helper),
            FixAction::OpenProfiles => {
                st.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
            }
        })
}

fn render_macos_sudoers_card(
    layout: &Layout,
    is_busy: bool,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let current_user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let sudoers_rules = layout.sudoers(&current_user).unwrap_or_default();
    let sudoers_cmd = macos_sudoers_install_command(layout, &current_user).unwrap_or_default();
    let sudoers_root = macos_sudoers_root_script(layout, &current_user).unwrap_or_default();
    let mono_font = cx.theme().mono_font_family.clone();

    let cmd_for_copy = sudoers_cmd.clone();
    let preview_cmd = sudoers_cmd.clone();
    let root_for_dialog = sudoers_root.clone();
    let store_copy = store.clone();
    let store_repair = store.clone();

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
                                .child("macOS sudoers"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Scoped /etc/sudoers.d/sbctl rules for ln and launchctl, checked with visudo -cf before install."),
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
                                .label("Copy Command")
                                .disabled(sudoers_cmd.is_empty())
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        cmd_for_copy.clone(),
                                    ));
                                    store_copy.update(cx, |s, cx| {
                                        s.set_notice(NoticeOrigin::Doctor, BannerTone::Success, "Copied sudoers command", cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("doctor-repair-sudoers")
                                .primary()
                                .small()
                                .label("Repair Sudoers...")
                                .disabled(
                                    is_busy || !cfg!(target_os = "macos") || sudoers_root.is_empty(),
                                )
                                .on_click(move |_, window, cx| {
                                    open_sudoers_confirm_dialog(
                                        root_for_dialog.clone(),
                                        preview_cmd.clone(),
                                        store_repair.clone(),
                                        window,
                                        cx,
                                    );
                                }),
                        ),
                ),
        )
        .when(!sudoers_rules.is_empty(), |t| {
            t.child(
                div()
                    .w_full()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(subtle_pill_bg(cx))
                    .text_xs()
                    .font_family(mono_font)
                    .text_color(cx.theme().foreground)
                    .child(sudoers_rules),
            )
        })
}

fn render_windows_helper_card(
    layout: &Layout,
    is_busy: bool,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let install_cmd = "powershell -ExecutionPolicy Bypass -File scripts\\install.ps1";
    let store_copy = store.clone();
    let store_repair = store.clone();
    let mono_font = cx.theme().mono_font_family.clone();

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
                                .child("Windows helper service"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "sbctl-agent listens on {} so profile switches do not prompt for UAC.",
                                    layout.pipe_name
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("doctor-copy-win-install")
                                .outline()
                                .small()
                                .icon(IconName::Copy)
                                .label("Copy Command")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        install_cmd.to_string(),
                                    ));
                                    store_copy.update(cx, |s, cx| {
                                        s.set_notice(NoticeOrigin::Doctor, BannerTone::Success, "Copied installer command", cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("doctor-repair-win-helper")
                                .primary()
                                .small()
                                .label("Start Helper")
                                .disabled(is_busy)
                                .on_click(move |_, _, cx| {
                                    store_repair.update(cx, AppStore::repair_windows_helper);
                                }),
                        ),
                ),
        )
        .child(
            div()
                .w_full()
                .p_3()
                .rounded(cx.theme().radius)
                .bg(subtle_pill_bg(cx))
                .text_xs()
                .font_family(mono_font)
                .text_color(cx.theme().foreground)
                .child(install_cmd),
        )
}

fn open_sudoers_confirm_dialog(
    root_script: String,
    display_cmd: String,
    store: Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let script_run = root_script.clone();
        let store_done = store.clone();
        alert
            .confirm()
            .title("Install /etc/sudoers.d/sbctl")
            .description(format!(
                "Prompts for an administrator password, validates with visudo -cf, and writes /etc/sudoers.d/sbctl:\n\n{display_cmd}"
            ))
            .ok_text("Install")
            .on_ok(move |_, _, cx| {
                store_done.update(cx, |s, cx| s.repair_macos_sudoers(script_run.clone(), cx));
                true
            })
    });
}
