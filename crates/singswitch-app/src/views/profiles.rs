use crate::fonts;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::icons::TrayVisualState;
use crate::state::{AppStore, BannerTone, LatencyReading, NoticeOrigin};
use crate::views::editor::{ProfileEditorPanel, format_latency};
use singswitch_core::logs::contains_ignore_ascii_case;
use singswitch_core::profile::Profile;
use singswitch_core::profile_link;

pub struct ProfilesView {
    store: Entity<AppStore>,
    editor: Entity<ProfileEditorPanel>,
    search_input: Entity<InputState>,
    expanded_profile: Option<String>,
    /// Drives the Disconnect button's hover animation. A tiny entity because
    /// the animation frame callback only has `&mut App`, not the view.
    disconnect_hover: Entity<HoverSpring>,
    _subs: [Subscription; 2],
}

/// Hover progress for one animated control: 0.0 idle, 1.0 hovered.
pub struct HoverSpring {
    pub value: f32,
}

impl ProfilesView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| ProfileEditorPanel::new(store.clone(), window, cx));
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search profiles")
                .default_value(store.read(cx).profile_search_query.clone())
        });

        let store_sub = store.clone();
        let sub = cx.subscribe_in(&search_input, window, move |_, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let q = state.read(cx).value().to_string();
                store_sub.update(cx, |s, cx| s.set_profile_search(q, cx));
            }
        });

        let store_observer = cx.observe(&store, |_, _, cx| cx.notify());

        let disconnect_hover = cx.new(|_| HoverSpring { value: 0.0 });
        Self {
            store,
            editor,
            search_input,
            expanded_profile: None,
            disconnect_hover,
            _subs: [sub, store_observer],
        }
    }

    /// Ask the editor to stash its buffer before the panel goes away.
    pub fn stash_draft(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |ed, cx| ed.stash_draft(cx));
    }

    pub fn trigger_save(
        &mut self,
        and_activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |ed, cx| {
            ed.trigger_save(and_activate, window, cx);
        });
    }

    pub fn open_new_profile_dialog(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. home-vless"));
        let initial_focus = Cell::new(true);
        let error = Rc::new(RefCell::new(None::<String>));

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let input_for_ok = name_input.clone();
            let store_for_ok = store.clone();
            let error_for_ok = error.clone();

            dialog
                .title("New profile")
                .width(px(420.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Create a profile from the VLESS + Reality template."),
                        )
                        .child(Input::new(&name_input).cleanable(true)),
                )
                .when_some(error.borrow().clone(), |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("new-prof-cancel")
                                .secondary()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-prof-create")
                                .primary()
                                .label("Create")
                                .on_click(move |_, window, cx| {
                                    if store_for_ok.read(cx).busy_phase.is_some() {
                                        *error_for_ok.borrow_mut() = Some(
                                            "Wait for the current operation to finish.".into(),
                                        );
                                        window.refresh();
                                        return;
                                    }
                                    let raw_name = input_for_ok.read(cx).value().trim().to_string();
                                    // The create itself runs on a background
                                    // task (on Windows it can raise a UAC
                                    // prompt); a failure arrives as a notice
                                    // on the stage, so the dialog dispatches
                                    // and closes rather than holding the
                                    // window open across the await.
                                    if store_for_ok
                                        .update(cx, |s, cx| s.create_profile(&raw_name, cx))
                                        .is_ok()
                                    {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    pub fn open_import_url_dialog(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Profile name"));
        let url_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("https://example.com/sing-box.json"));
        let initial_focus = Cell::new(true);
        let error = Rc::new(RefCell::new(None::<String>));

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let n_ok = name_input.clone();
            let u_ok = url_input.clone();
            let s_ok = store.clone();
            let error_for_ok = error.clone();

            dialog
                .title("Import profile")
                .width(px(480.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Fetch a sing-box JSON config over HTTP(S) and validate it.",
                                ),
                        )
                        .child(Input::new(&name_input).cleanable(true))
                        .child(Input::new(&url_input).cleanable(true)),
                )
                .when_some(error.borrow().clone(), |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("import-cancel")
                                .secondary()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("import-submit")
                                .primary()
                                .label("Import")
                                .on_click(move |_, window, cx| {
                                    let name = n_ok.read(cx).value().trim().to_string();
                                    let url = u_ok.read(cx).value().trim().to_string();
                                    let error = if s_ok.read(cx).busy_phase.is_some() {
                                        Some("Wait for the current operation to finish.".into())
                                    } else if url.is_empty() {
                                        Some("Enter the configuration URL.".into())
                                    } else {
                                        singswitch_core::profile::validate_name(&name)
                                            .and_then(|()| {
                                                singswitch_core::remote::normalise_profile_url(&url)
                                                    .map(|_| ())
                                            })
                                            .err()
                                            .map(|err| err.to_string())
                                    };
                                    if let Some(error) = error {
                                        *error_for_ok.borrow_mut() = Some(error);
                                        window.refresh();
                                        return;
                                    }
                                    s_ok.update(cx, |s, cx| {
                                        s.pull_profile(name, url, NoticeOrigin::Stage, cx)
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    /// Import a profile from a `vless://`-style share link.
    ///
    /// The link is parsed into a config and validated with `sing-box check` on a
    /// background executor, then written with an exclusive create, so the window
    /// never blocks on a subprocess and an existing profile is never replaced.
    pub fn open_import_link_dialog(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Profile name"));
        let link_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("vless://uuid@host:443?security=tls#name")
        });
        let initial_focus = Cell::new(true);
        let error = Rc::new(RefCell::new(None::<String>));
        let busy = Rc::new(Cell::new(false));

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let n_ok = name_input.clone();
            let l_ok = link_input.clone();
            let s_ok = store.clone();
            let error_for_ok = error.clone();
            let busy_for_ok = busy.clone();

            dialog
                .title("Import from share link")
                .width(px(520.0))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Paste a vless, trojan, hysteria2, ss or vmess link. It is converted to a sing-box config and validated before it is saved.",
                                ),
                        )
                        .child(Input::new(&name_input).cleanable(true))
                        .child(Input::new(&link_input).cleanable(true)),
                )
                .when_some(error.borrow().clone(), |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("import-link-cancel")
                                .secondary()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("import-link-submit")
                                .primary()
                                .label("Import")
                                .on_click(move |_, window, cx| {
                                    let name = n_ok.read(cx).value().trim().to_string();
                                    let link = l_ok.read(cx).value().trim().to_string();
                                    let error = if busy_for_ok.get()
                                        || s_ok.read(cx).busy_phase.is_some()
                                    {
                                        Some("Wait for the current operation to finish.".into())
                                    } else if link.is_empty() {
                                        Some("Paste the share link.".into())
                                    } else {
                                        singswitch_core::profile::validate_name(&name)
                                            .err()
                                            .map(|err| err.to_string())
                                    };
                                    if let Some(error) = error {
                                        *error_for_ok.borrow_mut() = Some(error);
                                        window.refresh();
                                        return;
                                    }
                                    *error_for_ok.borrow_mut() = None;
                                    Self::start_import_from_link(
                                        s_ok.clone(),
                                        name,
                                        link,
                                        busy_for_ok.clone(),
                                        cx,
                                    );
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    /// Run the import off the UI thread, then refresh the store's snapshot.
    ///
    /// Everything here goes through fields and methods the store already
    /// exposes (`ctrl`, `busy_phase`, `refresh_snapshot`, `set_notice`), because
    /// the stage's own actions live in `state.rs`.
    fn start_import_from_link(
        store: Entity<AppStore>,
        name: String,
        link: String,
        busy: Rc<Cell<bool>>,
        cx: &mut App,
    ) {
        let Some((ctrl, profiles_dir)) = store.read(cx).ctrl.clone().map(|ctrl| {
            let dir = ctrl.layout.profiles_dir.clone();
            (ctrl, dir)
        }) else {
            store.update(cx, |s, cx| {
                let message = s
                    .init_error
                    .clone()
                    .unwrap_or_else(|| "This platform is not supported".to_string());
                s.set_notice(NoticeOrigin::Stage, BannerTone::Warning, message, cx);
            });
            return;
        };

        busy.set(true);
        store.update(cx, |s, cx| {
            s.busy_phase = Some(format!("Importing {name}..."));
            cx.notify();
        });

        cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
            let checker = ctrl.checker.clone();
            let (name_for_work, link_for_work) = (name.clone(), link);
            let result = cx
                .background_executor()
                .spawn(async move {
                    profile_link::import_profile(
                        &profiles_dir,
                        checker.as_ref(),
                        &name_for_work,
                        &link_for_work,
                    )
                })
                .await;

            busy.set(false);
            store.update(cx, |s, cx| {
                s.busy_phase = None;
                match result {
                    Ok(_) => {
                        // Same race as `create_profile` and `pull_profile`: a
                        // poll that started earlier can land with a profile
                        // list that still lacks the new name and reselect the
                        // previous profile over it. The store owns the pin.
                        s.import_landed(name.clone(), cx);
                    }
                    Err(err) => {
                        // The store's shared error printer (error_text) is
                        // private to state.rs, so this mirrors it. Reuse it the
                        // next time state.rs is open: same shape, one place.
                        let message = if err.hints.is_empty() {
                            err.message.clone()
                        } else {
                            format!("{}: {}", err.message, err.hints.join("; "))
                        };
                        s.set_notice(NoticeOrigin::Stage, BannerTone::Error, message, cx);
                    }
                }
                // The stage re-reads profiles from disk and the status from the
                // service; it is cheap and already the path every other action
                // uses to make a new file visible.
                s.refresh_snapshot(true, cx);
            });
        })
        .detach();
    }

    fn render_connections(&self, cx: &Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let busy = store.is_busy();
        let content = v_flex()
            .size_full()
            .px(px(14.0))
            .py(px(12.0))
            .gap(px(10.0))
            .child(self.render_connection(cx))
            .when_some(
                super::notice_strip(&self.store, NoticeOrigin::Stage, cx),
                |page, notice| page.child(notice),
            );

        if store.snapshot.profiles.is_empty() && !store.is_probing() {
            let create = self.store.clone();
            let import = self.store.clone();
            let import_link = self.store.clone();
            return content.child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .justify_center()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(32.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(crate::theme::surface(cx))
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded_full()
                            .bg(cx.theme().muted)
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Route)
                                    .size(px(24.0))
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(fonts::EMPHASIS)
                            .text_color(cx.theme().foreground)
                            .child("No profiles yet"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                        "Create a profile, paste a subscription URL, or paste a share link to start routing.",
                                    ),
                    )
                    .child(
                        h_flex()
                            .mt(px(12.0))
                            .gap(px(8.0))
                            .child(
                                Button::new("first-profile-new")
                                    .primary()
                                    .h(px(32.0))
                                    .icon(IconName::FilePlus)
                                    .label("Add profile")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        Self::open_new_profile_dialog(create.clone(), window, cx);
                                    }),
                            )
                            .child(
                                Button::new("first-profile-import")
                                    .secondary()
                                    .h(px(32.0))
                                    .icon(IconName::Link)
                                    .label("Import from URL")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        Self::open_import_url_dialog(import.clone(), window, cx);
                                    }),
                            )
                            .child(
                                Button::new("first-profile-import-link")
                                    .secondary()
                                    .h(px(32.0))
                                    .icon(IconName::ExternalLink)
                                    .label("Import from share link")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        Self::open_import_link_dialog(
                                            import_link.clone(),
                                            window,
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            );
        }

        content.child(self.render_library(store, cx))
    }

    fn render_connection(&self, cx: &Context<Self>) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        // Read everything needed up front, then drop the borrow: the disconnect
        // button builder needs `&mut self`/`&mut cx` while constructing.
        let store = self.store.read(cx);
        let snap = &store.snapshot;
        let visual = store.tray_visual_state();
        let label = store.status_label();
        let tone = if store.is_probing() {
            cx.theme().muted_foreground
        } else {
            match visual {
                TrayVisualState::Transitioning => cx.theme().warning,
                TrayVisualState::Broken => cx.theme().danger,
                TrayVisualState::Connected => crate::theme::success_text(cx),
                TrayVisualState::Disconnected => cx.theme().muted_foreground,
            }
        };
        let dot = if store.is_probing() {
            tone
        } else {
            match visual {
                TrayVisualState::Connected => cx.theme().success,
                _ => tone,
            }
        };
        let active = snap.active.as_ref().filter(|_| snap.running).cloned();
        let activity = store.busy_phase.as_ref().cloned().or(active).or_else(|| {
            (!snap.running)
                .then(|| store.selected_profile.clone())
                .flatten()
        });
        let issue = (visual == TrayVisualState::Broken)
            .then(|| {
                store
                    .init_error
                    .as_deref()
                    .or(snap.broken.as_deref())
                    .filter(|detail| !detail.is_empty())
                    .unwrap_or(snap.health.detail.as_str())
            })
            .filter(|issue| !issue.is_empty());
        let ip = if store.ip_loading {
            "Checking…".to_owned()
        } else if store.ip_error.is_some() {
            "Lookup failed".to_owned()
        } else {
            store
                .ip_info
                .as_ref()
                .map(|i| i.ip.clone())
                .unwrap_or_else(|| "Not checked".into())
        };
        let ip_details = if store.ip_loading {
            None
        } else {
            store.ip_error.as_ref()
        };
        let ip_to_copy = store
            .ip_info
            .as_ref()
            .filter(|_| !store.ip_loading && store.ip_error.is_none())
            .map(|info| (info.ip.clone(), info.formatted_location()));
        let busy = store.is_busy();
        let show_logs = self.store.clone();
        let refresh = self.store.clone();
        let restart = self.store.clone();

        let connected = snap.running && visual == TrayVisualState::Connected;
        let broken = visual == TrayVisualState::Broken;

        v_flex()
            .w_full()
            .flex_none()
            .rounded(px(10.0))
            .border_1()
            .border_color(cx.theme().border)
            .bg(crate::theme::surface(cx))
            .p(px(14.0))
            .gap(px(8.0))
            // A 3px accent rule on the left edge carries the state colour. A
            // full border tinted green read as a glowing frame around the
            // whole card; a rule reads as a status marker and leaves the card
            // itself neutral. It sits beside the content in one row, not
            // above it, so it spans the card's height as a vertical bar.
            .child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("connection-accent")
                            .w(px(3.0))
                            .flex_none()
                            .rounded_full()
                            .bg(if connected {
                                cx.theme().success
                            } else if broken {
                                cx.theme().danger
                            } else if visual == TrayVisualState::Transitioning {
                                cx.theme().warning
                            } else {
                                cx.theme().muted_foreground.opacity(0.4)
                            }),
                    )
                    // LEFT: Status & Profile Identity
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                h_flex()
                                    .size(px(38.0))
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.0))
                                    .bg(if connected {
                                        cx.theme().success.opacity(0.12)
                                    } else if broken {
                                        cx.theme().danger.opacity(0.12)
                                    } else if visual == TrayVisualState::Transitioning {
                                        cx.theme().warning.opacity(0.12)
                                    } else {
                                        cx.theme().muted.opacity(0.6)
                                    })
                                    .items_center()
                                    .justify_center()
                                    .child({
                                        let icon = Icon::new(if connected {
                                            IconName::CircleCheck
                                        } else if broken {
                                            IconName::TriangleAlert
                                        } else if visual == TrayVisualState::Transitioning {
                                            IconName::RotateCw
                                        } else {
                                            IconName::PowerOff
                                        })
                                        .size(px(18.0))
                                        .text_color(tone);
                                        // While the service starts or stops, the working
                                        // glyph spins once per second.
                                        if visual == TrayVisualState::Transitioning
                                            && !store.is_probing()
                                        {
                                            use gpui_kit::linear;
                                            use gpui_kit::{Animation, AnimationExt as _, radians};
                                            use std::time::Duration;
                                            icon.with_animation(
                                                gpui_kit::ElementId::Name("conn-icon-spin".into()),
                                                Animation::new(Duration::from_secs(1))
                                                    .repeat()
                                                    .with_easing(linear),
                                                |this, delta| {
                                                    this.rotate(radians(
                                                        delta * std::f32::consts::TAU,
                                                    ))
                                                },
                                            )
                                            .into_any_element()
                                        } else {
                                            icon.into_any_element()
                                        }
                                    }),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(fonts::EMPHASIS)
                                            .text_color(cx.theme().foreground)
                                            .truncate()
                                            .child(activity.unwrap_or_else(|| {
                                                if connected {
                                                    "Active profile".to_string()
                                                } else {
                                                    "No active profile".to_string()
                                                }
                                            })),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .size(px(6.0))
                                                    .flex_none()
                                                    .rounded_full()
                                                    .bg(dot),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(fonts::EMPHASIS)
                                                    .text_color(tone)
                                                    .child(label),
                                            )
                                            .when(connected, |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child("· sing-box active"),
                                                )
                                            })
                                            .when(
                                                !snap.running
                                                    && visual == TrayVisualState::Disconnected,
                                                |row| {
                                                    row.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("· stopped"),
                                                    )
                                                },
                                            ),
                                    ),
                            ),
                    )
                    // RIGHT: Telemetry & Actions
                    .child(
                        h_flex()
                            .ml_auto()
                            .flex_none()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(10.0))
                                    .h(px(32.0))
                                    .rounded(px(6.0))
                                    .bg(cx.theme().muted.opacity(0.5))
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        Icon::new(IconName::Globe)
                                            .small()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .font_family(fonts::mono_family())
                                            .text_xs()
                                            .text_color(cx.theme().foreground)
                                            .child(ip),
                                    )
                                    .when_some(ip_to_copy, |pill, (ip, _)| {
                                        pill.child(
                                            Button::new("copy-ip")
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::Copy)
                                                .accessibility_label("Copy public IP")
                                                .on_click(move |_, _, cx| {
                                                    cx.write_to_clipboard(
                                                        gpui_kit::ClipboardItem::new_string(
                                                            ip.clone(),
                                                        ),
                                                    );
                                                }),
                                        )
                                    })
                                    .child(
                                        Button::new("refresh-network")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::RefreshCw)
                                            .accessibility_label("Refresh connection and public IP")
                                            .disabled(store.ip_loading)
                                            .on_click(move |_, _, cx| {
                                                refresh.update(cx, |s, cx| {
                                                    s.refresh_snapshot(true, cx);
                                                    s.refresh_ip(cx);
                                                });
                                            }),
                                    ),
                            )
                            .when(snap.running, |row| {
                                row.child(self.render_disconnect_button(cx))
                            })
                            .when(broken, |row| {
                                row.child(
                                    Button::new("connection-logs")
                                        .secondary()
                                        .h(px(32.0))
                                        .flex_none()
                                        .icon(IconName::ScrollText)
                                        .label("View logs")
                                        .on_click(move |_, _, cx| {
                                            show_logs.update(cx, |s, cx| {
                                                s.set_tab(crate::actions::NavTab::Logs, cx)
                                            });
                                        }),
                                )
                            })
                            .when(snap.running, |row| {
                                row.child(
                                    Button::new("connection-options")
                                        .ghost()
                                        .h(px(32.0))
                                        .w(px(32.0))
                                        .flex_none()
                                        .icon(IconName::Ellipsis)
                                        .accessibility_label("Connection options")
                                        .dropdown_menu(move |menu, _, _| {
                                            let restart = restart.clone();
                                            menu.item(
                                                PopupMenuItem::new("Restart service")
                                                    .icon(IconName::RotateCw)
                                                    .disabled(busy)
                                                    .on_click(move |_, _, cx| {
                                                        restart.update(cx, |s, cx| {
                                                            s.restart_active(
                                                                NoticeOrigin::Stage,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                        }),
                                )
                            }),
                    ),
            )
            .when_some(issue, |strip, reason| {
                strip.child(connection_error("service-error", reason, cx))
            })
            .when_some(ip_details, |strip, reason| {
                strip.child(connection_error("ip-error", reason, cx))
            })
    }

    /// The Disconnect button with a subtle hover animation: on hover its fill
    /// deepens by one step and it eases to 97% scale, so the affordance reads
    /// as motion rather than a snap. The animation re-keys on hover state so
    /// entering and leaving each play once.
    fn render_disconnect_button(&self, cx: &Context<Self>) -> impl IntoElement {
        use gpui_kit::{Animation, AnimationExt as _};
        use std::time::Duration;

        let hovered = self.disconnect_hover.read(cx).value > 0.5;
        let busy = self.store.read(cx).is_busy();
        let disconnect = self.store.clone();
        let hover_spring = self.disconnect_hover.clone();

        let base_bg = cx.theme().secondary;

        let mut btn = Button::new("connection-disconnect")
            .secondary()
            .h(px(32.0))
            .flex_none()
            .icon(IconName::PowerOff)
            .label("Disconnect")
            .disabled(busy);
        btn = btn.on_click(move |_, _, cx| {
            disconnect.update(cx, |s, cx| s.disconnect(NoticeOrigin::Stage, cx));
        });

        div()
            .id("connection-disconnect-wrap")
            .on_hover(move |hovered, _window, cx| {
                let target = if *hovered { 1.0 } else { 0.0 };
                hover_spring.update(cx, |spring, cx| {
                    spring.value = target;
                    cx.notify();
                });
            })
            .child(btn.with_animation(
                gpui_kit::ElementId::NamedInteger("disconnect-hover".into(), hovered as u64),
                Animation::new(Duration::from_millis(140)).with_easing(gpui_kit::ease_in_out),
                move |this, delta| this.bg(base_bg.opacity(1.0 - 0.25 * delta)),
            ))
    }

    fn render_add_actions(&self, store: &AppStore) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let create = self.store.clone();
        let import = self.store.clone();
        let import_link = self.store.clone();
        let sync = self.store.clone();
        let busy = store.is_busy();
        let has_remote = store
            .snapshot
            .profiles
            .iter()
            .any(|p| p.source_url.is_some());

        h_flex()
            .gap(px(8.0))
            .items_center()
            .when(has_remote, |row| {
                row.child(
                    Button::new("library-sync-all")
                        .ghost()
                        .h(px(32.0))
                        .icon(IconName::CloudDownload)
                        .label("Sync remote profiles")
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            sync.update(cx, |s, cx| s.sync_all(NoticeOrigin::Stage, cx));
                        }),
                )
            })
            .child(
                Button::new("library-add")
                    .secondary()
                    .h(px(32.0))
                    .icon(IconName::CirclePlus)
                    .label("Add profile")
                    .disabled(busy)
                    .dropdown_menu(move |menu, _, _| {
                        let create = create.clone();
                        let import = import.clone();
                        let import_link = import_link.clone();
                        menu.item(
                            PopupMenuItem::new("New profile…")
                                .icon(IconName::FilePlus)
                                .on_click(move |_, window, cx| {
                                    Self::open_new_profile_dialog(create.clone(), window, cx);
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Import from URL…")
                                .icon(IconName::Link)
                                .on_click(move |_, window, cx| {
                                    Self::open_import_url_dialog(import.clone(), window, cx);
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Import from share link…")
                                .icon(IconName::ExternalLink)
                                .on_click(move |_, window, cx| {
                                    Self::open_import_link_dialog(import_link.clone(), window, cx);
                                }),
                        )
                    }),
            )
    }

    fn render_library(&self, store: &AppStore, cx: &Context<Self>) -> impl IntoElement {
        let query = store.profile_search_query.trim();
        let profiles = store
            .snapshot
            .profiles
            .iter()
            .filter(|p| {
                contains_ignore_ascii_case(&p.name, query)
                    || p.facts
                        .endpoint
                        .as_deref()
                        .is_some_and(|server| contains_ignore_ascii_case(server, query))
            })
            .collect::<Vec<_>>();
        let none = profiles.is_empty();

        v_flex()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(10.0))
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .gap(px(8.0))
                    .justify_between()
                    .items_center()
                    .child(
                        div().w(px(260.0)).child(
                            Input::new(&self.search_input)
                                .h(px(32.0))
                                .cleanable(true)
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(self.render_add_actions(store)),
            )
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(crate::theme::surface(cx))
                    .overflow_hidden()
                    .child(
                        profile_columns(cx)
                            .h(px(32.0))
                            .flex_none()
                            .bg(cx.theme().muted.opacity(0.4))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .font_weight(fonts::EMPHASIS)
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(220.0))
                                    .pl(px(32.0))
                                    .child("PROFILE"),
                            )
                            .child(div().w(px(200.0)).flex_none().child("SERVER / ENDPOINT"))
                            .child(div().w(px(90.0)).flex_none().child("LATENCY"))
                            .child(div().w(px(110.0)).flex_none().child("STATUS"))
                            .child(div().w(px(150.0)).flex_none()),
                    )
                    .child(
                        v_flex()
                            .id("profile-library")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .when(none, |list| {
                                let empty = store.snapshot.profiles.is_empty();
                                list.child(
                                    div()
                                        .p(px(16.0))
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if empty {
                                            "Looking for profiles…"
                                        } else {
                                            "No matching profiles"
                                        }),
                                )
                            })
                            .children(profiles.iter().enumerate().map(|(row, profile)| {
                                self.render_profile(profile, row, store, cx)
                            })),
                    ),
            )
    }

    fn render_profile(
        &self,
        profile: &Profile,
        row: usize,
        store: &AppStore,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let running = store.snapshot.running;
        let live = running && store.snapshot.active.as_deref() == Some(profile.name.as_str());
        let selected = store.selected_profile.as_deref() == Some(profile.name.as_str());
        let busy = store.is_busy();
        // Only this row's button spins: a repeating spinner per row kept the
        // whole list repainting at display rate for the duration of any
        // single operation.
        let row_busy = busy && store.busy_target.as_deref() == Some(profile.name.as_str());
        let ready = profile.ready();
        let expanded = self.expanded_profile.as_deref() == Some(profile.name.as_str());
        let app = self.store.clone();
        let activation_name = profile.name.clone();
        let edit_name = profile.name.clone();
        let detail_name = profile.name.clone();
        let edit = self.store.clone();
        let select = self.store.clone();
        let endpoint = profile.facts.endpoint.clone();
        let (status, color) = if live {
            ("Connected", crate::theme::success_text(cx))
        } else if !ready {
            ("Needs setup", cx.theme().warning)
        } else {
            ("Ready", cx.theme().muted_foreground)
        };

        v_flex()
            .w_full()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.6))
            .bg(if live {
                cx.theme().success.opacity(0.06)
            } else if selected {
                cx.theme().muted.opacity(0.4)
            } else {
                gpui_kit::Hsla::transparent_black()
            })
            .child(
                profile_columns(cx)
                    .min_h(px(44.0))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(220.0))
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                Button::new(gpui_kit::ElementId::NamedInteger(
                                    "expand".into(),
                                    row as u64,
                                ))
                                .ghost()
                                .xsmall()
                                .icon(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .toggled(expanded)
                                .accessibility_label(format!("{} details", profile.name))
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        view.expanded_profile = if view.expanded_profile.as_ref()
                                            == Some(&detail_name)
                                        {
                                            None
                                        } else {
                                            Some(detail_name.clone())
                                        };
                                        select.update(cx, |s, cx| {
                                            s.select_profile(Some(detail_name.clone()), cx)
                                        });
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Icon::new(if live {
                                    IconName::Route
                                } else if ready {
                                    IconName::Waypoints
                                } else {
                                    IconName::Cable
                                })
                                .small()
                                .text_color(color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(fonts::EMPHASIS)
                                    .text_color(cx.theme().foreground)
                                    .truncate()
                                    .child(profile.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .flex_none()
                            .font_family(fonts::mono_family())
                            .text_size(px(12.0))
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(endpoint.unwrap_or_else(|| "—".into())),
                    )
                    .child(self.render_latency_cell(profile, row, store, busy, cx))
                    .child(
                        h_flex()
                            .w(px(110.0))
                            .flex_none()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().size(px(6.0)).flex_none().rounded_full().bg(color))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(fonts::EMPHASIS)
                                    .text_color(color)
                                    .child(status),
                            ),
                    )
                    .child(
                        h_flex()
                            .w(px(150.0))
                            .flex_none()
                            .justify_end()
                            .gap(px(4.0))
                            .when(ready && !live, |actions| {
                                actions.child(
                                    Button::new(gpui_kit::ElementId::NamedInteger(
                                        "connect".into(),
                                        row as u64,
                                    ))
                                    .h(px(26.0))
                                    .when(selected, Button::primary)
                                    .when(!selected, Button::secondary)
                                    .label(if running { "Switch" } else { "Connect" })
                                    .loading(row_busy)
                                    .disabled(busy)
                                    .on_click(
                                        move |_, _, cx| {
                                            app.update(cx, |s, cx| {
                                                s.activate_profile(
                                                    activation_name.clone(),
                                                    NoticeOrigin::Stage,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                            })
                            .child(
                                Button::new(gpui_kit::ElementId::NamedInteger(
                                    "edit".into(),
                                    row as u64,
                                ))
                                .h(px(26.0))
                                .when(ready, Button::ghost)
                                .when(!ready, Button::secondary)
                                .label(if ready { "Edit" } else { "Finish setup" })
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    edit.update(cx, |s, cx| {
                                        s.select_profile(Some(edit_name.clone()), cx);
                                        s.begin_edit(cx);
                                    });
                                }),
                            )
                            .child(self.render_profile_menu(profile, row, live, busy)),
                    ),
            )
            .when(expanded, |row| {
                row.child(self.render_profile_details(profile, store, live, cx))
            })
    }

    /// The LATENCY column cell: a small ghost button that runs the probe, and
    /// the reading beside it. A probe never touches the running daemon, so it
    /// is not disabled by `busy`; it is only disabled while that profile's own
    /// probe is already in flight.
    fn render_latency_cell(
        &self,
        profile: &Profile,
        row: usize,
        store: &AppStore,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let name = profile.name.clone();
        let testable = profile.config_parsed && profile.facts.endpoint.is_some();
        let reading: Option<&LatencyReading> = store.latency_results.get(&name);
        let stale = reading.is_some_and(|r| r.since_disk_revision != store.disk_revision);
        let probing = store.latency_probing(&name);

        let (text, color) = if probing {
            ("Testing…".to_string(), cx.theme().muted_foreground)
        } else if stale {
            ("—".to_string(), cx.theme().muted_foreground.opacity(0.6))
        } else {
            match reading.map(|r| &r.result) {
                Some(Ok(elapsed)) => (format_latency(*elapsed), crate::theme::success_text(cx)),
                Some(Err(message)) => (truncate_label(message, 18), cx.theme().danger),
                None => ("—".to_string(), cx.theme().muted_foreground.opacity(0.6)),
            }
        };

        let test = self.store.clone();
        let test_name = name.clone();
        h_flex()
            .w(px(90.0))
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .child(
                Button::new(gpui_kit::ElementId::NamedInteger(
                    "latency".into(),
                    row as u64,
                ))
                .ghost()
                .xsmall()
                .icon(IconName::Gauge)
                .accessibility_label(format!("Test latency for {}", profile.name))
                .loading(probing)
                .disabled(probing || !testable || busy)
                .on_click(move |_, _, cx| {
                    test.update(cx, |s, cx| s.test_profile_latency(test_name.clone(), cx));
                }),
            )
            .child(
                div()
                    .font_family(fonts::mono_family())
                    .text_xs()
                    .truncate()
                    .text_color(color)
                    .child(text),
            )
    }

    fn render_profile_details(
        &self,
        profile: &Profile,
        store: &AppStore,
        live: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let facts = &profile.facts;
        // An unreadable file has no facts to report; `tun_state()` would say
        // "Absent" and print "No TUN inbound" right above "not JSON".
        let tunnel = if !profile.config_parsed {
            "—".to_string()
        } else {
            match facts.tun_state() {
                singswitch_core::profile::TunState::Named => {
                    facts.tun_interface.clone().unwrap_or_default()
                }
                singswitch_core::profile::TunState::Unnamed => "OS default".into(),
                singswitch_core::profile::TunState::Absent => "No TUN inbound".into(),
            }
        };

        v_flex()
            .w_full()
            .px(px(24.0))
            .pb(px(6.0))
            .when(!profile.config_parsed, |panel| {
                panel.child(detail_row(
                    "Configuration",
                    "Could not read as JSON".into(),
                    cx,
                ))
            })
            .when(!profile.missing.is_empty(), |panel| {
                let fields = profile
                    .missing
                    .iter()
                    .map(|field| field.label())
                    .collect::<Vec<_>>();
                panel.child(detail_row("Required fields", fields.join(", "), cx))
            })
            .when(profile.config_parsed, |panel| {
                panel.when_some(facts.outbound_type.as_ref(), |panel, protocol| {
                    panel.child(detail_row_with_icon(
                        "Protocol",
                        protocol.clone(),
                        IconName::Cable,
                        cx,
                    ))
                })
            })
            .child(detail_row_with_icon("Tunnel", tunnel, IconName::Route, cx))
            .child(detail_row_with_icon(
                "Config file",
                profile.path.display().to_string(),
                IconName::FileText,
                cx,
            ))
            .when_some(profile.source_url.as_ref(), |panel, url| {
                panel.child(detail_row("Source URL", url.clone(), cx))
            })
            .when(live, |panel| {
                let health = &store.snapshot.health;
                panel
                    .when_some(store.connected_since, |panel, since| {
                        panel.child(detail_row(
                            "Connected for",
                            format_duration(since.elapsed().as_secs()),
                            cx,
                        ))
                    })
                    .when_some(health.pid, |panel, pid| {
                        panel.child(detail_row("Process ID", pid.to_string(), cx))
                    })
                    .when_some(health.restarts.filter(|n| *n > 0), |panel, count| {
                        panel.child(detail_row("Restarts", count.to_string(), cx))
                    })
                    .when_some(health.last_exit.as_ref(), |panel, code| {
                        panel.child(detail_row("Last exit", code.clone(), cx))
                    })
            })
    }

    fn render_profile_menu(
        &self,
        profile: &Profile,
        row: usize,
        is_live: bool,
        busy: bool,
    ) -> impl IntoElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let app = self.store.clone();
        let name = profile.name.clone();
        let path = profile.path.display().to_string();
        let has_source = profile.source_url.is_some();

        Button::new(gpui_kit::ElementId::NamedInteger(
            "profile-options".into(),
            row as u64,
        ))
        .ghost()
        .h(px(26.0))
        .w(px(26.0))
        .icon(IconName::Ellipsis)
        .accessibility_label("Profile options")
        .dropdown_menu(move |menu, _, _| {
            let sync = app.clone();
            let delete = app.clone();
            let sync_name = name.clone();
            let delete_name = name.clone();
            let path = path.clone();

            menu.when(has_source, |menu| {
                menu.item(
                    PopupMenuItem::new("Sync from source")
                        .icon(IconName::CloudDownload)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            sync.update(cx, |s, cx| {
                                s.sync_profile(sync_name.clone(), NoticeOrigin::Stage, cx)
                            });
                        }),
                )
            })
            .item(
                PopupMenuItem::new("Copy config path")
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("Delete profile…")
                    .icon(IconName::Trash)
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        open_delete_dialog(
                            delete_name.clone(),
                            is_live,
                            NoticeOrigin::Stage,
                            delete.clone(),
                            window,
                            cx,
                        );
                    }),
            )
        })
    }
}

