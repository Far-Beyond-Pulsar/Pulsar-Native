//! Shared rendering for a [`ToolMode`](crate::level_editor::tool_modes::ToolMode)'s
//! declarative [`ToolWidget`] list.
//!
//! One rendering function, two call sites: the horizontal toolbar strip
//! (`ui/toolbar/tool_mode_controls.rs`, used when [`ModeLayout::show_mode_panel`]
//! is `false`) and the vertical left-hand mode-tools dock panel
//! (`workspace/panels/mode_tools.rs`, used when it's `true`). Both read the
//! *same* `toolbar_controls()` data — only the container's flex axis and the
//! divider element differ — so a mode's widgets can never drift between the
//! two layouts, and there is exactly one place that knows how to turn a
//! `ToolWidget` into GPUI elements.
//!
//! [`ModeLayout::show_mode_panel`]: crate::level_editor::tool_modes::ModeLayout::show_mode_panel

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable, IconName, Sizable,
};

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::dispatcher::{ToolModeDispatcher, ToolWidgetEdit};
use crate::level_editor::tool_modes::{CameraFrame, ToolModeContext, ToolWidget, ViewportFrame};

/// Which strip a mode's widgets are being rendered into. Only affects layout
/// (flex axis, divider orientation, alignment) — the widget-to-element
/// mapping itself is identical either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetLayout {
    /// The horizontal toolbar strip: compact, single row.
    Toolbar,
    /// A vertical left-hand dock panel: one widget per row, full width.
    Panel,
}

/// The active tool mode's `toolbar_controls()`, or `None` if it returned
/// nothing (both call sites treat that as "render nothing").
pub fn active_mode_widgets(
    state: &LevelEditorState,
    gpu_engine: &Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
) -> Vec<ToolWidget> {
    let mut state_clone = state.clone();
    let ctx = ToolModeContext {
        state: &mut state_clone,
        gpu_engine,
        // Widget data comes from editor state alone; no mode's
        // `toolbar_controls`/`status` impl reads `ctx.terrain` today (only
        // `on_pointer` does), so the toolbar/panel never need a live seam.
        terrain: None,
        camera: CameraFrame::default(),
        viewport: ViewportFrame::default(),
    };
    state
        .editor
        .tool_mode_registry
        .selected()
        .toolbar_controls(&ctx)
}

/// Render one mode's widget list as either a toolbar strip or a panel column.
///
/// Returns an empty, zero-size element when `controls` is empty so callers
/// can render this unconditionally without their own emptiness check.
pub fn render_mode_widgets<V>(
    controls: Vec<ToolWidget>,
    layout: WidgetLayout,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    if controls.is_empty() {
        return div().into_any_element();
    }

    let theme = cx.theme();
    let mut container = match layout {
        WidgetLayout::Toolbar => h_flex().gap_2().items_center(),
        WidgetLayout::Panel => v_flex().gap_3().items_start().w_full(),
    };

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
                if layout == WidgetLayout::Panel {
                    seg_group = seg_group.w_full().justify_center();
                }

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
                if layout == WidgetLayout::Panel {
                    slider_widget = slider_widget.w_full().justify_between();
                }

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
                if layout == WidgetLayout::Panel {
                    btn = btn.w_full();
                }

                let btn = if on { btn.primary() } else { btn.ghost() };
                container = container.child(btn);
            }
            ToolWidget::Action { id, label_key } => {
                let state_clone = state_arc.clone();
                // The terrain seam is fetched inside the click, not per
                // render: the same one-locked-pass shape `on_set_tool_mode`
                // already uses for a user action.
                let engine = gpu_engine.clone();
                let mut btn = Button::new(id)
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
                    });
                if layout == WidgetLayout::Panel {
                    btn = btn.w_full();
                }
                container = container.child(btn);
            }
            ToolWidget::Divider => {
                container = container.child(match layout {
                    WidgetLayout::Toolbar => div().h_5().w_px().bg(theme.border.opacity(0.4)),
                    WidgetLayout::Panel => div().w_full().h_px().bg(theme.border.opacity(0.4)),
                });
            }
        }
    }

    container.into_any_element()
}
