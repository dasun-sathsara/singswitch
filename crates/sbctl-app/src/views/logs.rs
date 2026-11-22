use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, VirtualListScrollHandle, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size as GpuiSize,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use sbctl_core::logs::{LogLevel, contains_ignore_ascii_case};

use crate::state::{AppStore, BannerTone, LogSeverityFilter, LogStreamKind, NoticeOrigin};
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
                .placeholder("Filter logs...")
                .default_value(store.read(cx).log_search_query.clone())
        });

        let store_sub = store.clone();
        let sub = cx.subscribe_in(&search_input, window, move |_, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let query = state.read(cx).value().to_string();
                store_sub.update(cx, |s, cx| {
                    s.log_search_query = query;
                    cx.notify();
                });
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
        // Reuse the existing size buffer when it is uniquely ours. This is one
        // entry per row and all entries are identical (rows are a fixed height),
        // so a fresh 5000-element Vec per poll was pure churn.
        let row = size(px(1400.0), px(22.0));
        match Rc::get_mut(&mut self.item_sizes) {
            Some(sizes) => {
                sizes.clear();
                sizes.resize(count, row);
            }
            None => self.item_sizes = Rc::new(vec![row; count]),
        }
        self.cached_revision = store.log_revision;
        self.cached_stream = store.log_stream;
        self.cached_severity = store.log_severity;
        self.cached_query = query.to_string();
    }

    /// Header: stream tabs, search, and follow. Tabs are underline, not pills;
    /// no state here is rendered as a rounded tag.
    fn render_tabs(
        &self,
        stream: LogStreamKind,
        follow: bool,
        count: usize,
        total: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let store_tab = self.store.clone();
        let store_follow = self.store.clone();

        v_flex()
            .w_full()
            .gap(px(10.0))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(14.0))
                    .items_center()
                    .child(
                        TabBar::new("logs-stream-tabs")
                            .underline()
                            .selected_index(match stream {
                                LogStreamKind::Error => 0,
                                LogStreamKind::Access => 1,
                            })
                            .child(Tab::new().label("Service"))
                            .child(Tab::new().label("Access"))
                            .on_click(cx.listener(move |this, idx, _, cx| {
                                let next = if *idx == 1 {
                                    LogStreamKind::Access
                                } else {
                                    LogStreamKind::Error
                                };
                                this.selected_id = None;
                                store_tab.update(cx, |s, cx| s.set_log_stream(next, cx));
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(240.0))
                            .child(Input::new(&self.search_input).small().cleanable(true)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.0))
                    .items_center()
                    .child(Switch::new("logs-follow").checked(follow).small().on_click(
                        move |checked, _, cx| {
                            let next = *checked;
                            store_follow.update(cx, |s, cx| s.set_log_follow(next, cx));
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(cx.theme().foreground)
                            .child(if follow { "Following" } else { "Paused" }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(crate::fonts::mono_family())
                            .text_size(px(12.0))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{count} of {total}")),
                    ),
            )
    }

    /// Actions only. Search and follow live in the tab row, so this bar stays
    /// three buttons and one menu and never wraps at 960px.
    fn render_controls_bar(
        &self,
        count: usize,
        total_count: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let store_clear = self.store.clone();
        let store_reload = self.store.clone();
        let store_copy = self.store.clone();
        let store_sev = self.store.clone();
        let filtered_for_copy = self.filtered_indices.clone();
        let severity = self.store.read(cx).log_severity;

        h_flex()
            .gap(px(8.0))
            .items_center()
            .child(
                Button::new("logs-severity")
                    .secondary()
                    .small()
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
                                            s.log_severity = filter;
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("logs-clear")
                    .ghost()
                    .small()
                    .label("Clear view")
                    .disabled(total_count == 0)
                    .tooltip("Clears this view only; the log file is untouched")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_id = None;
                        store_clear.update(cx, AppStore::clear_logs);
                    })),
            )
            .child(
                Button::new("logs-reload")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .tooltip("Reload from disk")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_id = None;
                        store_reload.update(cx, |s, cx| {
                            s.reload_logs();
                            cx.notify();
                        });
                    })),
            )
            .child(
                Button::new("logs-copy")
                    .ghost()
                    .small()
                    .icon(IconName::Copy)
                    .tooltip("Copy the filtered lines")
                    .disabled(count == 0)
                    .on_click(move |_, _, cx| {
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
                        store_copy.update(cx, |s, cx| {
                            s.set_notice(
                                NoticeOrigin::Logs,
                                BannerTone::Success,
                                "Copied logs",
                                cx,
                            );
                        });
                    }),
            )
    }

    /// Detail for the selected line, docked inside the log frame and scrolling
    /// on its own so a very long line never pushes the list off screen.
    fn render_selected_line_panel(
        &self,
        line: SharedString,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let line_copy = line.clone();
        let store_banner = self.store.clone();

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
                                store_banner.update(cx, |s, cx| {
                                    s.set_notice(
                                        NoticeOrigin::Logs,
                                        BannerTone::Success,
                                        "Line copied",
                                        cx,
                                    );
                                });
                            }),
                    )
                    .child(
                        Button::new("log-dismiss-line")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Dismiss")
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
            self.selected_id
                .and_then(|id| records.iter().find(|r| r.id == id))
                .map(|r| r.line.clone())
        };
        let count = self.filtered_indices.len();
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
                    .items_start()
                    .gap(px(16.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(4.0))
                            .child(div().text_size(px(19.0)).font_bold().child("Logs"))
                            .child(
                                div()
                                    .font_family(crate::fonts::mono_family())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(log_path.display().to_string()),
                            ),
                    )
                    .child(self.render_controls_bar(count, total_count, cx)),
            )
            .when_some(
                crate::views::notice_strip(&self.store, NoticeOrigin::Logs, cx),
                |el, notice| el.child(div().mt(px(10.0)).child(notice)),
            )
            .child(div().mt(px(8.0)).child(self.render_tabs(
                stream,
                follow,
                count,
                total_count,
                cx,
            )))
            .child(
                div()
                    .mt(px(6.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(crate::theme::surface(cx))
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
                    .id(SharedString::from(format!("log-row-{rec_idx}")))
                    .w_full()
                    .h(px(22.0))
                    .px(px(10.0))
                    .gap(px(10.0))
                    .items_center()
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(hover_bg))
                    .hover(|d| d.bg(hover_bg))
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
                            .font_bold()
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

#[cfg(test)]
mod filter_tests {
    use super::filter_prefix_reusable;

    #[test]
    fn reuse_only_while_the_prefix_is_stable() {
        // Steady append: reuse.
        assert!(filter_prefix_reusable(false, Some(1), Some(1), 5, 3));
        // Eviction moved the front: rebuild.
        assert!(!filter_prefix_reusable(false, Some(1), Some(2), 5, 5));
        // Filters changed: rebuild.
        assert!(!filter_prefix_reusable(true, Some(1), Some(1), 5, 3));
        // Empty buffer has no trustworthy prefix.
        assert!(!filter_prefix_reusable(false, None, None, 0, 0));
        // Shrank below what was scanned (truncation): rebuild.
        assert!(!filter_prefix_reusable(false, Some(1), Some(1), 2, 7));
        // Fresh cache mid-stream (first draw): no cached front, so rebuild.
        // `scanned` is 0, so the rebuild starts from the first record anyway.
        assert!(!filter_prefix_reusable(false, None, Some(5), 4, 0));
    }
}
