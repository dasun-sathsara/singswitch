use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, ClipboardItem, Entity, IntoElement, ParentElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use sbctl_core::doctor::macos_sudoers_root_script;
use sbctl_core::layout::PlatformKind;
use sbctl_core::profile::Profile;

use crate::actions::NavTab;
use crate::state::{AppStore, BannerTone};
use crate::theme::{subtle_pill_bg, surface_card_bg};
use crate::views::profiles::ProfilesView;

pub fn render_onboarding_card(store: &Entity<AppStore>, cx: &App) -> impl IntoElement {
    let s = store.read(cx);
    let layout = s.layout();
    let checks = &s.doctor_checks;
    let is_busy = s.busy_phase.is_some();

    let singbox_ok = checks
        .iter()
        .find(|c| c.name == "sing-box")
        .is_some_and(|c| c.ok);
    let priv_ok = match layout.platform {
        PlatformKind::Darwin => {
            let any_sudo = checks.iter().any(|c| c.name.starts_with("sudo:"));
            any_sudo
                && checks
                    .iter()
                    .filter(|c| c.name.starts_with("sudo:"))
                    .all(|c| c.ok)
        }
        PlatformKind::Windows => checks
            .iter()
            .find(|c| c.name == "helper service")
            .is_some_and(|c| c.ok && !c.warning),
    };
    let profile_ready = s.snapshot.profiles.iter().any(Profile::ready);
    let has_any_profile = !s.snapshot.profiles.is_empty();
    let ready_to_connect = singbox_ok && profile_ready;

    let store_dismiss = store.clone();
    let store_connect = store.clone();

    v_flex()
        .w_full()
        .p_5()
        .gap_3()
        .rounded(cx.theme().radius_lg)
        .bg(surface_card_bg(cx))
        .border_1()
        .border_color(cx.theme().primary)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Tag::info().small().child("Setup"))
                                .child(
                                    div()
                                        .text_base()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child("First-run setup"),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Finish these four steps before connecting."),
                        ),
                )
                .child(
                    Button::new("onboard-dismiss")
                        .ghost()
                        .small()
                        .label("Dismiss")
                        .on_click(move |_, _, cx| {
                            store_dismiss.update(cx, |s, cx| s.set_onboarding_dismissed(true, cx));
                        }),
                ),
        )
        .child(step_install_singbox(singbox_ok, layout.platform, store, cx))
        .child(step_privileges(priv_ok, is_busy, &layout, store, cx))
        .child(step_profile(has_any_profile, profile_ready, store, cx))
        .child(step_connect(
            ready_to_connect,
            s.snapshot.running,
            is_busy,
            store_connect,
            cx,
        ))
}

fn step_row(
    num: &str,
    title: &'static str,
    done: bool,
    detail: String,
    actions: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .p_3()
        .justify_between()
        .items_center()
        .gap_3()
        .rounded(cx.theme().radius)
        .bg(subtle_pill_bg(cx))
        .child(
            h_flex()
                .flex_1()
                .gap_3()
                .items_center()
                .child(
                    Icon::new(if done {
                        IconName::CircleCheck
                    } else {
                        IconName::CircleX
                    })
                    .text_color(if done {
                        cx.theme().success
                    } else {
                        cx.theme().muted_foreground
                    }),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child(format!("{num}. {title}")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail),
                        ),
                ),
        )
        .child(h_flex().gap_2().items_center().child(actions))
}

fn step_install_singbox(
    done: bool,
    platform: PlatformKind,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let install_cmd = match platform {
        PlatformKind::Darwin => "brew install sing-box",
        PlatformKind::Windows => "winget install SagerNet.sing-box",
    };
    let store_copy = store.clone();
    let store_recheck = store.clone();
    step_row(
        "1",
        "Install sing-box",
        done,
        if done {
            "Installed".to_string()
        } else {
            format!("Install with `{install_cmd}`.")
        },
        h_flex()
            .gap_2()
            .when(!done, |b| {
                b.child(
                    Button::new("onboard-copy-sb")
                        .outline()
                        .xsmall()
                        .icon(IconName::Copy)
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                install_cmd.to_string(),
                            ));
                            store_copy.update(cx, |s, cx| {
                                s.set_banner(
                                    BannerTone::Success,
                                    format!("Copied `{install_cmd}`"),
                                    cx,
                                );
                            });
                        }),
                )
            })
            .child(
                Button::new("onboard-recheck-sb")
                    .ghost()
                    .xsmall()
                    .label("Recheck")
                    .on_click(move |_, _, cx| store_recheck.update(cx, AppStore::run_doctor)),
            ),
        cx,
    )
}

