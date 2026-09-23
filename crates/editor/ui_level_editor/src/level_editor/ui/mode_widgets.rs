//! Rendering for a [`ToolMode`](crate::level_editor::tool_modes::ToolMode)'s
//! declarative [`ToolWidget`] list in the horizontal toolbar strip
//! (`ui/toolbar/mod.rs`).

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Disableable, IconName, Sizable,
};

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::dispatcher::{ToolModeDispatcher, ToolWidgetEdit};
use crate::level_editor::tool_modes::{
    CameraFrame, ToolModeContext, ToolWidget, ViewportFrame,
};

/// The active tool mode's `toolbar_controls()`, or `None` if it returned
/// nothing (the toolbar treats that as "render nothing").
pub fn active_mode_widgets(
    state: &LevelEditorState,
    gpu_engine: &Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
) -> Vec<ToolWidget> {
    let mut state_clone = state.clone();
    let ctx = ToolModeContext {
        state: &mut state_clone,
        gpu_engine,
        camera: CameraFrame::default(),
        viewport: ViewportFrame::default(),
    };
    state
        .editor
        .tool_mode_registry
        .selected()
        .toolbar_controls(&ctx)
}

/// Render one mode's widget list as a toolbar strip.
///
/// Returns an empty, zero-size element when `controls` is empty so callers
/// can render this unconditionally without their own emptiness check.
pub fn render_mode_widgets<V>(
    controls: Vec<ToolWidget>,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    _gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    if controls.is_empty() {
        return div().into_any_element();
    }

    let theme = cx.theme();
    let mut container = h_flex().gap_2().items_center();

    for widget in controls {
        match widget {
            ToolWidget::Segmented {
                id,
                options,
                selected,
            } => {
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
            ToolWidget::Slider {
                id,
                label_key,
                value,
                min,
                max,
                step,
            } => {
                let state_clone_dec = state_arc.clone();
                let state_clone_inc = state_arc.clone();
                let dec_val = (value - step).clamp(min, max);
                let inc_val = (value + step).clamp(min, max);

                let mut slider_widget = h_flex().gap_1().items_center();

                slider_widget = slider_widget
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{}:", t!(label_key))),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
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
                                            &ToolWidgetEdit::SetSlider { id, value: dec_val },
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
                                            &ToolWidgetEdit::SetSlider { id, value: inc_val },
                                        );
                                    }),
                            ),
                    );

                container = container.child(slider_widget);
            }
            ToolWidget::Toggle { id, label_key, on } => {
                let state_clone = state_arc.clone();
                let mut btn = Button::new(id)
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
                let mut btn = Button::new(id)
                    .icon(IconName::Plus)
                    .label(t!(label_key))
                    .small()
                    .ghost()
                    .on_click(move |_, _, _| {
                        let mut st = state_clone.write();
                        ToolModeDispatcher::dispatch_widget_edit(&mut st, &ToolWidgetEdit::Invoke { id });
                    });
                container = container.child(btn);
            }
            ToolWidget::Divider => {
                container = container.child(div().h_5().w_px().bg(theme.border.opacity(0.4)));
            }
        }
    }

    container.into_any_element()
}
