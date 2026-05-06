//! Component renderer that auto-generates UI from reflection metadata
//!
//! This module uses the reflection system to automatically generate property
//! editors for components. It reads PropertyMetadata from EngineClass components
//! and renders appropriate input widgets based on PropertyType.

use super::property_inputs::*;
use engine_backend::{ComponentInstance, EditorObjectId, EngineClass, PropertyType, PropertyValue};
use gpui::{prelude::*, App, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    label::Label,
    v_flex, ActiveTheme, CollapsibleSection, Icon, IconName, Sizable, StyledExt,
};

/// Auto-generates UI for a component based on reflection metadata
///
/// This struct renders all properties of a component by inspecting its
/// PropertyMetadata from the EngineClass trait. Each property gets an
/// appropriate input widget based on its PropertyType.
pub struct ComponentRenderer {
    /// Object ID this component belongs to
    object_id: EditorObjectId,

    /// Index of this component in the object's component list
    component_index: usize,

    /// Component class name
    class_name: String,
}

impl ComponentRenderer {
    /// Create a new component renderer
    pub fn new(
        object_id: EditorObjectId,
        component_index: usize,
        class_name: String,
    ) -> Self {
        Self {
            object_id,
            component_index,
            class_name,
        }
    }

    /// Render the component as a collapsible section
    ///
    /// This will:
    /// 1. Display component class name as the header
    /// 2. Show a remove button
    /// 3. Render all properties in a vertical list
    pub fn render(
        &self,
        component_data: &serde_json::Value,
        _collapsed: bool,
        cx: &App,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .bg(cx.theme().sidebar)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            // Header with component name and remove button
            .child(self.render_header(cx))
            // Properties list
            .child(self.render_properties(component_data, cx))
    }

    /// Render component header with name and remove button
    fn render_header(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::Component)
                            .small()
                            .text_color(cx.theme().accent)
                    )
                    .child(
                        Label::new(self.class_name.clone())
                            .text_sm()
                            .text_color(cx.theme().foreground)
                    )
            )
            .child(
                Button::new(format!("remove-component-{}", self.component_index))
                    .icon(IconName::Trash)
                    .xsmall()
                    .ghost()
            )
    }

    /// Render all properties for this component
    ///
    /// In a full implementation, this would:
    /// 1. Query the component registry for the class
    /// 2. Create an instance from the JSON data
    /// 3. Call get_properties() to get reflection metadata
    /// 4. Render each property with appropriate widget
    ///
    /// For now, we render a placeholder that shows the structure.
    fn render_properties(
        &self,
        component_data: &serde_json::Value,
        cx: &App,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new("Component properties will be auto-generated here")
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
            )
            .child(
                div()
                    .p_2()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_sm()
                    .child(
                        Label::new(format!("Data: {}", component_data.to_string()))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                    )
            )
    }

    /// Render a single property based on its PropertyType
    ///
    /// This is the core of the auto-generation system. It reads PropertyMetadata
    /// and dispatches to the appropriate input widget.
    #[allow(dead_code)]
    fn render_property(
        &self,
        property_name: &str,
        property_type: &PropertyType,
        property_value: &PropertyValue,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            // Property label
            .child(render_property_label(property_name, cx))
            // Property input (auto-selected based on type)
            .child(self.render_property_input(property_type, property_value, cx))
    }

    /// Render the appropriate input widget for a property type
    #[allow(dead_code)]
    fn render_property_input(
        &self,
        property_type: &PropertyType,
        property_value: &PropertyValue,
        cx: &App,
    ) -> impl IntoElement {
        match (property_type, property_value) {
            // F32 input with constraints
            (PropertyType::F32 { min, max, step }, PropertyValue::F32(value)) => {
                render_f32_input(*value, *min, *max, *step, |_new_value| {}, cx).into_any_element()
            }

            // I32 input with constraints
            (PropertyType::I32 { min, max }, PropertyValue::I32(value)) => {
                render_i32_input(*value, *min, *max, |_new_value| {}, cx).into_any_element()
            }

            // Boolean checkbox
            (PropertyType::Bool, PropertyValue::Bool(value)) => {
                render_bool_input(*value, |_new_value| {}, cx).into_any_element()
            }

            // String input
            (PropertyType::String { max_length }, PropertyValue::String(value)) => {
                render_string_input(value, *max_length, |_new_value| {}, cx).into_any_element()
            }

            // Vec3 input
            (PropertyType::Vec3, PropertyValue::Vec3(value)) => {
                render_vec3_input(*value, |_new_value| {}, cx).into_any_element()
            }

            // Color input
            (PropertyType::Color, PropertyValue::Color(value)) => {
                render_color_input(*value, |_new_value| {}, cx).into_any_element()
            }

            // Enum dropdown
            (PropertyType::Enum { variants }, PropertyValue::EnumVariant(selected)) => {
                let selected_name = variants.get(*selected).unwrap_or(&"Unknown");
                render_enum_input(variants, *selected, selected_name, |_new_index| {}, cx)
                    .into_any_element()
            }

            // Vec<T> array editor
            (PropertyType::Vec { element_type }, PropertyValue::Vec(items)) => {
                let items_str: Vec<String> = items
                    .iter()
                    .map(|v| format!("{:?}", v))
                    .collect();

                render_vec_input(
                    &items_str,
                    "element",
                    || {},
                    |_index| {},
                    cx,
                )
                .into_any_element()
            }

            // Nested component
            (PropertyType::Component { class_name }, PropertyValue::Component { .. }) => {
                v_flex()
                    .gap_1()
                    .ml_4()
                    .p_2()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_sm()
                    .child(
                        Label::new(format!("Nested Component: {}", class_name))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                    )
                    .into_any_element()
            }

            // Type mismatch fallback
            _ => {
                div()
                    .child(
                        Label::new("Type mismatch in property")
                            .text_xs()
                            .text_color(cx.theme().danger_foreground)
                    )
                    .into_any_element()
            }
        }
    }
}

