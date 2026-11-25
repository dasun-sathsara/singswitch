//! Window-level "Nodes" strip for Clash-style `selector` groups.
//!
//! Shows the live selection of every `selector`/`urltest` outbound the active
//! config declares, and switches a selector to another member through the
//! profile's own experimental Clash API. Nothing here writes config: a switch is
//! a runtime call to the daemon, so profiles without `experimental.clash_api`
//! stay exactly as they were synced.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};

use singswitch_core::clash_groups::{
    ClashEndpoint, GroupInfo, GroupKind, fetch_groups, find_groups, merge_live, read_config,
    select_group_member,
};

use crate::actions::{NavTab, SwitchGroup};
use crate::fonts;
use crate::state::{AppStore, BannerTone, NoticeOrigin};

/// What the loaded group list describes. A reload only runs when this changes,
/// or when the window comes back into focus.
type NodesKey = (Option<String>, bool, u64);

pub struct NodesStrip {
    store: Entity<AppStore>,
    groups: Vec<GroupInfo>,
    endpoint: Option<ClashEndpoint>,
    /// Why the live selection is missing, when the daemon was expected to answer.
    live_error: Option<String>,
    loaded: Option<NodesKey>,
    task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl NodesStrip {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut strip = Self {
            store: store.clone(),
            groups: Vec::new(),
            endpoint: None,
            live_error: None,
            loaded: None,
            task: None,
            _subs: Vec::new(),
        };

        // Cadence, without a timer: the groups are re-read when the state they
        // describe changes (active profile, service running, config on disk) and
        // whenever the window is focused again. A selector's selection is only
        // changed by this app, and a switch reloads on completion.
        let store_observer = cx.observe(&store, |strip: &mut Self, _, cx| strip.reload(false, cx));
        strip._subs.push(store_observer);
        let activation = cx.observe_window_activation(window, |strip: &mut Self, window, cx| {
            if window.is_window_active() {
                strip.reload(true, cx);
            }
        });
        strip._subs.push(activation);

        // Window-level dispatch for this action is registered once in
        // `main.rs::register_navigation_actions` against the app-global main
        // view, not here: registering per-strip leaked one permanent global
        // listener per window open/close cycle (App::on_action has no removal),
        // and dispatch had to walk every stale closure.

        strip.reload(true, cx);
        strip
    }

