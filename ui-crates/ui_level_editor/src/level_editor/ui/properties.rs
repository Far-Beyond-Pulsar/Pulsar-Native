use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Sizable, StyledExt,
    input::{TextInput, InputState},
    IconName,
};
use std::sync::Arc;
use std::collections::HashSet;
use rust_i18n::t;

use super::state::{LevelEditorState, Transform};
use crate::level_editor::scene_database::ObjectType;
use crate::level_editor::workspace_panels::PropertiesPanelWrapper;

/// Properties Panel - Inspector showing properties of the selected object
pub struct PropertiesPanel;

impl PropertiesPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        collapsed_sections: &HashSet<String>,
        object_header_section: &Option<Entity<super::ObjectHeaderSection>>,
        transform_section: &Option<Entity<super::TransformSection>>,
        material_section: &Option<Entity<super::MaterialSection>>,
        window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header
            .child(self.render_header(state, cx))
            // Main content area
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .size_full()
                            .scrollable(ScrollbarAxis::Vertical)
                            .child(
                                if let Some(_selected) = state.get_selected_object() {
                                    let mut flex = v_flex()
                                        .w_full()
                                        .p_3()
                                        .gap_4();

                                    // Render new ObjectHeaderSection if available (new binding system)
                                    if let Some(ref section) = object_header_section {
                                        flex = flex.child(section.clone());
                                    }

                                    // Render new TransformSection if available (new binding system)
                                    if let Some(ref section) = transform_section {
                                        flex = flex.child(section.clone());
                                    }

                                    // Render new MaterialSection if available (new binding system)
                                    if let Some(ref section) = material_section {
                                        flex = flex.child(section.clone());
                                    }

                                    // Keep old sections for now (TODO: convert to binding system)
                                    // flex = flex
                                    //     .child(Self::render_object_type_section(&selected, collapsed_sections, cx))
                                    //     .child(Self::render_tags_section(collapsed_sections, cx))
                                    //     .child(Self::render_components_section(collapsed_sections, cx))
                                    //     .child(Self::render_rendering_section(&selected, collapsed_sections, cx))
                                    //     .child(Self::render_physics_section(&selected, collapsed_sections, cx));

                                    flex.into_any_element()
                                } else {
                                    Self::render_empty_state(cx).into_any_element()
                                }
                            )
                    )
            )
    }

    fn render_header(&self, state: &LevelEditorState, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let has_selection = state.get_selected_object().is_some();
        
        h_flex()
            .w_full()
            .px_4()
            .py_3()
            .justify_between()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(t!("LevelEditor.Properties.Title").to_string())
                    )
                    .when(has_selection, |this| {
                        this.child(
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(cx.theme().accent.opacity(0.15))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(cx.theme().accent)
                                .child(t!("LevelEditor.Properties.Selected").to_string())
                        )
                    })
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("more_options")
                            .icon(IconName::Ellipsis)
                            .xsmall()
                    )
            )
    }

    fn render_empty_state(cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                v_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        ui::Icon::new(IconName::CursorPointer)
                            .size(px(48.0))
                            .text_color(cx.theme().muted_foreground.opacity(0.5))
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("LevelEditor.Properties.NoSelection").to_string())
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                            .text_center()
                            .child(t!("LevelEditor.Properties.NoSelectionDesc").to_string())
                    )
            )
    }

    fn render_object_header(object: &super::state::SceneObject, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let icon = match object.object_type {
            ObjectType::Camera => IconName::Camera,
            ObjectType::Folder => IconName::Folder,
            ObjectType::Light(_) => IconName::Sun,
            ObjectType::Mesh(_) => IconName::Box,
            ObjectType::Empty => IconName::Circle,
            ObjectType::ParticleSystem => IconName::Spark,
            ObjectType::AudioSource => IconName::MusicNote,
        };
        
        v_flex()
            .w_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .size_10()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .bg(cx.theme().accent.opacity(0.15))
                            .child(
                                ui::Icon::new(icon)
                                    .size(px(20.0))
                                    .text_color(cx.theme().accent)
                            )
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(object.name.clone())
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("ID: {}", object.id))
                            )
                    )
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Self::render_toggle_chip("Visible", object.visible, IconName::Eye, cx)
                    )
                    .child(
                        Self::render_toggle_chip("Locked", false, IconName::Lock, cx)
                    )
            )
    }
    
    fn render_toggle_chip(label: &str, active: bool, icon: IconName, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let bg_color = if active {
            cx.theme().accent.opacity(0.15)
        } else {
            cx.theme().muted.opacity(0.3)
        };
        let text_color = if active {
            cx.theme().accent
        } else {
            cx.theme().muted_foreground
        };
        let chip_id = SharedString::from(format!("toggle-chip-{}", label));
        
        h_flex()
            .id(chip_id)
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .rounded(px(4.0))
            .bg(bg_color)
            .cursor_pointer()
            .hover(|s| s.opacity(0.8))
            .child(
                ui::Icon::new(icon)
                    .size(px(12.0))
                    .text_color(text_color)
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_color)
                    .child(label.to_string())
            )
    }

    fn render_transform_section(
        transform: &Transform,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        is_collapsed: bool,
        window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>
    ) -> impl IntoElement {
        Self::render_collapsible_section(
            "Transform",
            IconName::Axes,
            is_collapsed,
            v_flex()
                .gap_3()
                .child(Self::render_vector3_field("Position", "position", transform.position, editing_property, property_input, window, cx))
                .child(Self::render_vector3_field("Rotation", "rotation", transform.rotation, editing_property, property_input, window, cx))
                .child(Self::render_vector3_field("Scale", "scale", transform.scale, editing_property, property_input, window, cx)),
            cx
        )
    }
    
    fn render_collapsible_section(
        title: &str,
        icon: IconName,
        is_collapsed: bool,
        content: impl IntoElement,
        cx: &mut Context<PropertiesPanelWrapper>
    ) -> impl IntoElement {
        let section_name = title.to_string();
        let chevron_icon = if is_collapsed { IconName::ChevronRight } else { IconName::ChevronDown };
        let section_id = SharedString::from(format!("props-section-{}", title));
        
        v_flex()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Section header - clickable to toggle
                h_flex()
                    .id(section_id)
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .when(!is_collapsed, |this| this.border_b_1().border_color(cx.theme().border))
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar.opacity(0.8)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _event, _window, cx| {
                        this.toggle_section(section_name.clone(), cx);
                    }))
                    .child(
                        ui::Icon::new(chevron_icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        ui::Icon::new(icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(title.to_string())
                    )
            )
            .when(!is_collapsed, |this| {
                this.child(
                    // Section content - only shown when not collapsed
                    div()
                        .w_full()
                        .p_3()
                        .bg(cx.theme().background)
                        .child(content)
                )
            })
    }

    fn render_object_type_section(object: &super::state::SceneObject, collapsed_sections: &HashSet<String>, cx: &mut Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let (title, icon) = match object.object_type {
            ObjectType::Camera => ("Camera Settings", IconName::Camera),
            ObjectType::Folder => ("Folder Settings", IconName::Folder),
            ObjectType::Light(_) => ("Light Settings", IconName::Sun),
            ObjectType::Mesh(_) => ("Mesh Settings", IconName::Box),
            ObjectType::Empty => ("Empty Object", IconName::Circle),
            ObjectType::ParticleSystem => ("Particle System", IconName::Sparks),
            ObjectType::AudioSource => ("Audio Source", IconName::MusicNote),
        };
        
        let is_collapsed = collapsed_sections.contains(title);
        
        let content = match object.object_type {
            ObjectType::Camera => Self::render_camera_settings(cx).into_any_element(),
            ObjectType::Light(_) => Self::render_light_settings(cx).into_any_element(),
            ObjectType::Mesh(_) => Self::render_mesh_settings(cx).into_any_element(),
            _ => v_flex()
                .items_center()
                .py_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No additional settings")
                )
                .into_any_element(),
        };
        
        Self::render_collapsible_section(title, icon, is_collapsed, content, cx)
    }

    fn render_camera_settings(cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(Self::render_property_row("FOV", "60", "°", cx))
            .child(Self::render_property_row("Near Clip", "0.1", "m", cx))
            .child(Self::render_property_row("Far Clip", "1000", "m", cx))
            .child(Self::render_dropdown_row("Projection", "Perspective", cx))
    }

    fn render_light_settings(cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(Self::render_property_row("Intensity", "1.0", "", cx))
            .child(Self::render_color_row("Color", Hsla { h: 45.0, s: 0.9, l: 0.6, a: 1.0 }, cx))
            .child(Self::render_dropdown_row("Shadow Mode", "Soft Shadows", cx))
            .child(Self::render_property_row("Shadow Bias", "0.001", "", cx))
    }

    fn render_mesh_settings(cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(Self::render_asset_row("Material", "Default Material", IconName::EditPencil, cx))
            .child(Self::render_color_row("Tint", Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 }, cx))
            .child(Self::render_toggle_row("Cast Shadows", true, cx))
            .child(Self::render_toggle_row("Receive Shadows", true, cx))
    }
    
    fn render_property_row(label: &str, value: &str, unit: &str, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .bg(cx.theme().input)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(4.0))
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .cursor_pointer()
                            .hover(|s| s.border_color(cx.theme().accent.opacity(0.5)))
                            .child(value.to_string())
                    )
                    .when(!unit.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(unit.to_string())
                        )
                    })
            )
    }
    
    fn render_dropdown_row(label: &str, value: &str, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .justify_between()
                    .bg(cx.theme().input)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.border_color(cx.theme().accent.opacity(0.5)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(value.to_string())
                    )
                    .child(
                        ui::Icon::new(IconName::ChevronDown)
                            .size(px(14.0))
                            .text_color(cx.theme().muted_foreground)
                    )
            )
    }
    
    fn render_color_row(label: &str, color: Hsla, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .size_7()
                            .bg(color)
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(cx.theme().border)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .bg(cx.theme().input)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(4.0))
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(format!("#{:02X}{:02X}{:02X}", 
                                (color.l * 255.0) as u8,
                                (color.l * 255.0) as u8,
                                (color.l * 255.0) as u8
                            ))
                    )
            )
    }
    
    fn render_toggle_row(label: &str, enabled: bool, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                div()
                    .w_9()
                    .h_5()
                    .rounded_full()
                    .bg(if enabled { cx.theme().accent } else { cx.theme().muted })
                    .cursor_pointer()
                    .child(
                        div()
                            .size_4()
                            .mt(px(2.0))
                            .ml(if enabled { px(18.0) } else { px(2.0) })
                            .rounded_full()
                            .bg(white())
                            .shadow_sm()
                    )
            )
    }
    
    fn render_asset_row(label: &str, value: &str, icon: IconName, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().input)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.border_color(cx.theme().accent.opacity(0.5)))
                    .child(
                        ui::Icon::new(icon)
                            .size(px(14.0))
                            .text_color(cx.theme().accent)
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(value.to_string())
                    )
                    .child(
                        ui::Icon::new(IconName::ChevronRight)
                            .size(px(14.0))
                            .text_color(cx.theme().muted_foreground)
                    )
            )
    }

    fn render_vector3_field(
        label: &str,
        field_name: &str,  // "position", "rotation", "scale"
        values: [f32; 3],
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>
    ) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}", label))
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(Self::render_axis_input(
                        "X",
                        Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 }, // Red - East/West
                        &format!("{}.x", field_name),
                        values[0],
                        editing_property,
                        property_input,
                        window,
                        cx
                    ))
                    .child(Self::render_axis_input(
                        "Y",
                        Hsla { h: 50.0, s: 0.9, l: 0.5, a: 1.0 }, // Yellow - Vertical
                        &format!("{}.y", field_name),
                        values[1],
                        editing_property,
                        property_input,
                        window,
                        cx
                    ))
                    .child(Self::render_axis_input(
                        "Z",
                        Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue - North/South
                        &format!("{}.z", field_name),
                        values[2],
                        editing_property,
                        property_input,
                        window,
                        cx
                    ))
            )
    }

    fn render_axis_input(
        axis: &str,
        axis_color: Hsla,
        property_path: &str,
        value: f32,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>
    ) -> impl IntoElement {
        let value_str = format!("{:.2}", value);
        let is_editing = editing_property.as_ref() == Some(&property_path.to_string());
        let property_path_owned = property_path.to_string();
        let value_str_for_click = value_str.clone();

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
                            .child(axis.to_string())
                    )
            )
            .child(
                if is_editing {
                    div()
                        .flex_1()
                        .h_full()
                        .child(
                            TextInput::new(property_input)
                                .w_full()
                                .px_2()
                                .text_xs()
                                .bg(cx.theme().input)
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .px_2()
                        .bg(cx.theme().input)
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
                        .child(value_str.clone())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _event, window, cx| {
                            this.start_editing(property_path_owned.clone(), value_str_for_click.clone(), window, cx);
                        }))
                        .into_any_element()
                }
            )
    }

    // ===== NEW SECTIONS =====

    fn render_tags_section(collapsed_sections: &HashSet<String>, cx: &mut Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let is_collapsed = collapsed_sections.contains("Tags & Layers");
        
        Self::render_collapsible_section(
            "Tags & Layers",
            IconName::Label,
            is_collapsed,
            v_flex()
                .gap_3()
                .child(Self::render_dropdown_row("Tag", "Untagged", cx))
                .child(Self::render_dropdown_row("Layer", "Default", cx))
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(Self::render_mini_toggle("Static", true, cx))
                        .child(Self::render_mini_toggle("Navigation", false, cx))
                ),
            cx
        )
    }

    fn render_components_section(collapsed_sections: &HashSet<String>, cx: &mut Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let is_collapsed = collapsed_sections.contains("Components");
        
        Self::render_collapsible_section(
            "Components",
            IconName::Puzzle,
            is_collapsed,
            v_flex()
                .gap_2()
                .child(Self::render_component_item("Transform", "Built-in", true, cx))
                .child(Self::render_component_item("MeshRenderer", "Built-in", true, cx))
                .child(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .pt_2()
                        .child(
                            Button::new("add_component")
                                .label("Add Component")
                                .icon(IconName::Plus)
                                .xsmall()
                        )
                ),
            cx
        )
    }

    fn render_rendering_section(object: &super::state::SceneObject, collapsed_sections: &HashSet<String>, cx: &mut Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let is_collapsed = collapsed_sections.contains("Rendering");
        
        Self::render_collapsible_section(
            "Rendering",
            IconName::Eye,
            is_collapsed,
            v_flex()
                .gap_3()
                .child(Self::render_dropdown_row("Render Queue", "Opaque (2000)", cx))
                .child(Self::render_toggle_row("Cast Shadows", true, cx))
                .child(Self::render_toggle_row("Receive Shadows", true, cx))
                .child(Self::render_dropdown_row("Light Probes", "Blend Probes", cx))
                .child(Self::render_dropdown_row("Reflection Probes", "Blend Probes", cx)),
            cx
        )
    }

    fn render_physics_section(object: &super::state::SceneObject, collapsed_sections: &HashSet<String>, cx: &mut Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let is_collapsed = collapsed_sections.contains("Physics");
        
        Self::render_collapsible_section(
            "Physics",
            IconName::Activity,
            is_collapsed,
            v_flex()
                .gap_3()
                .child(Self::render_toggle_row("Use Gravity", true, cx))
                .child(Self::render_toggle_row("Is Kinematic", false, cx))
                .child(Self::render_property_row("Mass", "1.0", "kg", cx))
                .child(Self::render_property_row("Drag", "0.0", "", cx))
                .child(Self::render_property_row("Angular Drag", "0.05", "", cx))
                .child(Self::render_dropdown_row("Collision Detection", "Discrete", cx)),
            cx
        )
    }

    fn render_mini_toggle(label: &str, active: bool, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let toggle_id = SharedString::from(format!("mini-toggle-{}", label));
        let bg = if active { cx.theme().accent.opacity(0.2) } else { cx.theme().muted.opacity(0.3) };
        let text_color = if active { cx.theme().accent } else { cx.theme().muted_foreground };
        
        h_flex()
            .id(toggle_id)
            .flex_1()
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .justify_center()
            .bg(bg)
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.opacity(0.8))
            .child(
                div()
                    .size_2()
                    .rounded_full()
                    .bg(text_color)
            )
            .child(
                div()
                    .text_xs()
                    .text_color(text_color)
                    .child(label.to_string())
            )
    }

    fn render_component_item(name: &str, category: &str, enabled: bool, cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        let item_id = SharedString::from(format!("component-{}", name));
        
        h_flex()
            .id(item_id)
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .bg(cx.theme().input)
            .rounded(px(4.0))
            .border_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().accent.opacity(0.1)))
            .child(
                div()
                    .size_4()
                    .rounded(px(2.0))
                    .bg(if enabled { cx.theme().accent } else { cx.theme().muted })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        ui::Icon::new(IconName::Check)
                            .size(px(10.0))
                            .text_color(white())
                    )
            )
            .child(
                v_flex()
                    .flex_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(name.to_string())
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(category.to_string())
                    )
            )
            .child(
                ui::Icon::new(IconName::Ellipsis)
                    .size(px(14.0))
                    .text_color(cx.theme().muted_foreground)
            )
    }
}