/// Component list section that shows all components on an object
///
/// This renders a list of ComponentRenderer instances, one for each component
/// attached to the selected object.
pub struct ComponentListSection {
    object_id: EditorObjectId,
}

impl ComponentListSection {
    pub fn new(object_id: EditorObjectId) -> Self {
        Self { object_id }
    }

    /// Render all components for the object
    pub fn render(
        &self,
        components: &[ComponentInstance],
        cx: &App,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            // Section header with add component button
            .child(self.render_header(cx))
            // Component list
            .children(components.iter().enumerate().map(|(idx, component)| {
                ComponentRenderer::new(
                    self.object_id.clone(),
                    idx,
                    component.class_name.clone(),
                )
                .render(&component.data, false, cx)
            }))
            // Empty state if no components
            .when(components.is_empty(), |this| {
                this.child(self.render_empty_state(cx))
            })
    }

    fn render_header(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(
                Label::new("Components")
                    .text_sm()
                    .text_color(cx.theme().foreground)
            )
            .child(
                Button::new("add-component")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
            )
    }

    fn render_empty_state(&self, cx: &App) -> impl IntoElement {
        div()
            .w_full()
            .p_4()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .child(
                v_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::Component)
                            .with_size(ui::Size::Medium)
                            .text_color(cx.theme().muted_foreground)
                    )
                    .child(
                        Label::new("No components")
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                    )
                    .child(
                        Label::new("Click + to add a component")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                    )
            )
    }
}

/// Example of how to integrate with the reflection system
///
/// This shows how the full implementation would work with actual EngineClass instances.
#[allow(dead_code)]
fn example_render_component_with_reflection(
    component: &dyn EngineClass,
    cx: &App,
) -> impl IntoElement {
    let class_name = std::any::type_name_of_val(component);
    let properties = component.get_properties();

    v_flex()
        .w_full()
        .gap_3()
        .child(
            Label::new(class_name)
                .text_sm()
                .text_color(cx.theme().foreground)
        )
        .children(properties.iter().map(|prop| {
            let value = (prop.getter)(component);

            h_flex()
                .w_full()
                .gap_2()
                .child(render_property_label(&prop.display_name, cx))
                .child(
                    match (&prop.property_type, &value) {
                        (PropertyType::F32 { min, max, step }, PropertyValue::F32(v)) => {
                            render_f32_input(*v, *min, *max, *step, |_| {}, cx).into_any_element()
                        }
                        (PropertyType::Bool, PropertyValue::Bool(v)) => {
                            render_bool_input(*v, |_| {}, cx).into_any_element()
                        }
                        // ... other types
                        _ => div().into_any_element(),
                    }
                )
        }))
}
