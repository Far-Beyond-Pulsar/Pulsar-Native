use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, IconName, Sizable, Disableable
};

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::dispatcher::{ToolModeDispatcher, ToolWidgetEdit};
use crate::level_editor::tool_modes::{CameraFrame, ToolModeContext, ToolWidget, ViewportFrame};

/// Renders mode-specific toolbar widgets provided by [`crate::level_editor::tool_modes::ToolMode::toolbar_controls`].
pub struct ToolModeControls;

impl ToolModeControls {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let mut state_clone = state.clone();
        let controls = {
            let ctx = ToolModeContext {
                state: &mut state_clone,
                gpu_engine: &gpu_engine,
                // Widget data comes from editor state alone.
                terrain: None,
                camera: CameraFrame::default(),
                viewport: ViewportFrame::default(),
            };
            state.editor.tool_mode_registry.selected().toolbar_controls(&ctx)
        };

        if controls.is_empty() {
            return h_flex().into_any_element();
        }

        let theme = cx.theme();
        let mut container = h_flex().gap_2().items_center();

        for widget in controls {
            match widget {
                ToolWidget::Segmented { id, options, selected } => {
                    let mut seg_group = h_flex()
                        .items_center()
                        .rounded(px(6.0))
                        .bg(theme.muted.opacity(0.1))
                        .p(px(2.0))
                        .gap_1();

                    for (opt_label_key, opt_value) in options {
                        let is_sel = opt_value == selected;
                        let state_clone = state_arc.clone();
                        let opt_val_static = opt_value;
                        let btn = Button::new(format!("{id}_{opt_value}"))
                            .label(t!(opt_label_key))
                            .small()
                            .on_click(move |_, _, _| {
                                let mut st = state_clone.write();
                                ToolModeDispatcher::dispatch_widget_edit(
                                    &mut st,
                                    &ToolWidgetEdit::SetSegmented {
                                        id,
                                        value: opt_val_static,
                                    },
                                );
                            });

                        let btn = if is_sel { btn.primary() } else { btn.ghost() };
                        seg_group = seg_group.child(btn);
                    }
                    container = container.child(seg_group);
                }
                ToolWidget::Slider { id, label_key, value, min, max, step } => {
                    let state_clone_dec = state_arc.clone();
                    let state_clone_inc = state_arc.clone();
                    let dec_val = (value - step).clamp(min, max);
                    let inc_val = (value + step).clamp(min, max);

                    let slider_widget = h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}:", t!(label_key))),
                        )
                        .child(
                            Button::new(format!("{id}_dec"))
                                .icon(IconName::Minus)
                                .small()
                                .ghost()
                                .disabled(value <= min)
                                .on_click(move |_, _, _| {
                                    let mut st = state_clone_dec.write();
                                    ToolModeDispatcher::dispatch_widget_edit(
                                        &mut st,
                                        &ToolWidgetEdit::SetSlider {
                                            id,
                                            value: dec_val,
                                        },
                                    );
                                }),
                        )
                        .child(
                            div()
                                .min_w(px(32.0))
                                .text_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(if step < 1.0 {
                                    format!("{:.1}", value)
                                } else {
                                    format!("{:.0}", value)
                                }),
                        )
                        .child(
                            Button::new(format!("{id}_inc"))
                                .icon(IconName::Plus)
                                .small()
                                .ghost()
                                .disabled(value >= max)
                                .on_click(move |_, _, _| {
                                    let mut st = state_clone_inc.write();
                                    ToolModeDispatcher::dispatch_widget_edit(
                                        &mut st,
                                        &ToolWidgetEdit::SetSlider {
                                            id,
                                            value: inc_val,
                                        },
                                    );
                                }),
                        );

                    container = container.child(slider_widget);
                }
                ToolWidget::Toggle { id, label_key, on } => {
                    let state_clone = state_arc.clone();
                    let btn = Button::new(id)
                        .label(t!(label_key))
                        .small()
                        .on_click(move |_, _, _| {
                            let mut st = state_clone.write();
                            ToolModeDispatcher::dispatch_widget_edit(
                                &mut st,
                                &ToolWidgetEdit::SetToggle { id, on: !on },
                            );
                        });

                    let btn = if on { btn.primary() } else { btn.ghost() };
                    container = container.child(btn);
                }
                ToolWidget::Action { id, label_key } => {
                    let state_clone = state_arc.clone();
                    // The terrain seam is fetched inside the click, not per
                    // render: the same one-locked-pass shape `on_set_tool_mode`
                    // already uses for a user action.
                    let engine = gpu_engine.clone();
                    container = container.child(
                        Button::new(id)
                            .icon(IconName::Plus)
                            .label(t!(label_key))
                            .small()
                            .ghost()
                            .on_click(move |_, _, _| {
                                let terrain = engine
                                    .lock()
                                    .ok()
                                    .and_then(|engine| engine.terrain_edit_api());
                                let mut st = state_clone.write();
                                ToolModeDispatcher::dispatch_widget_edit_with_terrain(
                                    &mut st,
                                    terrain.as_ref(),
                                    &ToolWidgetEdit::Invoke { id },
                                );
                            }),
                    );
                }
                ToolWidget::Divider => {
                    container = container.child(
                        div().h_5().w_px().bg(theme.border.opacity(0.4)),
                    );
                }
            }
        }

        container.into_any_element()
    }
}
