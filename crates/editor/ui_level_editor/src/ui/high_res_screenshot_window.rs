//! Small configuration window for high-resolution viewport captures.

use std::sync::Arc;

use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, NumberInput},
    v_flex, ActiveTheme, Selectable as _, Size, TitleBar,
};

use crate::LevelEditorState;

pub struct HighResScreenshotWindow {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    multiplier: Entity<InputState>,
    mode: CaptureMode,
    message: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Pixels,
    Multiplier,
}

impl HighResScreenshotWindow {
    pub fn open(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut App) {
        use ui_common::PulsarWindowExt as _;
        Self::open_additional(state, cx);
    }

    fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let width = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value("1920", window, cx);
            input
        });
        let height = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value("1080", window, cx);
            input
        });
        let multiplier = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value("2", window, cx);
            input
        });
        Self {
            state,
            width,
            height,
            multiplier,
            mode: CaptureMode::Pixels,
            message: None,
        }
    }

    fn capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let max_dimension = state.read().build.max_texture_dimension_2d.max(1);
        let dims = match self.mode {
            CaptureMode::Pixels => {
                let width = self.width.read(cx).text().to_string().parse::<u32>().ok();
                let height = self.height.read(cx).text().to_string().parse::<u32>().ok();
                width.zip(height)
            }
            CaptureMode::Multiplier => {
                let multiplier = self
                    .multiplier
                    .read(cx)
                    .text()
                    .to_string()
                    .parse::<f32>()
                    .ok();
                multiplier.filter(|m| m.is_finite() && *m > 0.0).map(|m| {
                    let size = state.read().build.viewport_size;
                    (
                        (size.0 as f32 * m).round() as u32,
                        (size.1 as f32 * m).round() as u32,
                    )
                })
            }
        };
        match dims {
            Some((width, height)) if width > 0 && height > 0 => {
                let mut state = state.write();
                if state.build.screenshot_processing {
                    self.message = Some("A screenshot is already processing.".into());
                } else {
                    state.build.pending_screenshot_size = Some((width, height));
                    state.build.screenshot_processing = true;
                    let tile_side = 8192u32.min(max_dimension).max(1);
                    let columns = width.div_ceil(tile_side);
                    let rows = height.div_ceil(tile_side);
                    self.message = Some(format!(
                        "Rendering {width} × {height} in {} tiles. Scene time is frozen during capture.",
                        u64::from(columns) * u64::from(rows)
                    ));
                }
            }
            Some(_) => self.message = Some("Resolution must be greater than zero.".into()),
            None => self.message = Some("Enter a valid positive resolution or multiplier.".into()),
        }
        cx.notify();
        window.refresh();
    }
}

impl Render for HighResScreenshotWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.state.read().build.viewport_size;
        let aspect = if size.1 > 0 {
            format!("{:.3}:1", size.0 as f32 / size.1 as f32)
        } else {
            "—".into()
        };
        let this = cx.entity();
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(div().px_3().text_sm().child("High res screenshot")))
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("shot-pixels")
                                    .label("Fixed pixels")
                                    .selected(self.mode == CaptureMode::Pixels)
                                    .on_click({
                                        let this = this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |this, cx| {
                                                this.mode = CaptureMode::Pixels;
                                                cx.notify();
                                            })
                                        }
                                    }),
                            )
                            .child(
                                Button::new("shot-scale")
                                    .label("Viewport multiplier")
                                    .selected(self.mode == CaptureMode::Multiplier)
                                    .on_click({
                                        let this = this.clone();
                                        move |_, _, cx| {
                                            this.update(cx, |this, cx| {
                                                this.mode = CaptureMode::Multiplier;
                                                cx.notify();
                                            })
                                        }
                                    }),
                            ),
                    )
                    .child(if self.mode == CaptureMode::Pixels {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child("Width"))
                            .child(NumberInput::new(&self.width).size(Size::Small).w(px(100.0)))
                            .child(div().text_sm().child("Height"))
                            .child(
                                NumberInput::new(&self.height)
                                    .size(Size::Small)
                                    .w(px(100.0)),
                            )
                            .into_any_element()
                    } else {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child("Scale"))
                            .child(
                                NumberInput::new(&self.multiplier)
                                    .size(Size::Small)
                                    .w(px(100.0)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "× current viewport ({}×{}, {})",
                                        size.0, size.1, aspect
                                    )),
                            )
                            .into_any_element()
                    })
                    .child(if self.mode == CaptureMode::Pixels {
                        let width = self
                            .width
                            .read(cx)
                            .text()
                            .to_string()
                            .parse::<u32>()
                            .unwrap_or(0);
                        let height = self
                            .height
                            .read(cx)
                            .text()
                            .to_string()
                            .parse::<u32>()
                            .unwrap_or(0);
                        let aspect = if height > 0 {
                            format!("Aspect ratio: {:.3}:1", width as f32 / height as f32)
                        } else {
                            "Enter a valid resolution".into()
                        };
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(aspect)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(
                        Button::new("capture-high-res")
                            .label("Capture screenshot")
                            .primary()
                            .on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| this.capture(window, cx))
                            }),
                    )
                    .when(self.message.is_some(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.message.clone().unwrap_or_default()),
                        )
                    }),
            )
    }
}

impl window_manager::PulsarWindow for HighResScreenshotWindow {
    type Params = Arc<parking_lot::RwLock<LevelEditorState>>;
    fn window_name() -> &'static str {
        "HighResScreenshotWindow"
    }
    fn window_options(_: &Self::Params) -> WindowOptions {
        window_manager::default_window_options(440.0, 230.0)
    }
    fn build(state: Self::Params, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(state, window, cx))
    }
}
