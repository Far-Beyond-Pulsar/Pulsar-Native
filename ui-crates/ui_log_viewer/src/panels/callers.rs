//! Caller Sites panel — filterable, sortable virtual-list table of allocation call sites.

use std::rc::Rc;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, v_virtual_list, ActiveTheme, StyledExt,
    VirtualListScrollHandle,
    dock::{Panel, PanelEvent},
    input::{InputState, TextInput},
};
use crate::caller_tracking::{CALLER_SNAPSHOT, CallerRow, refresh_snapshot};
use crate::tracking_allocator::{is_tracking_active, enable_tracking, disable_tracking};

// ─── Sort state ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Symbol,
    Allocs,
    Deallocs,
    Live,
    Total,
    EstLeak,
}

// ─── Panel ────────────────────────────────────────────────────────────────────

pub struct CallerSitesPanel {
    focus_handle:  FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    filter_input:  Entity<InputState>,
    cached_rows:   Vec<CallerRow>,
    sort_col:      SortCol,
    sort_asc:      bool,
    _refresh_task: Task<()>,
}

impl CallerSitesPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by symbol…"));

        let refresh_task = cx.spawn(async move |this, cx| loop {
            smol::Timer::after(std::time::Duration::from_millis(500)).await;
            cx.background_executor()
                .spawn(async move { refresh_snapshot("") })
                .await;
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |_, cx| cx.notify());
                }
            });
        });

        Self {
            focus_handle:  cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
            filter_input,
            cached_rows:   Vec::new(),
            sort_col:      SortCol::EstLeak,
            sort_asc:      false,
            _refresh_task: refresh_task,
        }
    }

    fn set_sort(&mut self, col: SortCol, cx: &mut Context<Self>) {
        if self.sort_col == col {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_col = col;
            self.sort_asc = false;
        }
        self.apply_sort();
        cx.notify();
    }

    fn apply_sort(&mut self) {
        let asc = self.sort_asc;
        match self.sort_col {
            SortCol::Symbol   => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.symbol.cmp(&b.symbol); if asc { c } else { c.reverse() }
            }),
            SortCol::Allocs   => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.total_allocs.cmp(&b.total_allocs); if asc { c } else { c.reverse() }
            }),
            SortCol::Deallocs => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.total_deallocs.cmp(&b.total_deallocs); if asc { c } else { c.reverse() }
            }),
            SortCol::Live     => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.live_bytes.cmp(&b.live_bytes); if asc { c } else { c.reverse() }
            }),
            SortCol::Total    => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.total_bytes.cmp(&b.total_bytes); if asc { c } else { c.reverse() }
            }),
            SortCol::EstLeak  => self.cached_rows.sort_unstable_by(|a, b| {
                let c = a.leaked_estimate.cmp(&b.leaked_estimate); if asc { c } else { c.reverse() }
            }),
        }
    }

    fn render_col_header(
        &self,
        label: &str,
        col: SortCol,
        flex: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme      = cx.theme();
        let is_active  = self.sort_col == col;
        let is_asc     = self.sort_asc;
        let label_str  = if is_active {
            if is_asc { format!("{} ↑", label) } else { format!("{} ↓", label) }
        } else {
            label.to_string()
        };

        let color = if is_active { theme.accent } else { theme.muted_foreground };

        let base = div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.accent))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                this.set_sort(col, cx);
            }))
            .child(label_str);

        if flex { base.flex_1() } else { base.w(px(72.0)) }
    }

    fn fmt_bytes(bytes: u64) -> String {
        if bytes >= 1_073_741_824 { format!("{:.1}G", bytes as f64 / 1_073_741_824.0) }
        else if bytes >= 1_048_576 { format!("{:.1}M", bytes as f64 / 1_048_576.0) }
        else if bytes >= 1_024    { format!("{:.1}K", bytes as f64 / 1_024.0) }
        else                      { format!("{}B",   bytes) }
    }

    fn fmt_live(bytes: i64) -> String {
        if bytes < 0 { format!("-{}", Self::fmt_bytes((-bytes) as u64)) }
        else         { Self::fmt_bytes(bytes as u64) }
    }
}

impl EventEmitter<PanelEvent> for CallerSitesPanel {}

ui_common::panel_boilerplate!(CallerSitesPanel);

impl Panel for CallerSitesPanel {
    fn panel_name(&self) -> &'static str { "caller_sites" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "Callers".into_any_element() }
}

