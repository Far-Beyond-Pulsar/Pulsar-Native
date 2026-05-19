//! Dynamic Component Fields Section - Renders component fields using trait-based registry

use gpui::{prelude::*, *};
use std::collections::HashMap;
use std::sync::Arc;
use ui::{
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex, v_flex, ActiveTheme,
};

use crate::level_editor::scene_database::SceneDatabase;
use engine_backend::scene::ComponentFieldMetadata;

/// Type for custom field renderer functions
pub type CustomFieldRenderer =
    Arc<dyn Fn(&str, *const (), &Context<ComponentFieldsSection>) -> AnyElement + Send + Sync>;

/// Dynamic section that renders component fields based on trait-based registry
pub struct ComponentFieldsSection {
    component_index: usize,
    object_id: String,
    scene_db: SceneDatabase,
    custom_renderers: HashMap<String, CustomFieldRenderer>,
    /// One color-picker state per color field, keyed by field name.
    color_pickers: HashMap<&'static str, Entity<ColorPickerState>>,
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
            color_pickers: HashMap::new(),
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
    pub fn register_custom_renderer(
        &mut self,
        ui_key: impl Into<String>,
        renderer: CustomFieldRenderer,
    ) {
        self.custom_renderers.insert(ui_key.into(), renderer);
    }
}

impl Render for ComponentFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let obj = self.scene_db.get_object(&self.object_id);
        let (variant_name, field_metadata) = obj
            .as_ref()
            .and_then(|obj| {
                obj.components
                    .get(self.component_index)
                    .map(|c| (c.variant_name(), c.get_field_metadata()))
            })
            .unwrap_or(("Component", vec![]));

        let fields: Vec<AnyElement> = field_metadata
            .iter()
            .map(|field_meta| self.render_field(field_meta, window, cx))
            .collect();

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex().w_full().items_center().justify_between().child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(variant_name),
                ),
            )
            .children(fields)
    }
}

impl ComponentFieldsSection {
    fn render_field(
        &mut self,
        field_meta: &ComponentFieldMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match field_meta {
            ComponentFieldMetadata::F32 { name, value } => {
                self.render_f32_field(name, **value, cx).into_any_element()
            }
            ComponentFieldMetadata::Bool { name, value } => {
                self.render_bool_field(name, **value, cx).into_any_element()
            }
            ComponentFieldMetadata::String { name, value } => {
                self.render_string_field(name, value, cx).into_any_element()
            }
            ComponentFieldMetadata::Color { name, value } => {
                self.render_color_field(name, **value, window, cx).into_any_element()
            }
            ComponentFieldMetadata::Custom { name, type_name, ui_key, value_ptr } => self
                .render_custom_field(name, type_name, ui_key, *value_ptr, cx)
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn render_custom_field(
        &self,
        label: &'static str,
        type_name: &'static str,
        ui_key: &'static str,
        value_ptr: *const (),
        cx: &Context<Self>,
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
                    .child(format!("{} (custom: {})", label, type_name)),
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
                    .child(format!("Custom UI: {} (no renderer registered)", ui_key)),
            )
            .into_any_element()
    }

    fn render_f32_field(
        &self,
        label: &'static str,
        value: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
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
                    .child(div().text_sm().child(format!("{:.2}", value))),
            )
    }

    fn render_bool_field(
        &self,
        label: &'static str,
        value: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
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
                    .when(value, |this| this.bg(cx.theme().primary)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(label),
            )
    }

    fn render_string_field(
        &self,
        label: &'static str,
        value: &String,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
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
                    .child(div().text_sm().child(value.clone())),
            )
    }

    fn render_color_field(
        &mut self,
        label: &'static str,
        value: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Get or create the ColorPickerState for this field.
        let picker_state = self
            .color_pickers
            .entry(label)
            .or_insert_with(|| {
                cx.new(|cx| {
                    let mut state = ColorPickerState::new(window, cx);
                    // Seed the picker with the current component color.
                    let hsla = rgba_to_hsla(value);
                    state.set_value(hsla, window, cx);
                    state
                })
            })
            .clone();

        // Subscribe to color changes and write them back to the component.
        let scene_db = self.scene_db.clone();
        let object_id = self.object_id.clone();
        let component_index = self.component_index;
        cx.subscribe_in(&picker_state, window, move |_this, _picker, ev, _w, _cx| {
            if let ColorPickerEvent::Change(Some(hsla)) = ev {
                let rgba = hsla_to_rgba(*hsla);
                let mut obj = match scene_db.get_object(&object_id) {
                    Some(o) => o,
                    None => return,
                };
                if let Some(engine_backend::scene::Component::Light { color, .. }) =
                    obj.components.get_mut(component_index)
                {
                    *color = rgba;
                    scene_db.update_object(obj);
                }
            }
        }).detach();

        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(ColorPicker::new(&picker_state).label(label))
    }
}

/// Convert linear [r, g, b, a] (0..1) to `Hsla`.
fn rgba_to_hsla([r, g, b, a]: [f32; 4]) -> Hsla {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let s = if max == min {
        0.0
    } else if l < 0.5 {
        (max - min) / (max + min)
    } else {
        (max - min) / (2.0 - max - min)
    };
    let h = if max == min {
        0.0
    } else if max == r {
        ((g - b) / (max - min)).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / (max - min) + 2.0) / 6.0
    } else {
        ((r - g) / (max - min) + 4.0) / 6.0
    };
    Hsla { h, s, l, a }
}

/// Convert `Hsla` back to linear [r, g, b, a] (0..1).
fn hsla_to_rgba(Hsla { h, s, l, a }: Hsla) -> [f32; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r1 + m, g1 + m, b1 + m, a]
}
