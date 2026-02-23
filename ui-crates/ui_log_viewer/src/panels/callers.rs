//! Caller Sites panel — filterable virtual-list table of allocation call sites.

use std::rc::Rc;
use gpui::*;
use ui::{
    h_flex, v_flex, v_virtual_list, ActiveTheme, StyledExt,
    VirtualListScrollHandle,
    dock::{Panel, PanelEvent},
    input::{InputState, TextInput},
};
use crate::caller_tracking::{CALLER_SNAPSHOT, CallerRow, refresh_snapshot};

pub struct CallerSitesPanel {
    focus_handle:  FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    filter_input:  Entity<InputState>,
    cached_rows:   Vec<CallerRow>,
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
            _refresh_task: refresh_task,
        }
    }

    fn fmt_bytes(bytes: u64) -> String {
        if bytes >= 1_073_741_824 { format!("{:.1} GB", bytes as f64 / 1_073_741_824.0) }
        else if bytes >= 1_048_576 { format!("{:.1} MB", bytes as f64 / 1_048_576.0) }
        else if bytes >= 1_024    { format!("{:.1} KB", bytes as f64 / 1_024.0) }
        else                      { format!("{} B",    bytes) }
    }

    fn fmt_live(bytes: i64) -> String {
        if bytes < 0 { format!("-{}", Self::fmt_bytes((-bytes) as u64)) }
        else         { Self::fmt_bytes(bytes as u64) }
    }
}

impl EventEmitter<PanelEvent> for CallerSitesPanel {}

impl Focusable for CallerSitesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for CallerSitesPanel {
    fn panel_name(&self) -> &'static str { "caller_sites" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "Callers".into_any_element() }
}

impl Render for CallerSitesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let filter_text = self.filter_input.read(cx).value().to_string();

        // Refresh cached_rows from snapshot each frame.
        {
            let snap = CALLER_SNAPSHOT.read();
            self.cached_rows = if filter_text.is_empty() {
                (*snap).clone()
            } else {
                let f = filter_text.to_lowercase();
                snap.iter().filter(|r| r.symbol.to_lowercase().contains(&f)).cloned().collect()
            };
        }

        let row_count  = self.cached_rows.len();
        let row_height = px(28.0);
        let item_sizes = Rc::new(vec![size(px(0.0), row_height); row_count]);
        let view       = cx.entity().clone();

        // Clone data for move into virtual-list closure (follow memory.rs pattern).
        let cached_rows = self.cached_rows.clone();

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
                    .child(div().flex_1().child(TextInput::new(&self.filter_input)))
            )
            // ── Column headers ────────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full().px_3().py_1().gap_2()
                    .bg(theme.sidebar).border_b_1().border_color(theme.border)
                    .child(col_hdr("Symbol / Location", true,  theme.muted_foreground))
                    .child(col_hdr("Allocs",            false, theme.muted_foreground))
                    .child(col_hdr("Deallocs",          false, theme.muted_foreground))
                    .child(col_hdr("Live",              false, theme.muted_foreground))
                    .child(col_hdr("Total",             false, theme.muted_foreground))
                    .child(col_hdr("Est.Leak",          false, theme.danger))
            )
            // ── Virtual list (same pattern as memory.rs) ──────────────────────
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
                            let bg = if ix % 2 == 1 { theme.muted.opacity(0.08) }
                                     else { Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.0 } };
                            let live_color = if row.live_bytes < 0 { theme.danger } else { theme.success };

                            h_flex()
                                .w_full().h(px(28.0)).px_3().gap_2().items_center().bg(bg)
                                .hover(|s| s.bg(theme.accent.opacity(0.12)))
                                .child(
                                    div().flex_1().text_xs().text_color(theme.foreground)
                                        .overflow_hidden().text_ellipsis().child(row.symbol.clone())
                                )
                                .child(num_cell(format!("{}", row.total_allocs),   theme.muted_foreground))
                                .child(num_cell(format!("{}", row.total_deallocs), theme.muted_foreground))
                                .child(num_cell(CallerSitesPanel::fmt_live(row.live_bytes), live_color))
                                .child(num_cell(CallerSitesPanel::fmt_bytes(row.total_bytes), theme.warning))
                                .child(num_cell(CallerSitesPanel::fmt_bytes(row.leaked_estimate), theme.danger))
                                .into_any_element()
                        }).collect()
                    },
                )
                .track_scroll(&self.scroll_handle)
            )
    }
}

fn col_hdr(label: &str, flex: bool, color: Hsla) -> impl IntoElement {
    let b = div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(color).child(label.to_string());
    if flex { b.flex_1() } else { b.w(px(80.0)) }
}

fn num_cell(text: String, color: Hsla) -> impl IntoElement {
    div().w(px(80.0)).text_xs().text_color(color).text_ellipsis().overflow_hidden().child(text)
}