fn step_privileges(
    done: bool,
    is_busy: bool,
    layout: &sbctl_core::layout::Layout,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let detail = match layout.platform {
        PlatformKind::Darwin => {
            if done {
                "/etc/sudoers.d/sbctl is active.".to_string()
            } else {
                "Install scoped sudoers rules for ln and launchctl.".to_string()
            }
        }
        PlatformKind::Windows => {
            if done {
                "sbctl-agent pipe is running.".to_string()
            } else {
                "Start sbctl-agent to switch profiles without UAC prompts.".to_string()
            }
        }
    };
    let platform = layout.platform;
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let root_script = macos_sudoers_root_script(layout, &user).unwrap_or_default();
    let store_fix = store.clone();
    let store_doc = store.clone();

    step_row(
        "2",
        "Grant service permissions",
        done,
        detail,
        h_flex()
            .gap_2()
            .when(!done, |h| {
                h.child(
                    Button::new("onboard-fix-priv")
                        .outline()
                        .xsmall()
                        .disabled(is_busy)
                        .label(match platform {
                            PlatformKind::Darwin => "Install Sudoers...",
                            PlatformKind::Windows => "Start Helper",
                        })
                        .on_click(move |_, window, cx| match platform {
                            PlatformKind::Darwin => open_sudoers_prompt(
                                root_script.clone(),
                                store_fix.clone(),
                                window,
                                cx,
                            ),
                            PlatformKind::Windows => {
                                store_fix.update(cx, AppStore::repair_windows_helper);
                            }
                        }),
                )
            })
            .child(
                Button::new("onboard-open-doc")
                    .ghost()
                    .xsmall()
                    .label("Doctor")
                    .on_click(move |_, _, cx| {
                        store_doc.update(cx, |s, cx| {
                            s.set_tab(NavTab::Doctor, cx);
                            s.run_doctor(cx);
                        });
                    }),
            ),
        cx,
    )
}

fn step_profile(
    has_any: bool,
    ready: bool,
    store: &Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    let store_new = store.clone();
    let store_pull = store.clone();
    let store_edit = store.clone();
    let detail = if ready {
        "Ready profile available.".to_string()
    } else if has_any {
        "Replace remaining TODO_ placeholders.".to_string()
    } else {
        "Create a profile or import a JSON URL.".to_string()
    };

    step_row(
        "3",
        "Configure a profile",
        ready,
        detail,
        h_flex()
            .gap_2()
            .when(has_any && !ready, |h| {
                h.child(
                    Button::new("onboard-finish-edit")
                        .primary()
                        .xsmall()
                        .label("Edit")
                        .on_click(move |_, _, cx| {
                            store_edit.update(cx, |s, cx| s.set_tab(NavTab::Profiles, cx));
                        }),
                )
            })
            .child(
                Button::new("onboard-new-prof")
                    .outline()
                    .xsmall()
                    .label("New")
                    .on_click(move |_, window, cx| {
                        ProfilesView::open_new_profile_dialog(store_new.clone(), window, cx);
                    }),
            )
            .child(
                Button::new("onboard-pull-prof")
                    .outline()
                    .xsmall()
                    .label("Import")
                    .on_click(move |_, window, cx| {
                        ProfilesView::open_import_url_dialog(store_pull.clone(), window, cx);
                    }),
            ),
        cx,
    )
}

fn step_connect(
    ready_to_connect: bool,
    running: bool,
    is_busy: bool,
    store_connect: Entity<AppStore>,
    cx: &App,
) -> impl IntoElement {
    step_row(
        "4",
        "Connect",
        running,
        if running {
            "Tunnel is up.".to_string()
        } else {
            "Start sing-box with a ready profile.".to_string()
        },
        Button::new("onboard-connect")
            .primary()
            .xsmall()
            .label(if running { "Connected" } else { "Connect" })
            .disabled(is_busy || running || !ready_to_connect)
            .on_click(move |_, _, cx| store_connect.update(cx, AppStore::connect_active)),
        cx,
    )
}

fn open_sudoers_prompt(
    root_script: String,
    store: Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    if root_script.is_empty() {
        return;
    }
    window.open_alert_dialog(cx, move |alert, _, _| {
        let script = root_script.clone();
        let st = store.clone();
        alert
            .confirm()
            .title("Install /etc/sudoers.d/sbctl")
            .description("Prompts for an administrator password, validates the rules with visudo -cf, and writes /etc/sudoers.d/sbctl.")
            .ok_text("Install")
            .on_ok(move |_, _, cx| {
                st.update(cx, |s, cx| s.repair_macos_sudoers(script.clone(), cx));
                true
            })
    });
}