impl Render for CallerSitesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme       = cx.theme().clone();
        let filter_text = self.filter_input.read(cx).value().to_string();

        // True process-wide live bytes (all tracked allocs, regardless of CALLER_MAP cap or filter).
        let global_live = crate::caller_tracking::GLOBAL_LIVE_BYTES.load(std::sync::atomic::Ordering::Relaxed);

        // Refresh rows from snapshot and re-apply sort.
        {
            let snap = CALLER_SNAPSHOT.read();
            self.cached_rows = if filter_text.is_empty() {
                (*snap).clone()
            } else {
                let f = filter_text.to_lowercase();
                snap.iter().filter(|r| r.symbol.to_lowercase().contains(&f)).cloned().collect()
            };
        }
        self.apply_sort();

        let row_count  = self.cached_rows.len();

        // Compute column totals across all visible rows.
        let (tot_allocs, tot_deallocs, tot_live, tot_bytes, tot_leak) = self.cached_rows.iter().fold(
            (0u64, 0u64, 0i64, 0u64, 0u64),
            |(a, d, l, b, k), r| (
                a + r.total_allocs,
                d + r.total_deallocs,
                l + r.live_bytes,
                b + r.total_bytes,
                k + r.leaked_estimate,
            ),
        );

        let item_sizes = Rc::new(vec![size(px(0.0), px(28.0)); row_count]);
        let view       = cx.entity().clone();
        let cached_rows = self.cached_rows.clone();

        let live_total_color  = if tot_live  < 0 { theme.danger } else { theme.success };
        let leak_total_color  = if tot_leak  > 0 { theme.danger } else { theme.muted_foreground };

        // Check current tracking state
        let tracking_active = is_tracking_active();
        let (btn_text, btn_bg, btn_text_color) = if tracking_active {
            ("Tracking: ON", theme.success.opacity(0.2), theme.success)
        } else {
            ("Tracking: OFF", theme.muted.opacity(0.3), theme.muted_foreground)
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            // ── Toolbar ─────────────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full().px_3().py_2().gap_2().items_center()
                    .bg(theme.sidebar).border_b_1().border_color(theme.border)
                    .child(
                        div().text_sm().font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground).child("Allocation Call Sites")
                    )
                    .child(
                        div().px_2().py(px(2.0)).rounded(px(4.0))
                            .bg(theme.muted.opacity(0.5)).text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} sites", row_count))
                    )
                    .child(
                        div().px_2().py(px(2.0)).rounded(px(4.0))
                            .bg(theme.danger.opacity(0.15)).text_xs()
                            .text_color(theme.danger)
                            .child(format!("process live: {}", CallerSitesPanel::fmt_live(global_live)))
                    )
                    .child(
                        // Toggle button
                        div().px_3().py(px(4.0)).rounded(px(4.0))
                            .bg(btn_bg)
                            .text_xs().font_weight(FontWeight::SEMIBOLD)
                            .text_color(btn_text_color)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.accent.opacity(0.3)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _ev, _window, cx| {
                                if is_tracking_active() {
                                    disable_tracking();
                                } else {
                                    enable_tracking();
                                }
                                cx.notify();
                            }))
                            .child(btn_text)
                    )
                    .child(div().flex_1().child(TextInput::new(&self.filter_input)))
            )
            // ── Column headers (clickable) ────────────────────────────────────
            .child(
                h_flex()
                    .w_full().px_3().py_1().gap_2()
                    .bg(theme.sidebar).border_b_1().border_color(theme.border)
                    .child(self.render_col_header("Symbol / Location", SortCol::Symbol,   true,  cx))
                    .child(self.render_col_header("Allocs",            SortCol::Allocs,   false, cx))
                    .child(self.render_col_header("Deallocs",          SortCol::Deallocs, false, cx))
                    .child(self.render_col_header("Live",              SortCol::Live,     false, cx))
                    .child(self.render_col_header("Total",             SortCol::Total,    false, cx))
                    .child(self.render_col_header("Est.Leak",          SortCol::EstLeak,  false, cx))
            )
            // ── Totals row ────────────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full().px_3().py_1().gap_2().items_center()
                    .bg(theme.muted.opacity(0.15)).border_b_1().border_color(theme.border)
                    .child(
                        div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground).child("TOTAL")
                    )
                    .child(num_cell(format!("{}", tot_allocs),                              theme.muted_foreground))
                    .child(num_cell(format!("{}", tot_deallocs),                            theme.muted_foreground))
                    .child(num_cell(CallerSitesPanel::fmt_live(tot_live),                  live_total_color))
                    .child(num_cell(CallerSitesPanel::fmt_bytes(tot_bytes),                theme.warning))
                    .child(num_cell(CallerSitesPanel::fmt_bytes(tot_leak),                 leak_total_color))
            )
            // ── Virtual list ──────────────────────────────────────────────────
            .child(
                v_virtual_list(
                    view,
                    "caller-sites-list",
                    item_sizes,
                    move |_this, range, _window, cx| {
                        let theme = cx.theme().clone();
                        range.map(|ix| {
                            let Some(row) = cached_rows.get(ix) else {
                                return div().h(px(28.0)).into_any_element();
                            };
                            let row = row.clone();
                            let bg = if ix % 2 == 1 { theme.muted.opacity(0.06) }
                                     else { Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.0 } };
                            let live_color = if row.live_bytes < 0 { theme.danger } else { theme.success };
                            let leak_color = if row.leaked_estimate > 0 { theme.danger } else { theme.muted_foreground };

                            h_flex()
                                .w_full().h(px(28.0)).px_3().gap_2().items_center().bg(bg)
                                .hover(|s| s.bg(theme.accent.opacity(0.1)))
                                .child(
                                    div().flex_1().text_xs().text_color(theme.foreground)
                                        .overflow_hidden().text_ellipsis().child(row.symbol.clone())
                                )
                                .child(num_cell(format!("{}", row.total_allocs),   theme.muted_foreground))
                                .child(num_cell(format!("{}", row.total_deallocs), theme.muted_foreground))
                                .child(num_cell(CallerSitesPanel::fmt_live(row.live_bytes), live_color))
                                .child(num_cell(CallerSitesPanel::fmt_bytes(row.total_bytes), theme.warning))
                                .child(num_cell(CallerSitesPanel::fmt_bytes(row.leaked_estimate), leak_color))
                                .into_any_element()
                        }).collect()
                    },
                )
                .track_scroll(&self.scroll_handle)
            )
    }
}

fn num_cell(text: String, color: Hsla) -> impl IntoElement {
    div().w(px(72.0)).text_xs().text_color(color).text_ellipsis().overflow_hidden().child(text)
}