fn profile_columns(cx: &App) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .px(px(14.0))
        .gap(px(10.0))
        .items_center()
        .text_color(cx.theme().foreground)
}

fn connection_error(id: &'static str, reason: &str, cx: &App) -> impl IntoElement {
    div()
        .id(id)
        .px(px(8.0))
        .pb(px(4.0))
        .max_h(px(42.0))
        .overflow_y_scroll()
        .text_xs()
        .text_color(cx.theme().danger)
        .child(reason.to_owned())
}

fn detail_row(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    detail_row_inner(label, value, None, cx)
}

/// Same row, with a glyph that names what the value is. The icon sits in a
/// fixed-width slot so labels stay aligned with the iconless rows above.
fn detail_row_with_icon(
    label: &'static str,
    value: String,
    icon: IconName,
    cx: &App,
) -> impl IntoElement {
    detail_row_inner(label, value, Some(icon), cx)
}

fn detail_row_inner(
    label: &'static str,
    value: String,
    icon: Option<IconName>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .py(px(4.0))
        .gap(px(10.0))
        .items_baseline()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .w(px(100.0))
                .flex_none()
                .gap(px(6.0))
                .items_center()
                .when_some(icon, |row, icon| {
                    row.child(
                        Icon::new(icon)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .truncate()
                .child(value),
        )
}

/// Cap an error string for the narrow LATENCY cell, with an ellipsis when cut.
fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Deleting the expanded profile leaves only its name behind. Creating a
        // profile with that name later would then open its details on arrival.
        // Only pays a store read while a row is actually expanded.
        let expansion_gone = self.expanded_profile.as_deref().is_some_and(|name| {
            !self
                .store
                .read(cx)
                .snapshot
                .profiles
                .iter()
                .any(|p| p.name == name)
        });
        if expansion_gone {
            self.expanded_profile = None;
        }

        let s = self.store.read(cx);
        if s.editing {
            v_flex()
                .size_full()
                .px(px(14.0))
                .py(px(10.0))
                .child(self.editor.clone())
                .into_any_element()
        } else {
            self.render_connections(cx).into_any_element()
        }
    }
}

