use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size as GpuiSize,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use singswitch_core::logs::{LogLevel, contains_ignore_ascii_case};

use crate::fonts;
use crate::state::{AppStore, LogSeverityFilter, LogStreamKind, NoticeOrigin};
use crate::theme::subtle_pill_bg;

pub struct LogsView {
    store: Entity<AppStore>,
    search_input: Entity<InputState>,
    scroll_handle: VirtualListScrollHandle,
    selected_id: Option<u64>,
    /// Identity of the last record we scrolled to.
    ///
    /// Counting rows cannot work once the buffer is at its cap: each poll adds
    /// and drains the same number, so the count never changes and following
    /// silently stops while the switch still reads "Following".
    last_tail: Option<u64>,

    // Cached filtered row indices rebuilt only when logs, stream, severity, or search change.
    cached_revision: u64,
    cached_stream: LogStreamKind,
    cached_severity: LogSeverityFilter,
    cached_query: String,
    /// Records already tested against the current filters.
    scanned: usize,
    /// Id of the buffer's first record when `scanned` was recorded.
    cached_front: Option<u64>,
    filtered_indices: Rc<Vec<usize>>,
    item_sizes: Rc<Vec<GpuiSize<Pixels>>>,

    _sub: Subscription,
}

impl LogsView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter logs…")
                .default_value(store.read(cx).log_search_query.clone())
        });

        let store_sub = store.clone();
        let sub = cx.subscribe_in(&search_input, window, move |_, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let query = state.read(cx).value().to_string();
                store_sub.update(cx, |s, cx| s.set_log_search(query, cx));
            }
        });

        Self {
            store,
            search_input,
            scroll_handle: VirtualListScrollHandle::new(),
            selected_id: None,
            last_tail: None,
            cached_revision: u64::MAX,
            cached_stream: LogStreamKind::Error,
            cached_severity: LogSeverityFilter::All,
            cached_query: String::new(),
            scanned: 0,
            cached_front: None,
            filtered_indices: Rc::new(Vec::new()),
            item_sizes: Rc::new(Vec::new()),
            _sub: sub,
        }
    }

    fn ensure_filter_cache(&mut self, store: &AppStore) {
        let query = store.log_search_query.trim();
        if self.cached_revision == store.log_revision
            && self.cached_stream == store.log_stream
            && self.cached_severity == store.log_severity
            && self.cached_query == query
        {
            return;
        }

        let records = store.active_log_records();
        let filters_changed = self.cached_stream != store.log_stream
            || self.cached_severity != store.log_severity
            || self.cached_query != query;

        // A poll appends and leaves earlier records untouched, so only the new
        // tail needs testing. Rebuilding all 5000 indices on every poll was the
        // dominant cost of a live view. Reuse is only safe while the front of
        // the buffer has not moved: ids are monotonic, so a changed first id
        // means an eviction shifted every recorded index down by one.
        let front = records.first().map(|r| r.id);
        let reusable = filter_prefix_reusable(
            filters_changed,
            self.cached_front,
            front,
            records.len(),
            self.scanned,
        );
        let mut indices: Vec<usize> = if reusable {
            Rc::try_unwrap(std::mem::replace(
                &mut self.filtered_indices,
                Rc::new(Vec::new()),
            ))
            .unwrap_or_else(|rc| (*rc).clone())
        } else {
            Vec::new()
        };
        let from = if reusable {
            self.scanned.min(records.len())
        } else {
            0
        };
        if !reusable {
            indices.clear();
        }
        indices.extend(
            records
                .iter()
                .enumerate()
                .skip(from)
                .filter(|(_, rec)| store.log_severity.matches(rec.level))
                .filter(|(_, rec)| contains_ignore_ascii_case(rec.line.as_ref(), query))
                .map(|(idx, _)| idx),
        );

        let count = indices.len();
        self.scanned = records.len();
        self.cached_front = front;
        self.filtered_indices = Rc::new(indices);
        // The list keeps its own clone of this Vec across frames, so `get_mut`
        // could never succeed; rebuilding it is the only path.
        self.item_sizes = Rc::new(vec![size(px(1400.0), px(22.0)); count]);
        self.cached_revision = store.log_revision;
        self.cached_stream = store.log_stream;
        self.cached_severity = store.log_severity;
        self.cached_query = query.to_string();
    }

    fn render_severity_dropdown(&self, cx: &Context<Self>) -> impl IntoElement {
        let store_sev = self.store.clone();
        let severity = self.store.read(cx).log_severity;

        Button::new("logs-severity")
            .secondary()
            .h(px(32.0))
            // The caret separates this control from the static count pill two
            // slots right, which uses the same fill.
            .dropdown_caret(true)
            .label(match severity {
                LogSeverityFilter::All => "All levels",
                LogSeverityFilter::Error => "Errors only",
                LogSeverityFilter::Warn => "Warnings and up",
                LogSeverityFilter::Info => "Info and up",
            })
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, filter) in [
                    ("All levels", LogSeverityFilter::All),
                    ("Errors only", LogSeverityFilter::Error),
                    ("Warnings and up", LogSeverityFilter::Warn),
                    ("Info and up", LogSeverityFilter::Info),
                ] {
                    let st = store_sev.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .checked(severity == filter)
                            .on_click(move |_, _, cx| {
                                st.update(cx, |s, cx| {
                                    s.set_log_severity(filter, cx);
                                });
                            }),
                    );
                }
                menu
            })
    }

    /// Detail for the selected line, docked inside the log frame and scrolling
    /// on its own so a very long line never pushes the list off screen.
    fn render_selected_line_panel(
        &self,
        line: SharedString,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let line_copy = line.clone();

        v_flex()
            .w_full()
            .max_h(px(168.0))
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(cx.theme().muted_foreground)
                            .child("Selected line"),
                    )
                    .child(
                        Button::new("log-copy-line")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    line_copy.to_string(),
                                ));
                                // Clipboard writes need no confirmation strip.
                            }),
                    )
                    .child(
                        Button::new("log-dismiss-line")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .accessibility_label("Dismiss detail")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_id = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("log-detail-scroll")
                    .px(px(10.0))
                    .pb(px(10.0))
                    .overflow_y_scroll()
                    .font_family(crate::fonts::mono_family())
                    .text_size(px(12.5))
                    .child(line),
            )
    }
}

