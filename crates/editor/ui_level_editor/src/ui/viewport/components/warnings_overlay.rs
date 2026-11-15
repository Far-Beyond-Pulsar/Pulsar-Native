//! Virtualized viewport warnings, refreshed from the renderer's published diagnostics.

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::*;
use ui::{
    ActiveTheme, h_flex,
    scroll::{Scrollbar, ScrollbarState},
    v_flex,
};

use crate::ui::viewport::performance::RendererStatsSource;

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;

const REFRESH: Duration = Duration::from_millis(250);
const MIN_WIDTH: f32 = 300.0;
const MAX_WIDTH: f32 = 760.0;
const MIN_HEIGHT: f32 = 150.0;
const MAX_HEIGHT: f32 = 560.0;

pub struct WarningsOverlay {
    warnings: Rc<[String]>,
    scroll: UniformListScrollHandle,
    scrollbar: ScrollbarState,
    source: RendererStatsSource,
    width: f32,
    height: f32,
    resize_start: Option<(Point<Pixels>, f32, f32)>,
    _tick: Task<()>,
}

impl WarningsOverlay {
    pub fn new(gpu_engine: GpuEngine, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            warnings: Rc::from([]),
            scroll: UniformListScrollHandle::default(),
            scrollbar: ScrollbarState::default(),
            source: RendererStatsSource::new(gpu_engine),
            width: 410.0,
            height: 250.0,
            resize_start: None,
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

    fn begin_resize(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_start = Some((event.position, self.width, self.height));
        cx.stop_propagation();
    }

    fn resize(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((origin, width, height)) = self.resize_start {
            self.width =
                (width + (event.position.x - origin.x) / px(1.0)).clamp(MIN_WIDTH, MAX_WIDTH);
            self.height =
                (height + (event.position.y - origin.y) / px(1.0)).clamp(MIN_HEIGHT, MAX_HEIGHT);
            cx.notify();
        }
    }

    fn end_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_start = None;
        cx.notify();
    }
}

pub fn render_warnings_overlay<V>(
    gpu_engine: &GpuEngine,
    slot: &std::cell::RefCell<Option<Entity<WarningsOverlay>>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let engine = gpu_engine.clone();
            cx.new(|cx| WarningsOverlay::new(engine, cx))
        })
        .clone();
    div()
        .child(
            AnyView::from(overlay)
                .cached_auto_height(StyleRefinement::default().flex_shrink_0())
                .isolated(),
        )
        .into_any_element()
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
            .relative()
            .w(px(self.width))
            .h(px(self.height))
            .p_3()
            .gap_2()
            .bg(theme.background.opacity(0.88))
            .rounded_lg()
            .border_1()
            .border_color(border.opacity(0.5))
            .shadow_lg()
            .on_mouse_move(cx.listener(Self::resize))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_resize))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_resize))
            .child(h_flex().w_full().justify_between()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(foreground).child("Warnings"))
                .child(div().text_xs().text_color(if empty { muted } else { warning_color }).child(format!("{}", warnings.len()))))
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(if empty {
                div().flex_1().text_xs().text_color(muted).child("No active renderer warnings").into_any_element()
            } else {
                div().relative().flex_1().w_full().overflow_hidden().occlude()
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
            .child(div().absolute().right_1().bottom_1().w(px(14.0)).h(px(14.0))
                .cursor(CursorStyle::ResizeUpRightDownLeft)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_resize))
                .child(div().absolute().right_1().bottom_1().w(px(8.0)).h(px(8.0)).border_r_2().border_b_2().border_color(muted)))
    }
}
