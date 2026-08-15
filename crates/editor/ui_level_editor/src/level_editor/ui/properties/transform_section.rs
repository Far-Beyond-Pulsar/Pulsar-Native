//! Transform Section - Property panel section for editing object transforms
//!
//! This component provides a clean UI for editing position, rotation, and scale
//! of scene objects using the field binding system for automatic sync and undo/redo.

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable};

use super::super::bindings::bound_field::F32BoundField;
use crate::level_editor::core::commands::{execute_command, SceneCommand};
use crate::level_editor::scene_database::SceneDatabase;
use crate::level_editor::state::LevelEditorState;

#[derive(Clone, Copy)]
enum Axis {
    Position,
    Rotation,
    Scale,
}

/// Builds an `F32FieldBinding` for one axis (x/y/z) of one transform
/// component (position/rotation/scale), routed through `SceneCommand::
/// SetTransform`/`execute_command` (Pulsar-Native#561), not
/// `SceneDatabase::update_object`.
///
/// Previously every one of these 9 fields used `F32FieldBinding::new`
/// (whole-`SceneObjectData` getter/setter) whose `set()` called
/// `db.update_object(obj)` -- a full read-modify-write of the ENTIRE object,
/// which (via `sync_registered_component_props_to_scene_db`) re-serialized
/// and re-hydrated every `World`-registered component on the object on
/// *every keystroke*, and never touched the undo stack despite this
/// module's own now-removed doc claiming it did. `new_with_db`'s setter
/// closure below builds the single, minimal `SceneCommand::SetTransform`
/// for just the axis that changed and runs it through `execute_command`,
/// which is both cheaper (`SceneDatabase::set_transform` touches only the
/// transform, not any component) and correctly undo-tracked.
fn axis_binding(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    axis: Axis,
    index: usize,
) -> super::super::bindings::field_bindings::F32FieldBinding {
    use super::super::bindings::field_bindings::F32FieldBinding;

    F32FieldBinding::new_with_db(
        move |id, db| {
            db.get_object(id).map(|obj| match axis {
                Axis::Position => obj.transform.position[index],
                Axis::Rotation => obj.transform.rotation[index],
                Axis::Scale => obj.transform.scale[index],
            })
        },
        move |id, val, db| {
            let Some(obj) = db.get_object(id) else { return false };
            let mut position = None;
            let mut rotation = None;
            let mut scale = None;
            match axis {
                Axis::Position => {
                    let mut v = obj.transform.position;
                    v[index] = val;
                    position = Some(v);
                }
                Axis::Rotation => {
                    let mut v = obj.transform.rotation;
                    v[index] = val;
                    rotation = Some(v);
                }
                Axis::Scale => {
                    let mut v = obj.transform.scale;
                    v[index] = val;
                    scale = Some(v);
                }
            }
            execute_command(
                &mut state_arc.write(),
                SceneCommand::SetTransform { id: id.clone(), position, rotation, scale },
            )
            .changed
        },
    )
}

/// Transform section component for the properties panel
///
/// Displays and allows editing of:
/// - Position (X, Y, Z) with red, yellow, blue color indicators
/// - Rotation (X, Y, Z in degrees)
/// - Scale (X, Y, Z)
pub struct TransformSection {
    // Position fields
    position_x: Entity<F32BoundField>,
    position_y: Entity<F32BoundField>,
    position_z: Entity<F32BoundField>,

    // Rotation fields
    rotation_x: Entity<F32BoundField>,
    rotation_y: Entity<F32BoundField>,
    rotation_z: Entity<F32BoundField>,

    // Scale fields
    scale_x: Entity<F32BoundField>,
    scale_y: Entity<F32BoundField>,
    scale_z: Entity<F32BoundField>,

    object_id: String,
    collapsed: bool,
}