impl Render for LogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (stream, follow, log_path, log_error, is_busy, total_count) = {
            let s = self.store.read(cx);
            self.ensure_filter_cache(s);
            let layout = s.layout();
            let path = match s.log_stream {
                LogStreamKind::Error => layout.error_log_path.clone(),
                LogStreamKind::Access => layout.access_log_path.clone(),
            };
            (
                s.log_stream,
                s.log_follow,
                path,
                s.active_log_error().map(str::to_owned),
                s.busy_phase.is_some(),
                s.active_log_records().len(),
            )
        };

        // Only a permission failure is something `chmod` can fix; offering the
        // button for "is a directory" or "too many open files" is a dead end.
        let can_fix_permissions = log_error
            .as_deref()
            .is_some_and(|e| e.to_ascii_lowercase().contains("permission denied"));
        let records = {
            let s = self.store.read(cx);
            s.active_log_records()
        };
        let selected_detail = {
            let s = self.store.read(cx);
            let records = s.active_log_records();
            // Ids are monotonic, so the buffer is sorted by id: a binary search
            // instead of a linear scan over up to MAX_LOG_ENTRIES records on
            // every frame that keeps a row selected.
            self.selected_id.and_then(|id| {
                records
                    .binary_search_by_key(&id, |r| r.id)
                    .ok()
                    .map(|idx| records[idx].line.clone())
            })
        };
        let count = self.filtered_indices.len();
        // Rows present but hidden by the query or the level filter.
        let filtered_out = (count == 0 && total_count > 0).then_some(total_count);
        let store_reset = self.store.clone();
        let search_input_reset = self.search_input.clone();
        let tail_id = self
            .filtered_indices
            .last()
            .and_then(|ix| records.get(*ix))
            .map(|r| r.id);
        if follow && tail_id.is_some() && tail_id != self.last_tail && count > 0 {
            self.scroll_handle
                .scroll_to_item(count - 1, gpui_kit::ScrollStrategy::Bottom);
        }
        self.last_tail = tail_id;

        v_flex()
            .size_full()
            .px(px(14.0))
            .py(px(10.0))
            .gap(px(0.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .items_center()
                            .gap(px(10.0))
                            .child(div().text_size(px(16.0)).font_weight(fonts::EMPHASIS).child("Logs"))
                            .child(div().h(px(14.0)).w(px(1.0)).bg(cx.theme().border))
                            .child(
                                div()
                                    .font_family(crate::fonts::mono_family())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(log_path.display().to_string()),
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
                            .child(
                                Button::new("tab-service")
                                    .h(px(28.0))
                                    .when(stream == LogStreamKind::Error, Button::secondary)
                                    .when(stream != LogStreamKind::Error, Button::ghost)
                                    .label("Service")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.selected_id = None;
                                        this.store.update(cx, |s, cx| s.set_log_stream(LogStreamKind::Error, cx));
                                    })),
                            )
                            .child(
                                Button::new("tab-access")
                                    .h(px(28.0))
                                    .when(stream == LogStreamKind::Access, Button::secondary)
                                    .when(stream != LogStreamKind::Access, Button::ghost)
                                    .label("Access")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.selected_id = None;
                                        this.store.update(cx, |s, cx| s.set_log_stream(LogStreamKind::Access, cx));
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.0))
                    .mt(px(8.0))
                    .child(
                        div()
                            .w(px(220.0))
                            .child(
                                Input::new(&self.search_input)
                                    .h(px(32.0))
                                    .cleanable(true)
                                    .prefix(Icon::new(IconName::Search).small()),
                            ),
                    )
                    .child(self.render_severity_dropdown(cx))
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(Switch::new("logs-follow").checked(follow).small().on_click({
                                let store_follow = self.store.clone();
                                move |checked, _, cx| {
                                    let next = *checked;
                                    store_follow.update(cx, |s, cx| s.set_log_follow(next, cx));
                                }
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(fonts::EMPHASIS)
                                    .text_color(cx.theme().foreground)
                                    .child(if follow { "Following" } else { "Paused" }),
                            ),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .h(px(32.0))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(crate::theme::subtle_pill_bg(cx))
                            .font_family(crate::fonts::mono_family())
                            .text_size(px(11.5))
                            // "N of M" only when a filter is hiding something;
                            // at full view the bare count says everything the
                            // "of M" would.
                            .text_color(cx.theme().foreground.opacity(0.8))
                            .child(if count == total_count {
                                count.to_string()
                            } else {
                                format!("{count} of {total_count}")
                            }),
                    )
                    .child(
                        Button::new("logs-clear")
                            .ghost()
                            .h(px(32.0))
                            .label("Clear")
                            .accessibility_label("Clear view")
                            .disabled(total_count == 0)
                            .on_click({
                                let store_clear = self.store.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.selected_id = None;
                                    store_clear.update(cx, AppStore::clear_logs);
                                })
                            }),
                    )
                    .child(
                        Button::new("logs-reload")
                            .ghost()
                            .h(px(32.0))
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Reload from disk")
                            .on_click({
                                let store_reload = self.store.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.selected_id = None;
                                    store_reload.update(cx, |s, cx| {
                                        s.reload_logs();
                                        cx.notify();
                                    });
                                })
                            }),
                    )
                    .child(
                        Button::new("logs-copy")
                            .ghost()
                            .h(px(32.0))
                            .icon(IconName::Copy)
                            .accessibility_label("Copy filtered lines")
                            .disabled(count == 0)
                            .on_click({
                                let store_copy = self.store.clone();
                                let filtered_for_copy = self.filtered_indices.clone();
                                move |_, _, cx| {
                                    let joined = {
                                        let s = store_copy.read(cx);
                                        let recs = s.active_log_records();
                                        filtered_for_copy
                                            .iter()
                                            .filter_map(|&idx| recs.get(idx))
                                            .map(|r| r.line.as_ref())
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    };
                                    cx.write_to_clipboard(ClipboardItem::new_string(joined));
                                    // Clipboard writes need no confirmation strip.

                                }
                            }),
                    ),
            )
            .when_some(
                crate::views::notice_strip(&self.store, NoticeOrigin::Logs, cx),
                |el, notice| el.child(div().mt(px(8.0)).child(notice)),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(crate::theme::surface(cx))
                    .overflow_hidden()
                    .when(count > 0 && log_error.is_some(), |this| {
                        this.child(
                            h_flex()
                                .w_full()
                                .flex_none()
                                .px(px(10.0))
                                .py(px(6.0))
                                .gap(px(8.0))
                                .items_center()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().danger.opacity(0.08))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(11.5))
                                        .text_color(cx.theme().danger)
                                        .child(format!(
                                            "Reading this log failed; the lines below are stale. {}",
                                            log_error.clone().unwrap_or_default()
                                        )),
                                ),
                        )
                    })
                    .when(count == 0, |this| {
                        // Three distinct states, never collapsed into one line
                        // of copy: nothing written, nothing matching, unreadable.
                        let (title, detail) = match (&log_error, total_count) {
                            (Some(err), _) => ("Cannot read the log file", err.clone()),
                            (None, 0) => (
                                "Nothing logged yet.",
                                format!("{}", log_path.display()),
                            ),
                            (None, _) => (
                                "No lines match this filter.",
                                format!("{} line(s) hidden by the current search or level.", total_count),
                            ),
                        };
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap(px(6.0))
                                .px(px(24.0))
                                .child(div().text_sm().text_color(cx.theme().foreground).child(title))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(crate::fonts::mono_family())
                                        .text_color(cx.theme().muted_foreground)
                                        .child(detail),
                                )
                                // A filter that hides everything is a dead end
                                // without a way back; "Clear view" next to it
                                // empties the buffer instead of the query.
                                .when_some(filtered_out, |v, hidden| {
                                    let store = store_reset.clone();
                                    let input = search_input_reset.clone();
                                    v.child(
                                        Button::new("logs-reset-filters")
                                            .secondary()
                                            .small()
                                            .label(format!("Reset filters ({hidden} hidden)"))
                                            .on_click(move |_, window, cx| {
                                                input.update(cx, |s, cx| {
                                                    s.set_value("", window, cx)
                                                });
                                                store.update(cx, |s, cx| {
                                                    s.reset_log_filters(cx)
                                                });
                                            }),
                                    )
                                })
                                .when(can_fix_permissions && cfg!(target_os = "macos"), |v| {
                                    v.child(
                                        Button::new("logs-fix-perms")
                                            .secondary()
                                            .small()
                                            .label("Fix permissions…")
                                            .disabled(is_busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.store
                                    .update(cx, |s, cx| s.repair_macos_logs(NoticeOrigin::Logs, cx));
                                            })),
                                    )
                                }),
                        )
                    })
                    .when(count > 0, |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .min_h(px(0.0))
                                // Scrolling up to read an older line must not be
                                // undone by the next line sing-box writes.
                                .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                                    let up = match event.delta {
                                        gpui_kit::ScrollDelta::Lines(p) => p.y > 0.0,
                                        gpui_kit::ScrollDelta::Pixels(p) => p.y > px(0.0),
                                    };
                                    if up && this.store.read(cx).log_follow {
                                        let store = this.store.clone();
                                        store.update(cx, |s, cx| s.set_log_follow(false, cx));
                                    }
                                }))
                                .child(
                                    div().flex_1().min_h(px(0.0)).w_full().child(
                                        v_virtual_list(
                                            cx.entity(),
                                            "logs-virtual-list",
                                            self.item_sizes.clone(),
                                            move |this, range, _, cx| {
                                                render_log_rows(this, range, cx)
                                            },
                                        )
                                        .track_scroll(&self.scroll_handle)
                                        .size_full(),
                                    ),
                                )
                                .when_some(selected_detail, |v, line| {
                                    v.child(self.render_selected_line_panel(line, cx))
                                }),
                        )
                    }),
            )
    }
}