    /// Refresh the group list unless the inputs it depends on are unchanged.
    fn reload(&mut self, force: bool, cx: &mut Context<Self>) {
        let (key, active_config) = {
            let store = self.store.read(cx);
            let key = (
                store.snapshot.active.clone(),
                store.snapshot.running,
                store.disk_revision,
            );
            (key, store.layout().active_config_path.clone())
        };
        let running = key.1;
        if !force && self.loaded.as_ref() == Some(&key) {
            return;
        }
        if !force && self.loaded.is_none() && key.0.is_none() && !running {
            // Nothing activated yet and the first probe has not answered.
            self.loaded = Some(key);
            return;
        }
        self.loaded = Some(key.clone());

        self.task = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let (declared, endpoint) = cx
                    .background_executor()
                    .spawn(async move {
                        let config = read_config(&active_config).ok();
                        let declared = config.as_ref().map(find_groups).unwrap_or_default();
                        let endpoint = config.as_ref().and_then(ClashEndpoint::from_config);
                        (declared, endpoint)
                    })
                    .await;

                // The Clash API only answers while sing-box runs, and only for configs
                // that opted in; both are checked here rather than in the fetch so a
                // read-only profile costs no connection attempt.
                let live = match (endpoint.clone(), running) {
                    (Some(endpoint), true) => cx
                        .background_executor()
                        .spawn(async move {
                            fetch_groups(&endpoint.controller, endpoint.secret.as_deref())
                        })
                        .await
                        .map_err(|err| describe(&err)),
                    _ => Ok(Vec::new()),
                };

                let _ = this.update(cx, |strip, cx| {
                    if strip.loaded.as_ref() != Some(&key) {
                        // A newer load already described a different state.
                        return;
                    }
                    match live {
                        Ok(live) => {
                            strip.groups = merge_live(declared, &live);
                            strip.live_error = None;
                        }
                        Err(err) => {
                            strip.groups = declared;
                            strip.live_error = Some(err);
                        }
                    }
                    strip.endpoint = endpoint;
                    cx.notify();
                });
            }),
        );
    }

    pub fn start_switch(&mut self, group: String, member: String, cx: &mut Context<Self>) {
        let Some(endpoint) = self.endpoint.clone() else {
            return;
        };
        let store = self.store.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (group_for_worker, member_for_worker) = (group.clone(), member.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    select_group_member(
                        &endpoint.controller,
                        endpoint.secret.as_deref(),
                        &group_for_worker,
                        &member_for_worker,
                    )
                })
                .await;

            let _ = this.update(cx, |strip, cx| {
                match &result {
                    // No success banner: the group chip re-reads the daemon and
                    // shows the new member itself.
                    Ok(()) => {}
                    Err(err) => store.update(cx, |s, cx| {
                        s.set_notice(NoticeOrigin::Toolbar, BannerTone::Error, describe(err), cx);
                    }),
                }
                // The daemon is the authority on the new selection; ask it rather
                // than assuming the switch landed as asked.
                strip.reload(true, cx);
            });
        })
        .detach();
    }

    fn render_group(&self, group: &GroupInfo, running: bool, cx: &App) -> AnyElement {
        let label = match &group.selected {
            Some(selected) => format!("{} · {selected}", group.tag),
            None => group.tag.clone(),
        };

        match group.kind {
            GroupKind::Urltest => {
                // Picked by the daemon's own probes, so there is nothing to
                // click. Same chip family as the selector below: one surface,
                // one border, one height — the kind is carried by the icon,
                // not by a different fill that read as selected-vs-not.
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(24.0))
                    .px(px(8.0))
                    .rounded_full()
                    .bg(crate::theme::subtle_pill_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_size(px(12.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::Gauge).small())
                    .child(div().max_w(px(180.0)).truncate().child(label))
                    .into_any_element()
            }
            GroupKind::Selector => {
                let tag = group.tag.clone();
                let members = group.members.clone();
                let selected = group.selected.clone();
                Button::new(SharedString::from(format!("node-group-{tag}")))
                    .ghost()
                    .h(px(24.0))
                    .px(px(8.0))
                    .gap(px(4.0))
                    .rounded_full()
                    .bg(crate::theme::subtle_pill_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_size(px(12.0))
                    .text_color(cx.theme().muted_foreground)
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu.scrollable(true).max_h(px(320.0));
                        for member in &members {
                            let checked = selected.as_deref() == Some(member.as_str());
                            let action = SwitchGroup {
                                group: tag.clone(),
                                member: member.clone(),
                            };
                            menu = menu.item(
                                PopupMenuItem::new(member.clone())
                                    .checked(checked)
                                    // Without a running daemon the API is not
                                    // listening, so the item would only produce
                                    // an avoidable error.
                                    .disabled(!running)
                                    .on_click(move |_, window, cx| {
                                        window.dispatch_action(Box::new(action.clone()), cx);
                                    }),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }
        }
    }
}

impl Render for NodesStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let visible =
            !self.groups.is_empty() && store.selected_tab == NavTab::Profiles && !store.editing;
        let running = store.snapshot.running;

        // A hidden strip must occupy exactly nothing, or it pushes the stage
        // down a few pixels on every profile screen even when no group exists.
        if !visible {
            return div().h_0().w_full();
        }

        h_flex()
            .w_full()
            .items_center()
            .gap(px(6.0))
            .px(px(14.0))
            // Symmetric: a bottom-only padding sat the chips above the band's
            // midline.
            .py(px(8.0))
            .child(
                div()
                    .flex_none()
                    .pr(px(2.0))
                    .text_size(px(11.0))
                    .font_weight(fonts::EMPHASIS)
                    .text_color(cx.theme().muted_foreground)
                    .child("Nodes"),
            )
            .children(
                self.groups
                    .iter()
                    .map(|group| self.render_group(group, running, cx)),
            )
            .when_some(self.live_error.clone(), |row, reason| {
                row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_none()
                        .max_w(px(360.0))
                        .truncate()
                        .pl(px(6.0))
                        .text_size(px(11.0))
                        .text_color(cx.theme().muted_foreground)
                        .child(reason),
                )
            })
    }
}

/// One line for the strip's inline error: the human fact first, internal
/// plumbing (URLs, errno text) kept out of the row. A refusal while the
/// service runs means the controller died or loopback is blocked, and the
/// fix is on the service side, not in this string.
fn describe(err: &singswitch_core::error::SingSwitchError) -> String {
    let refused = err
        .message
        .to_ascii_lowercase()
        .contains("connection refused");
    if refused {
        return "live selections unavailable: the controller is not answering".to_string();
    }
    let mut hint = err.hints.first().cloned().unwrap_or_default();
    // URLs and raw OS errors belong in the expandable detail, not the strip.
    if let Some((cut, _)) = hint.char_indices().nth(77) {
        hint = format!("{}…", &hint[..cut]);
    }
    format!("{} — {}", err.message, hint)
}