impl TransformSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Position fields
        let position_x = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Position, 0),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let position_y = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Position, 1),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let position_z = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Position, 2),
                "Z",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Rotation fields
        let rotation_x = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Rotation, 0),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let rotation_y = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Rotation, 1),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let rotation_z = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Rotation, 2),
                "Z",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Scale fields
        let scale_x = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Scale, 0),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let scale_y = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Scale, 1),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let scale_z = cx.new(|cx| {
            F32BoundField::new(
                axis_binding(state_arc.clone(), Axis::Scale, 2),
                "Z",
                object_id.clone(),
                scene_db,
                window,
                cx,
            )
        });

        Self {
            position_x,
            position_y,
            position_z,
            rotation_x,
            rotation_y,
            rotation_z,
            scale_x,
            scale_y,
            scale_z,
            object_id,
            collapsed: false,
        }
    }

    /// Refresh all fields when scene data changes externally (e.g., from undo/redo or gizmo manipulation)
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        // Position
        self.position_x
            .update(cx, |field, cx| field.refresh(window, cx));
        self.position_y
            .update(cx, |field, cx| field.refresh(window, cx));
        self.position_z
            .update(cx, |field, cx| field.refresh(window, cx));

        // Rotation
        self.rotation_x
            .update(cx, |field, cx| field.refresh(window, cx));
        self.rotation_y
            .update(cx, |field, cx| field.refresh(window, cx));
        self.rotation_z
            .update(cx, |field, cx| field.refresh(window, cx));

        // Scale
        self.scale_x
            .update(cx, |field, cx| field.refresh(window, cx));
        self.scale_y
            .update(cx, |field, cx| field.refresh(window, cx));
        self.scale_z
            .update(cx, |field, cx| field.refresh(window, cx));
    }

    fn toggle_collapsed(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    /// Render a vector3 field (Position, Rotation, or Scale) with color-coded axes
    fn render_vector3_field(
        &self,
        label: &str,
        x_field: &Entity<F32BoundField>,
        y_field: &Entity<F32BoundField>,
        z_field: &Entity<F32BoundField>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(self.render_axis_field(
                        x_field,
                        Hsla {
                            h: 0.0,
                            s: 0.8,
                            l: 0.5,
                            a: 1.0,
                        }, // Red - East/West
                        cx,
                    ))
                    .child(self.render_axis_field(
                        y_field,
                        Hsla {
                            h: 50.0,
                            s: 0.9,
                            l: 0.5,
                            a: 1.0,
                        }, // Yellow - Vertical
                        cx,
                    ))
                    .child(self.render_axis_field(
                        z_field,
                        Hsla {
                            h: 220.0,
                            s: 0.8,
                            l: 0.55,
                            a: 1.0,
                        }, // Blue - North/South
                        cx,
                    )),
            )
    }

    /// Render a single axis input with colored indicator
    fn render_axis_field(
        &self,
        field: &Entity<F32BoundField>,
        axis_color: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // We need to override the default F32BoundField rendering to add the colored axis indicator
        // Get the label from the field
        let label = field.read(cx).label.clone();
        let input = field.read(cx).input.clone();

        h_flex()
            .flex_1()
            .h_7()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Axis label with color indicator
                div()
                    .w_6()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(axis_color.opacity(0.2))
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(axis_color)
                            .child(label),
                    ),
            )
            .child(
                div().flex_1().h_full().child(
                    ui::input::NumberInput::new(&input)
                        .appearance(false) // No border/background from NumberInput
                        .xsmall(),
                ),
            )
    }
}

impl Render for TransformSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section_name = "Transform".to_string();
        let chevron_icon = if self.collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };

        v_flex()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Section header - clickable to toggle
                h_flex()
                    .id("transform-section-header")
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .when(!self.collapsed, |this| {
                        this.border_b_1().border_color(cx.theme().border)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar.opacity(0.8)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_collapsed(cx);
                        }),
                    )
                    .child(
                        ui::Icon::new(chevron_icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground),
                    )
                    .child(
                        ui::Icon::new(IconName::Axes)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("Transform"),
                    ),
            )
            .when(!self.collapsed, |this| {
                this.child(
                    // Section content - only shown when not collapsed
                    div().w_full().p_3().bg(cx.theme().background).child(
                        v_flex()
                            .gap_3()
                            .child(self.render_vector3_field(
                                "Position",
                                &self.position_x,
                                &self.position_y,
                                &self.position_z,
                                cx,
                            ))
                            .child(self.render_vector3_field(
                                "Rotation",
                                &self.rotation_x,
                                &self.rotation_y,
                                &self.rotation_z,
                                cx,
                            ))
                            .child(self.render_vector3_field(
                                "Scale",
                                &self.scale_x,
                                &self.scale_y,
                                &self.scale_z,
                                cx,
                            )),
                    ),
                )
            })
    }
}