/// One log row: line number, a coloured level word (never a pill), and the
/// message truncated to one line. The detail pane carries the full text.
fn render_log_rows(
    this: &LogsView,
    range: std::ops::Range<usize>,
    cx: &Context<LogsView>,
) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
    let indices = this.filtered_indices.clone();
    let s = this.store.read(cx);
    let records = s.active_log_records();
    let mono = crate::fonts::mono_family();
    let fg = cx.theme().foreground;
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let warn = cx.theme().warning;
    let hover_bg = subtle_pill_bg(cx);
    let selected_ref = this.selected_id;

    range
        .filter_map(|vis_idx| {
            let rec_idx = *indices.get(vis_idx)?;
            let rec = records.get(rec_idx)?;
            let (lvl_text, lvl_color) = match rec.level {
                LogLevel::Error => ("ERR", danger),
                LogLevel::Warn => ("WRN", warn),
                LogLevel::Info => ("INF", muted),
                LogLevel::Debug => ("DBG", muted),
                LogLevel::Other => ("LOG", muted),
            };
            let is_selected = selected_ref == Some(rec.id);
            let rec_id = rec.id;

            Some(
                h_flex()
                    // Same element id as `"log-row-{rec_idx}"`, built without
                    // formatting a String for every visible row every frame.
                    .id(("log-row", rec_idx))
                    .w_full()
                    .h(px(22.0))
                    .px(px(10.0))
                    .gap(px(10.0))
                    .items_center()
                    .cursor_pointer()
                    // Selection and hover are different facts and must not
                    // paint the same: a selected row tells the detail pane's
                    // content, a hovered row is under no such claim.
                    .when(is_selected, |d| {
                        // The accent-tinted row plus the blue edge: neutral
                        // hover cannot be mistaken for a selection.
                        d.bg(crate::theme::selection_tint(cx))
                            .border_l_2()
                            .border_color(cx.theme().ring)
                    })
                    .when(!is_selected, |d| d.hover(|d| d.bg(hover_bg)))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.selected_id = Some(rec_id);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(42.0))
                            .flex_none()
                            .text_size(px(11.5))
                            .font_family(mono.clone())
                            .text_color(muted)
                            .child(format!("{:>4}", vis_idx + 1)),
                    )
                    .child(
                        div()
                            .w(px(34.0))
                            .flex_none()
                            .font_family(mono.clone())
                            .text_size(px(12.0))
                            .font_weight(fonts::EMPHASIS)
                            .text_color(lvl_color)
                            .child(lvl_text),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.5))
                            .font_family(mono.clone())
                            .text_color(fg)
                            .truncate()
                            .child(rec.line.clone()),
                    ),
            )
        })
        .collect()
}

/// Whether a cached filter prefix can be extended instead of rebuilt.
///
/// Reuse needs the same filters, a front record that has not moved (ids are
/// monotonic, so a changed first id means an eviction shifted every index), a
/// non-empty buffer, and a buffer that has not shrunk below what was scanned.
fn filter_prefix_reusable(
    filters_changed: bool,
    cached_front: Option<u64>,
    front: Option<u64>,
    len: usize,
    scanned: usize,
) -> bool {
    !filters_changed && cached_front == front && front.is_some() && len >= scanned
}
