use gpui::*;
use ui::{
    button::Button, h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Sizable, StyledExt,
    input::{TextInput, InputState},
};
use std::sync::Arc;

use super::state::{LevelEditorState, Transform};
use crate::tabs::level_editor::scene_database::ObjectType;

/// Properties Panel - Inspector showing properties of the selected object
pub struct PropertiesPanel;

impl PropertiesPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<F>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        on_start_edit: F,
        cx: &mut App
    ) -> impl IntoElement
    where
        F: Fn(String, String) + 'static + Clone,
    {
        v_flex()
            .size_full()
            .gap_2()
            .child(
                // Header
                h_flex()
                    .w_full()
                    .p_2()
                    .justify_between()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Properties")
                    )
            )
            .child(
                // Properties content
                div()
                    .flex_1()
                    .overflow_hidden()
                    .p_3()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .child(
                        v_flex()
                            .size_full()
                            .scrollable(ScrollbarAxis::Vertical)
                            .child(
                        if let Some(selected) = state.get_selected_object() {
                            v_flex()
                                .gap_4()
                                .child(Self::render_object_header(&selected, cx))
                                .child(Self::render_transform_section(
                                    &selected.transform,
                                    editing_property,
                                    property_input,
                                    on_start_edit.clone(),
                                    cx
                                ))
                                .child(Self::render_object_type_section(&selected, cx))
                                .into_any_element()
                        } else {
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No object selected")
                                )
                                .into_any_element()
                        })
                    )
            )
    }

    fn render_object_header(object: &super::state::SceneObject, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(object.name.clone())
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("ID: {}", object.id))
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Visible:")
                    )
                    .child(
                        Button::new("toggle_visibility")
                            .child(if object.visible { "Yes" } else { "No" })
                            .xsmall()
                    )
            )
    }

    fn render_transform_section<F>(
        transform: &Transform,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        on_start_edit: F,
        cx: &App
    ) -> impl IntoElement
    where
        F: Fn(String, String) + 'static + Clone,
    {
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("Transform")
            )
            .child(Self::render_vector3_field("Position", "position", transform.position, editing_property, property_input, on_start_edit.clone(), cx))
            .child(Self::render_vector3_field("Rotation", "rotation", transform.rotation, editing_property, property_input, on_start_edit.clone(), cx))
            .child(Self::render_vector3_field("Scale", "scale", transform.scale, editing_property, property_input, on_start_edit, cx))
    }

    fn render_object_type_section(object: &super::state::SceneObject, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(match object.object_type {
                        ObjectType::Camera => "Camera Settings",
                        ObjectType::Folder => "Folder",
                        ObjectType::Light(_) => "Light Settings",
                        ObjectType::Mesh(_) => "Mesh Settings",
                        ObjectType::Empty => "Empty Object",
                        ObjectType::ParticleSystem => "Particle System",
                        ObjectType::AudioSource => "Audio Source",
                    })
            )
            .child(
                match object.object_type {
                    ObjectType::Camera => Self::render_camera_settings(cx).into_any_element(),
                    ObjectType::Light(_) => Self::render_light_settings(cx).into_any_element(),
                    ObjectType::Mesh(_) => Self::render_mesh_settings(cx).into_any_element(),
                    _ => div().into_any_element(),
                }
            )
    }

    fn render_camera_settings(cx: &App) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(Self::render_field_row("FOV:", "60°", cx))
            .child(Self::render_field_row("Near Clip:", "0.1", cx))
            .child(Self::render_field_row("Far Clip:", "1000.0", cx))
    }

    fn render_light_settings(cx: &App) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(Self::render_field_row("Intensity:", "1.0", cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Color:")
                    )
                    .child(
                        div()
                            .size_6()
                            .bg(cx.theme().primary)
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(cx.theme().border)
                    )
            )
            .child(Self::render_field_row("Shadows:", "Enabled", cx))
    }

    fn render_mesh_settings(cx: &App) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Material:")
                    )
                    .child(
                        Button::new("select_material")
                            .child("Default Material")
                            .xsmall()
                    )
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Color:")
                    )
                    .child(
                        div()
                            .size_6()
                            .bg(cx.theme().primary)
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(cx.theme().border)
                    )
            )
            .child(Self::render_field_row("Cast Shadows:", "Yes", cx))
    }

    fn render_vector3_field<F>(
        label: &str,
        field_name: &str,  // "position", "rotation", "scale"
        values: [f32; 3],
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        on_start_edit: F,
        cx: &App
    ) -> impl IntoElement
    where
        F: Fn(String, String) + 'static + Clone,
    {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(format!("{}:", label))
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Self::render_float_input(
                        "X",
                        &format!("{}.x", field_name),
                        values[0],
                        editing_property,
                        property_input,
                        on_start_edit.clone(),
                        cx
                    ))
                    .child(Self::render_float_input(
                        "Y",
                        &format!("{}.y", field_name),
                        values[1],
                        editing_property,
                        property_input,
                        on_start_edit.clone(),
                        cx
                    ))
                    .child(Self::render_float_input(
                        "Z",
                        &format!("{}.z", field_name),
                        values[2],
                        editing_property,
                        property_input,
                        on_start_edit,
                        cx
                    ))
            )
    }

    fn render_float_input<F>(
        axis: &str,
        property_path: &str,  // "position.x", "rotation.y", etc.
        value: f32,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        on_start_edit: F,
        cx: &App
    ) -> impl IntoElement
    where
        F: Fn(String, String) + 'static,
    {
        let axis_str = axis.to_string();
        let value_str = format!("{:.2}", value);
        let is_editing = editing_property.as_ref() == Some(&property_path.to_string());
        let property_path_owned = property_path.to_string();
        let value_str_for_click = value_str.clone();

        h_flex()
            .flex_1()
            .gap_1()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(axis_str)
            )
            .child(
                if is_editing {
                    // Show TextInput when editing
                    div()
                        .flex_1()
                        .child(
                            TextInput::new(property_input)
                                .w_full()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .rounded(px(4.0))
                                .border_2()
                                .border_color(cx.theme().accent)
                        )
                        .into_any_element()
                } else {
                    // Show clickable div when not editing
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().input)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(px(4.0))
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().input.opacity(0.8)))
                        .child(value_str.clone())
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            // Call the callback to start editing
                            on_start_edit(property_path_owned.clone(), value_str_for_click.clone());
                        })
                        .into_any_element()
                }
            )
    }

    fn render_field_row(label: &str, value: &str, cx: &App) -> impl IntoElement {
        let label_str = label.to_string();
        let value_str = value.to_string();

        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(label_str)
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(value_str)
            )
    }
}
