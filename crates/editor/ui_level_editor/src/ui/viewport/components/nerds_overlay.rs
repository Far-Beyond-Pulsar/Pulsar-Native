//! Live editor and renderer diagnostics for the viewport.
//!
//! The overlay owns its refresh timer and is mounted as an isolated cached
//! view. Sampling notifies only this view, so the viewport and its other
//! overlays remain untouched.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme,
};

use crate::{
    state::LevelEditorState,
    ui::viewport::{
        components::camera_selector::CameraSpeedControl, input_state::InputState,
        performance::EngineFrameSnapshot,
    },
};

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;
const REFRESH: Duration = Duration::from_millis(100);

pub struct NerdsOverlay {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: GpuEngine,
    input: Arc<InputState>,
    shown: [String; 11],
    _tick: Task<()>,
}

impl NerdsOverlay {
    fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: GpuEngine,
        input: Arc<InputState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            state,
            gpu_engine,
            input,
            shown: Default::default(),
            _tick: Task::ready(()),
        };
        this.sample();
        this._tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH).await;
            if this
                .update(cx, |this, cx| {
                    if this.sample() {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        });
        this
    }

    fn sample(&mut self) -> bool {
        let speed = self.input.get_move_speed();
        let forward = self
            .input
            .forward
            .load(std::sync::atomic::Ordering::Relaxed);
        let right = self.input.right.load(std::sync::atomic::Ordering::Relaxed);
        let vertical = self.input.up.load(std::sync::atomic::Ordering::Relaxed);
        let movement = format!(
            "{}{}{}",
            if forward > 0 {
                "W"
            } else if forward < 0 {
                "S"
            } else {
                ""
            },
            if right > 0 {
                "D"
            } else if right < 0 {
                "A"
            } else {
                ""
            },
            if vertical > 0 {
                "Space"
            } else if vertical < 0 {
                "Ctrl"
            } else {
                ""
            },
        );
        let movement = if movement.is_empty() {
            "Idle".to_owned()
        } else {
            movement
        };
        let boost = self.input.boost.load(std::sync::atomic::Ordering::Relaxed);
        let latency_ms = self.input.get_input_latency_us() as f64 / 1000.0;
        let camera_mode = format!("{:?}", self.state.read().editor.camera_mode);

        let mut next = self.shown.clone();
        next[0] = format!("{speed:.1} units/s{}", if boost { " (boost)" } else { "" });
        next[1] = format!("{movement}{}", if boost { " + boost" } else { "" });
        next[2] = format!("{latency_ms:.2} ms");
        next[3] = camera_mode;

        if let Some(snapshot) = EngineFrameSnapshot::read_stats(&self.gpu_engine) {
            next[4] = format!("{:.0}", snapshot.ui_fps);
            next[5] = format!("{:.0}", snapshot.helio_fps);
            next[6] = format!("{:.0}", snapshot.render_fps);
            next[7] = format!("{:.2} ms", snapshot.frame_time_ms);
            next[8] = format!("{:.0}", snapshot.draw_calls);
            next[9] = format!("{:.1}k", snapshot.vertices / 1000.0);
            next[10] = format!("{:.1} MB", snapshot.memory_mb);
        }

        let changed = next != self.shown;
        self.shown = next;
        changed
    }
}

impl Render for NerdsOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let close_state = self.state.clone();
        let lines = [
            ("Camera Speed", self.shown[0].clone()),
            ("Movement", self.shown[1].clone()),
            ("Input Latency", self.shown[2].clone()),
            ("Camera Mode", self.shown[3].clone()),
            ("UI FPS", self.shown[4].clone()),
            ("Helio FPS", self.shown[5].clone()),
            ("Render FPS", self.shown[6].clone()),
            ("Frame Time", self.shown[7].clone()),
            ("Draw Calls", self.shown[8].clone()),
            ("Vertices", self.shown[9].clone()),
            ("GPU Memory", self.shown[10].clone()),
        ];

        v_flex()
            .w(px(230.0))
            .gap_1()
            .p_2()
            .bg(theme.background.opacity(0.88))
            .rounded_lg()
            .border_1()
            .border_color(theme.border.opacity(0.5))
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Nerds"),
                    )
                    .child(
                        Button::new("close_nerds")
                            .icon(ui::IconName::Close)
                            .ghost()
                            .tooltip("Close")
                            .on_click(move |_, _, _| {
                                close_state.write().overlays.set_show_nerds_overlay(false);
                            }),
                    ),
            )
            .children(lines.into_iter().map(|(label, value)| {
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(value),
                    )
            }))
    }
}

/// Create the live diagnostics view only while the overlay is visible.
pub fn render_nerds_overlay<V>(
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &GpuEngine,
    input: Arc<InputState>,
    slot: &std::cell::RefCell<Option<Entity<NerdsOverlay>>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let gpu_engine = gpu_engine.clone();
            cx.new(|cx| NerdsOverlay::new(state, gpu_engine, input, cx))
        })
        .clone();
    AnyView::from(overlay)
        .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
        .isolated()
        .into_any_element()
}
