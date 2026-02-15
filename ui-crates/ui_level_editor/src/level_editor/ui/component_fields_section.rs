//! Dynamic Component Fields Section - Renders component fields using trait-based registry

use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, ActiveTheme, Sizable, StyledExt};
use std::collections::HashMap;
use std::sync::Arc;

use crate::level_editor::scene_database::SceneDatabase;
use engine_backend::scene::ComponentFieldMetadata;

/// Type for custom field renderer functions
pub type CustomFieldRenderer = Arc<dyn Fn(&str, *const (), &Context<ComponentFieldsSection>) -> AnyElement + Send + Sync>;

/// Dynamic section that renders component fields based on trait-based registry
pub struct ComponentFieldsSection {
    component_index: usize,
    object_id: String,
    scene_db: SceneDatabase,
    custom_renderers: HashMap<String, CustomFieldRenderer>,
}

impl ComponentFieldsSection {
    pub fn new(
        component_index: usize,
        object_id: String,
        scene_db: SceneDatabase,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            component_index,
            object_id,
            scene_db,
            custom_renderers: HashMap::new(),
        }
    }
    
    /// Register a custom renderer for a specific type
    /// 
    /// # Example
    /// ```ignore
    /// section.register_custom_renderer("MyCustomType", Arc::new(|label, value_ptr, cx| {
    ///     // SAFETY: Caller must ensure value_ptr is valid and points to MyCustomType
    ///     let value = unsafe { &*(value_ptr as *const MyCustomType) };
    ///     // Return custom GPUI UI here
    ///     div().child(format!("Custom: {}", value)).into_any_element()
    /// }));
    /// ```
    pub fn register_custom_renderer(&mut self, ui_key: impl Into<String>, renderer: CustomFieldRenderer) {
        self.custom_renderers.insert(ui_key.into(), renderer);
    }
}

impl Render for ComponentFieldsSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let obj = self.scene_db.get_object(&self.object_id);
        let (variant_name, field_metadata) = obj.as_ref()
            .and_then(|obj| {
                obj.components.get(self.component_index)
                    .map(|c| (c.variant_name(), c.get_field_metadata()))
            })
            .unwrap_or(("Component", vec![]));

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(variant_name)
                    )
            )
            .children(field_metadata.iter().map(|field_meta| {
                self.render_field(field_meta, cx)
            }))
    }
}

impl ComponentFieldsSection {
    fn render_field(&self, field_meta: &ComponentFieldMetadata, cx: &Context<Self>) -> AnyElement {
        match field_meta {
            ComponentFieldMetadata::F32 { name, value } => {
                self.render_f32_field(name, **value, cx).into_any_element()
            },
            ComponentFieldMetadata::Bool { name, value } => {
                self.render_bool_field(name, **value, cx).into_any_element()
            },
            ComponentFieldMetadata::String { name, value } => {
                self.render_string_field(name, value, cx).into_any_element()
            },
            ComponentFieldMetadata::Color { name, value } => {
                self.render_color_field(name, **value, cx).into_any_element()
            },
            ComponentFieldMetadata::Custom { name, type_name, ui_key, value_ptr } => {
                self.render_custom_field(name, type_name, ui_key, *value_ptr, cx).into_any_element()
            },
            _ => div().into_any_element(),
        }
    }
    
    fn render_custom_field(
        &self, 
        label: &'static str, 
        type_name: &'static str,
        ui_key: &'static str,
        value_ptr: *const (),
        cx: &Context<Self>
    ) -> impl IntoElement {
        // Check if there's a custom renderer registered for this ui_key
        if let Some(renderer) = self.custom_renderers.get(ui_key) {
            return renderer(label, value_ptr, cx);
        }
        
        // Fallback: placeholder for custom UI rendering
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} (custom: {})", label, type_name))
            )
            .child(
                div()
                    .w_full()
                    .h_7()
                    .items_center()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_3()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(format!("Custom UI: {} (no renderer registered)", ui_key))
            )
            .into_any_element()
    }
    
    fn render_f32_field(&self, label: &'static str, value: f32, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label)
            )
            .child(
                h_flex()
                    .w_full()
                    .h_7()
                    .items_center()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_2()
                    .child(
                        div()
                            .text_sm()
                            .child(format!("{:.2}", value))
                    )
            )
    }
    
    fn render_bool_field(&self, label: &'static str, value: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w_4()
                    .h_4()
                    .rounded(px(2.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .when(value, |this| {
                        this.bg(cx.theme().primary)
                    })
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(label)
            )
    }
    
    fn render_string_field(&self, label: &'static str, value: &String, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label)
            )
            .child(
                h_flex()
                    .w_full()
                    .h_7()
                    .items_center()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .px_2()
                    .child(
                        div()
                            .text_sm()
                            .child(value.clone())
                    )
            )
    }
    
    fn render_color_field(&self, label: &'static str, value: [f32; 4], cx: &Context<Self>) -> impl IntoElement {
        let labels = ["R", "G", "B", "A"];
        let colors = [
            Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 },
            Hsla { h: 0.33, s: 0.8, l: 0.4, a: 1.0 },
            Hsla { h: 0.61, s: 0.8, l: 0.55, a: 1.0 },
            Hsla { h: 0.0, s: 0.0, l: 0.6, a: 1.0 },
        ];
        
        let fields: Vec<_> = (0..4).map(|i| {
            h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(cx.theme().border)
                .px_2()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors[i])
                        .child(labels[i])
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .child(format!("{:.2}", value[i]))
                )
        }).collect();
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label)
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(fields)
            )
    }
}
