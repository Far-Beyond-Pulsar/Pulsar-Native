//! Transform Section - Property panel section for editing object transforms
//!
//! This component provides a clean UI for editing position, rotation, and scale
//! of scene objects using the field binding system for automatic sync and undo/redo.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt,
};

use crate::level_editor::scene_database::SceneDatabase;
use super::bound_field::F32BoundField;

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::field_bindings::F32FieldBinding;

        // Position fields
        let position_x = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.position[0],
                    |obj, val| obj.transform.position[0] = val,
                ),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let position_y = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.position[1],
                    |obj, val| obj.transform.position[1] = val,
                ),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let position_z = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.position[2],
                    |obj, val| obj.transform.position[2] = val,
                ),
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
                F32FieldBinding::new(
                    |obj| obj.transform.rotation[0],
                    |obj, val| obj.transform.rotation[0] = val,
                ),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let rotation_y = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.rotation[1],
                    |obj, val| obj.transform.rotation[1] = val,
                ),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let rotation_z = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.rotation[2],
                    |obj, val| obj.transform.rotation[2] = val,
                ),
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
                F32FieldBinding::new(
                    |obj| obj.transform.scale[0],
                    |obj, val| obj.transform.scale[0] = val,
                ),
                "X",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let scale_y = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.scale[1],
                    |obj, val| obj.transform.scale[1] = val,
                ),
                "Y",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        let scale_z = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    |obj| obj.transform.scale[2],
                    |obj, val| obj.transform.scale[2] = val,
                ),
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
        self.position_x.update(cx, |field, cx| field.refresh(window, cx));
        self.position_y.update(cx, |field, cx| field.refresh(window, cx));
        self.position_z.update(cx, |field, cx| field.refresh(window, cx));

        // Rotation
        self.rotation_x.update(cx, |field, cx| field.refresh(window, cx));
        self.rotation_y.update(cx, |field, cx| field.refresh(window, cx));
        self.rotation_z.update(cx, |field, cx| field.refresh(window, cx));

        // Scale
        self.scale_x.update(cx, |field, cx| field.refresh(window, cx));
        self.scale_y.update(cx, |field, cx| field.refresh(window, cx));
        self.scale_z.update(cx, |field, cx| field.refresh(window, cx));
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
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(self.render_axis_field(
                        x_field,
                        Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 }, // Red - East/West
                        cx
                    ))
                    .child(self.render_axis_field(
                        y_field,
                        Hsla { h: 50.0, s: 0.9, l: 0.5, a: 1.0 }, // Yellow - Vertical
                        cx
                    ))
                    .child(self.render_axis_field(
                        z_field,
                        Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue - North/South
                        cx
                    ))
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
                            .child(label)
                    )
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .child(
                        ui::input::NumberInput::new(&input)
                            .appearance(false) // No border/background from NumberInput
                            .xsmall()
                    )
            )
    }
}

impl Render for TransformSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section_name = "Transform".to_string();
        let chevron_icon = if self.collapsed { IconName::ChevronRight } else { IconName::ChevronDown };

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
                    .when(!self.collapsed, |this| this.border_b_1().border_color(cx.theme().border))
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar.opacity(0.8)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                        this.toggle_collapsed(cx);
                    }))
                    .child(
                        ui::Icon::new(chevron_icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        ui::Icon::new(IconName::Axes)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("Transform")
                    )
            )
            .when(!self.collapsed, |this| {
                this.child(
                    // Section content - only shown when not collapsed
                    div()
                        .w_full()
                        .p_3()
                        .bg(cx.theme().background)
                        .child(
                            v_flex()
                                .gap_3()
                                .child(self.render_vector3_field("Position", &self.position_x, &self.position_y, &self.position_z, cx))
                                .child(self.render_vector3_field("Rotation", &self.rotation_x, &self.rotation_y, &self.rotation_z, cx))
                                .child(self.render_vector3_field("Scale", &self.scale_x, &self.scale_y, &self.scale_z, cx))
                        )
                )
            })
    }
}
