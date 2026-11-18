use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use sbctl_core::doctor::{
    DiagnosticCheck, FixAction, macos_sudoers_install_command, macos_sudoers_root_script,
};
use sbctl_core::layout::{Layout, PlatformKind};

use crate::actions::NavTab;
use crate::state::{AppStore, BannerTone, count_noun};
use crate::theme::{subtle_pill_bg, surface_card_bg};

pub struct DoctorView {
    store: Entity<AppStore>,
}

impl DoctorView {
    pub fn new(store: Entity<AppStore>) -> Self {
        Self { store }
    }

    fn render_summary_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let checks = &s.doctor_checks;
        let running = s.doctor_running;
        let init_error = &s.init_error;

        let failed_count = checks.iter().filter(|c| !c.ok && !c.warning).count();
        let warn_count = checks.iter().filter(|c| !c.ok && c.warning).count();
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
                            .text_base()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Diagnostics"),
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
                                format!("All {} checks passed.", checks.len())
                            } else {
                                let errs = count_noun(failed_count, "error", "errors");
                                let warns = count_noun(warn_count, "warning", "warnings");
                                format!("{errs}, {warns} across {} checks.", checks.len())
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
                    .gap_3()
                    .children(checks.into_iter().enumerate().map(|(ix, chk)| {
                        render_check_card(ix, chk, &layout, is_busy, &self.store, cx)
                    })),
            )
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
    let (icon_el, badge) = if chk.ok && !chk.warning {
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

    let hint = chk.hint;
    let fix = chk.fix;
    let store_hint = store.clone();
    let mono_font = cx.theme().mono_font_family.clone();

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
                    h_flex().gap_2().items_center().child(icon_el).child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(chk.name),
                    ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when_some(fix, |row, action| {
                            row.child(render_fix_button(ix, action, layout, is_busy, store))
                        })
                        .child(badge),
                ),
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
                            .font_family(mono_font)
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-hint-{ix}")))
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(hint_copy.clone()));
                                store_hint.update(cx, |s, cx| {
                                    s.set_banner(BannerTone::Success, "Copied hint", cx);
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
                                        s.set_banner(
                                            BannerTone::Success,
                                            "Copied sudoers command",
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
                                        s.set_banner(
                                            BannerTone::Success,
                                            "Copied installer command",
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .child(
                            Button::new("doctor-repair-win-helper")
                                .primary()
                                .small()
                                .label("Repair Helper")
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