/// Confirm deleting whichever profile is selected. Used by the keyboard action so
/// the warning about stopping a running service is never skipped.
pub fn confirm_delete_selected(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
    let (name, is_live) = {
        let s = store.read(cx);
        let Some(name) = s.selected_profile.clone() else {
            return;
        };
        let live = s.snapshot.running && s.snapshot.active.as_deref() == Some(name.as_str());
        (name, live)
    };
    // The dialog is opened from the editor or the stage; report back to the
    // surface the user is actually looking at.
    let origin = if store.read(cx).editing {
        NoticeOrigin::Editor
    } else {
        NoticeOrigin::Stage
    };
    open_delete_dialog(name, is_live, origin, store, window, cx);
}

fn open_delete_dialog(
    name: String,
    is_connected: bool,
    origin: NoticeOrigin,
    store: Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let n_ok = name.clone();
        let s_ok = store.clone();
        let desc = if is_connected {
            format!("{name} is running. Deleting it stops sing-box first.")
        } else {
            format!("Delete {name}.json and any saved source URL?")
        };
        alert
            .confirm()
            .title(format!("Delete {name}"))
            .description(desc)
            .ok_text("Delete")
            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                // `force` is never derived from the cached snapshot: a stale
                // snapshot would delete a running profile without the warning
                // this dialog exists to give. Core decides, and its refusal
                // comes back as a notice.
                s_ok.update(cx, |s, cx| {
                    s.delete_profile(n_ok.clone(), false, origin, cx);
                });
                true
            })
    });
}
