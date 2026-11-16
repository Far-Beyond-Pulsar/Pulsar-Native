//! Virtualized viewport warnings, refreshed from the renderer's published diagnostics.
//!
//! The card is an RnD panel on the viewport's Panel Canvas: its header is the
//! drag handle, and the canvas's resize handles size it (the canvas tracks a
//! resize at the window, so it keeps going when the pointer leaves the card).

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::*;
use ui::{
    ActiveTheme, h_flex,
    panel_canvas::{DragHandle, HandleEdge, PanelRef, RndPanel},
    scroll::{Scrollbar, ScrollbarState},
    v_flex,
};

use crate::ui::viewport::performance::RendererStatsSource;

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;

const REFRESH: Duration = Duration::from_millis(250);

pub struct WarningsOverlay {
    warnings: Rc<[String]>,
    scroll: UniformListScrollHandle,
    scrollbar: ScrollbarState,
    source: RendererStatsSource,
    /// The card this overlay fills; its header moves it.
    panel: PanelRef,
    _tick: Task<()>,
}

impl WarningsOverlay {
    pub fn new(gpu_engine: GpuEngine, panel: PanelRef, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            warnings: Rc::from([]),
            scroll: UniformListScrollHandle::default(),
            scrollbar: ScrollbarState::default(),
            source: RendererStatsSource::new(gpu_engine),
            panel,
            _tick: Task::ready(()),
        };
        this.poll();
        this._tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                if this
                    .update(cx, |this, cx| {
                        if this.poll() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        this
    }

    fn poll(&mut self) -> bool {
        let next: Rc<[String]> = self
            .source
            .latest()
            .and_then(|stats| stats.details)
            .map(|details| Rc::from(details.warnings.clone()))
            .unwrap_or_else(|| Rc::from([]));
        if self.warnings == next {
            return false;
        }
        self.warnings = next;
        true
    }
}

/// The warnings card: a resizable RnD panel in the viewport's `left` slot.
pub fn render_warnings_overlay<V>(
    gpu_engine: &GpuEngine,
    panel: PanelRef,
    slot: &std::cell::RefCell<Option<Entity<WarningsOverlay>>>,
    cx: &mut Context<V>,
) -> RndPanel
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let engine = gpu_engine.clone();
            let panel = panel.clone();
            cx.new(|cx| WarningsOverlay::new(engine, panel, cx))
        })
        .clone();
    let theme = cx.theme();
    RndPanel::new(panel)
        .default_slot("left")
        .default_size(size(px(410.0), px(250.0)))
        .min_size(size(px(300.0), px(150.0)))
        .max_size(size(px(760.0), px(560.0)))
        .resizable([HandleEdge::Right, HandleEdge::Bottom, HandleEdge::BottomRight])
        .bg(theme.background.opacity(0.88))
        .rounded_lg()
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .shadow_lg()
        .child(AnyView::from(overlay).cached(StyleRefinement::default().size_full()).isolated())
}

impl Render for WarningsOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let warning_color = theme.warning_foreground;
        let warnings = self.warnings.clone();
        let empty = warnings.is_empty();
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .child(DragHandle::new(self.panel.clone()).no_grip().w_full().justify_between()
                .child(h_flex().gap_2().items_center()
                    .child(div().flex().flex_col().gap(px(2.0)).children((0..3).map(|_| div().w(px(10.0)).h(px(1.5)).bg(muted))))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(foreground).child("Warnings")))
                .child(div().text_xs().text_color(if empty { muted } else { warning_color }).child(format!("{}", warnings.len()))))
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(if empty {
                div().flex_1().text_xs().text_color(muted).child("No active renderer warnings").into_any_element()
            } else {
                div().relative().flex_1().min_h_0().w_full().overflow_hidden().occlude()
                    .child(uniform_list("viewport-warnings", warnings.len(), move |range, _, _| {
                        range.filter_map(|index| warnings.get(index)).map(|warning| {
                            div().w_full().px_2().py_1().text_xs().text_color(foreground)
                                .child(warning.clone())
                        }).collect()
                    }).track_scroll(&self.scroll).size_full())
                    .child(div().absolute().inset_0().child(Scrollbar::uniform_scroll(&self.scrollbar, &self.scroll)))
                    .into_any_element()
            })
            .child(div().w_full().text_xs().text_color(muted)
                .child("For lighting overloads, select Lighting: Tile Light Count in the viewport view-mode menu."))
    }
}
