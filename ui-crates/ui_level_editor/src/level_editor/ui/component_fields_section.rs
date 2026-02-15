//! Dynamic Component Fields Section - Renders component fields based on introspected metadata
//!
//! This component uses the compile-time generated field metadata to dynamically
//! create appropriate bound fields for any component variant.

use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt,
};

use crate::level_editor::scene_database::{SceneDatabase, Component};
use engine_backend::scene::FieldTypeInfo;
use super::bound_field::{F32BoundField, BoolBoundField, StringBoundField};
use super::field_bindings::{F32FieldBinding, BoolFieldBinding, StringFieldBinding};

/// Enum to hold different field entity types
enum FieldEntity {
    F32(Entity<F32BoundField>),
    Bool(Entity<BoolBoundField>),
    String(Entity<StringBoundField>),
    Vec3([Entity<F32BoundField>; 3]),
    Color([Entity<F32BoundField>; 4]),
}

/// Dynamic section that renders component fields based on introspected metadata
pub struct ComponentFieldsSection {
    component_index: usize,
    object_id: String,
    scene_db: SceneDatabase,
    variant_name: String,
    // Store field entities to avoid recreating on every render
    fields: Vec<FieldEntity>,
}

impl ComponentFieldsSection {
    pub fn new(
        component_index: usize,
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Get the component variant name and metadata
        let (variant_name, field_metadata) = scene_db.get_object(&object_id)
            .map(|obj| {
                obj.components.get(component_index)
                    .map(|c| (c.variant_name().to_string(), c.field_metadata()))
                    .unwrap_or_else(|| ("Component".to_string(), vec![]))
            })
            .unwrap_or_else(|| ("Component".to_string(), vec![]));
        
        // Create field entities once during construction
        let fields: Vec<FieldEntity> = field_metadata.iter()
            .map(|&(field_name, field_type)| {
                Self::create_field_entity_wrapped(
                    field_name,
                    field_type,
                    component_index,
                    &object_id,
                    &scene_db,
                    window,
                    cx,
                )
            })
            .collect();
        
        Self {
            component_index,
            object_id,
            scene_db,
            variant_name,
            fields,
        }
    }
    
    fn create_field_entity_wrapped(
        field_name: &'static str,
        field_type: FieldTypeInfo,
        component_index: usize,
        object_id: &str,
        scene_db: &SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FieldEntity {
        match field_type {
            FieldTypeInfo::F32 | FieldTypeInfo::F64 | FieldTypeInfo::I32 | FieldTypeInfo::I64 | FieldTypeInfo::U32 | FieldTypeInfo::U64 => {
                let field = cx.new(|cx| {
                    F32BoundField::new(
                        F32FieldBinding::new(
                            move |obj| {
                                Self::extract_f32_field(&obj.components, component_index, field_name)
                            },
                            move |obj, val| {
                                Self::set_f32_field(&mut obj.components, component_index, field_name, val);
                            },
                        ),
                        field_name,
                        object_id.to_string(),
                        scene_db.clone(),
                        window,
                        cx,
                    )
                });
                FieldEntity::F32(field)
            },
            
            FieldTypeInfo::Bool => {
                let field = cx.new(|cx| {
                    BoolBoundField::new(
                        BoolFieldBinding::new(
                            move |obj| {
                                Self::extract_bool_field(&obj.components, component_index, field_name)
                            },
                            move |obj, val| {
                                Self::set_bool_field(&mut obj.components, component_index, field_name, val);
                            },
                        ),
                        field_name,
                        object_id.to_string(),
                        scene_db.clone(),
                        window,
                        cx,
                    )
                });
                FieldEntity::Bool(field)
            },
            
            FieldTypeInfo::String => {
                let field = cx.new(|cx| {
                    StringBoundField::new(
                        StringFieldBinding::new(
                            move |obj| {
                                Self::extract_string_field(&obj.components, component_index, field_name)
                            },
                            move |obj, val| {
                                Self::set_string_field(&mut obj.components, component_index, field_name, val);
                            },
                        ),
                        field_name,
                        object_id.to_string(),
                        scene_db.clone(),
                        window,
                        cx,
                    )
                });
                FieldEntity::String(field)
            },
            
            FieldTypeInfo::F32Array(3) => {
                let labels = ["X", "Y", "Z"];
                let entities: [Entity<F32BoundField>; 3] = std::array::from_fn(|i| {
                    cx.new(|cx| {
                        F32BoundField::new(
                            F32FieldBinding::new(
                                move |obj| {
                                    Self::extract_vec3_component(&obj.components, component_index, field_name, i)
                                },
                                move |obj, val| {
                                    Self::set_vec3_component(&mut obj.components, component_index, field_name, i, val);
                                },
                            ),
                            labels[i],
                            object_id.to_string(),
                            scene_db.clone(),
                            window,
                            cx,
                        )
                    })
                });
                FieldEntity::Vec3(entities)
            },
            
            FieldTypeInfo::F32Array(4) => {
                let labels = ["R", "G", "B", "A"];
                let entities: [Entity<F32BoundField>; 4] = std::array::from_fn(|i| {
                    cx.new(|cx| {
                        F32BoundField::new(
                            F32FieldBinding::new(
                                move |obj| {
                                    Self::extract_color_component(&obj.components, component_index, field_name, i)
                                },
                                move |obj, val| {
                                    Self::set_color_component(&mut obj.components, component_index, field_name, i, val);
                                },
                            ),
                            labels[i],
                            object_id.to_string(),
                            scene_db.clone(),
                            window,
                            cx,
                        )
                    })
                });
                FieldEntity::Color(entities)
            },
            
            _ => {
                // Unsupported type - create a placeholder F32 field
                let field = cx.new(|cx| {
                    F32BoundField::new(
                        F32FieldBinding::new(
                            move |_obj| 0.0,
                            move |_obj, _val| {},
                        ),
                        "<unsupported>",
                        object_id.to_string(),
                        scene_db.clone(),
                        window,
                        cx,
                    )
                });
                FieldEntity::F32(field)
            }
        }
    }
    
    fn render_f32_field(entity: &Entity<F32BoundField>, cx: &Context<Self>) -> impl IntoElement {
        let input = entity.read(cx).input.clone();
        let label = entity.read(cx).label.clone();
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
                        ui::input::NumberInput::new(&input)
                            .appearance(false)
                            .xsmall()
                    )
            )
    }
    
    fn render_string_field(entity: &Entity<StringBoundField>, cx: &Context<Self>) -> impl IntoElement {
        let input = entity.read(cx).input.clone();
        let label = entity.read(cx).label.clone();
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
                        ui::input::TextInput::new(&input)
                            .appearance(false)
                            .xsmall()
                    )
            )
    }
    
    fn render_vec3_field(entities: &[Entity<F32BoundField>; 3], cx: &Context<Self>) -> impl IntoElement {
        let colors = [
            Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 },   // Red
            Hsla { h: 120.0, s: 0.8, l: 0.4, a: 1.0 }, // Green
            Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue
        ];
        
        let first_label = entities[0].read(cx).label.clone();
        // Extract field name from first entity's parent (assumes pattern like "size")
        let field_name = if first_label == "X" || first_label == "Y" || first_label == "Z" {
            "Vec3" // Generic name if we can't determine
        } else {
            "Vec3"
        };
        
        let fields: Vec<_> = entities.iter().enumerate().map(|(i, entity)| {
            let input = entity.read(cx).input.clone();
            let axis_label = entity.read(cx).label.clone();
            h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(cx.theme().border)
                .overflow_hidden()
                .child(
                    div()
                        .w_6()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors[i].opacity(0.2))
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors[i])
                                .child(axis_label)
                        )
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .child(
                            ui::input::NumberInput::new(&input)
                                .appearance(false)
                                .xsmall()
                        )
                )
        }).collect();
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(field_name)
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(fields)
            )
    }
    
    fn render_color_field(entities: &[Entity<F32BoundField>; 4], cx: &Context<Self>) -> impl IntoElement {
        let colors = [
            Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 },   // Red
            Hsla { h: 120.0, s: 0.8, l: 0.4, a: 1.0 }, // Green
            Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue
            Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0 },   // Gray for Alpha
        ];
        
        let fields: Vec<_> = entities.iter().enumerate().map(|(i, entity)| {
            let input = entity.read(cx).input.clone();
            let channel_label = entity.read(cx).label.clone();
            h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(cx.theme().border)
                .overflow_hidden()
                .child(
                    div()
                        .w_6()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors[i].opacity(0.2))
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors[i])
                                .child(channel_label)
                        )
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .child(
                            ui::input::NumberInput::new(&input)
                                .appearance(false)
                                .xsmall()
                        )
                )
        }).collect();
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Color")
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(fields)
            )
    }
    
    fn create_vec3_field(
        field_name: &'static str,
        component_index: usize,
        object_id: &str,
        scene_db: &SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let labels = ["X", "Y", "Z"];
        let colors = [
            Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 },   // Red
            Hsla { h: 120.0, s: 0.8, l: 0.4, a: 1.0 }, // Green
            Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue
        ];
        
        let mut styled_fields = Vec::new();
        
        for (i, &label) in labels.iter().enumerate() {
            let field = cx.new(|cx| {
                F32BoundField::new(
                    F32FieldBinding::new(
                        move |obj| {
                            Self::extract_vec3_component(&obj.components, component_index, field_name, i)
                        },
                        move |obj, val| {
                            Self::set_vec3_component(&mut obj.components, component_index, field_name, i, val);
                        },
                    ),
                    label,
                    object_id.to_string(),
                    scene_db.clone(),
                    window,
                    cx,
                )
            });
            
            // Style each axis field like TransformSection does
            let input = field.read(cx).input.clone();
            let styled = h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(cx.theme().border)
                .overflow_hidden()
                .child(
                    div()
                        .w_6()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors[i].opacity(0.2))
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors[i])
                                .child(label)
                        )
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .child(
                            ui::input::NumberInput::new(&input)
                                .appearance(false)
                                .xsmall()
                        )
                );
            styled_fields.push(styled);
        }
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(field_name)
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(styled_fields)
            )
            .into_any_element()
    }
    
    fn create_color_field(
        field_name: &'static str,
        component_index: usize,
        object_id: &str,
        scene_db: &SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let labels = ["R", "G", "B", "A"];
        let colors = [
            Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 },    // Red
            Hsla { h: 120.0, s: 0.8, l: 0.4, a: 1.0 },  // Green  
            Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, // Blue
            Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0 },    // Gray for Alpha
        ];
        
        let mut styled_fields = Vec::new();
        
        for (i, &label) in labels.iter().enumerate() {
            let field = cx.new(|cx| {
                F32BoundField::new(
                    F32FieldBinding::new(
                        move |obj| {
                            Self::extract_color_component(&obj.components, component_index, field_name, i)
                        },
                        move |obj, val| {
                            Self::set_color_component(&mut obj.components, component_index, field_name, i, val);
                        },
                    ),
                    label,
                    object_id.to_string(),
                    scene_db.clone(),
                    window,
                    cx,
                )
            });
            
            // Style each color channel field
            let input = field.read(cx).input.clone();
            let styled = h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(cx.theme().border)
                .overflow_hidden()
                .child(
                    div()
                        .w_6()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors[i].opacity(0.2))
                        .border_r_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors[i])
                                .child(label)
                        )
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .child(
                            ui::input::NumberInput::new(&input)
                                .appearance(false)
                                .xsmall()
                        )
                );
            styled_fields.push(styled);
        }
        
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(field_name)
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .children(styled_fields)
            )
            .into_any_element()
    }
    
    // Helper methods to extract/set fields by name using pattern matching
    
    fn extract_f32_field(components: &[Component], index: usize, field_name: &str) -> f32 {
        components.get(index).map(|component| {
            match component {
                Component::Material { metallic, roughness, .. } => {
                    match field_name {
                        "metallic" => *metallic,
                        "roughness" => *roughness,
                        _ => 0.0,
                    }
                },
                Component::RigidBody { mass, .. } => {
                    match field_name {
                        "mass" => *mass,
                        _ => 0.0,
                    }
                },
                Component::Collider { shape } => {
                    use crate::level_editor::scene_database::ColliderShape;
                    match (field_name, shape) {
                        ("radius", ColliderShape::Sphere { radius }) => *radius,
                        ("radius", ColliderShape::Capsule { radius, .. }) => *radius,
                        ("height", ColliderShape::Capsule { height, .. }) => *height,
                        _ => 0.0,
                    }
                },
                _ => 0.0,
            }
        }).unwrap_or(0.0)
    }
    
    fn set_f32_field(components: &mut [Component], index: usize, field_name: &str, value: f32) {
        if let Some(component) = components.get_mut(index) {
            match component {
                Component::Material { metallic, roughness, .. } => {
                    match field_name {
                        "metallic" => *metallic = value,
                        "roughness" => *roughness = value,
                        _ => {},
                    }
                },
                Component::RigidBody { mass, .. } => {
                    match field_name {
                        "mass" => *mass = value,
                        _ => {},
                    }
                },
                Component::Collider { shape } => {
                    use crate::level_editor::scene_database::ColliderShape;
                    match (field_name, shape) {
                        ("radius", ColliderShape::Sphere { radius }) => *radius = value,
                        ("radius", ColliderShape::Capsule { radius, .. }) => *radius = value,
                        ("height", ColliderShape::Capsule { height, .. }) => *height = value,
                        _ => {},
                    }
                },
                _ => {},
            }
        }
    }
    
    fn extract_bool_field(components: &[Component], index: usize, field_name: &str) -> bool {
        components.get(index).map(|component| {
            match component {
                Component::RigidBody { kinematic, .. } => {
                    match field_name {
                        "kinematic" => *kinematic,
                        _ => false,
                    }
                },
                _ => false,
            }
        }).unwrap_or(false)
    }
    
    fn set_bool_field(components: &mut [Component], index: usize, field_name: &str, value: bool) {
        if let Some(component) = components.get_mut(index) {
            match component {
                Component::RigidBody { kinematic, .. } => {
                    match field_name {
                        "kinematic" => *kinematic = value,
                        _ => {},
                    }
                },
                _ => {},
            }
        }
    }
    
    fn extract_string_field(components: &[Component], index: usize, field_name: &str) -> String {
        components.get(index).map(|component| {
            match component {
                Component::Material { id, .. } => {
                    match field_name {
                        "id" => id.clone(),
                        _ => String::new(),
                    }
                },
                Component::Script { path } => {
                    match field_name {
                        "path" => path.clone(),
                        _ => String::new(),
                    }
                },
                _ => String::new(),
            }
        }).unwrap_or_default()
    }
    
    fn set_string_field(components: &mut [Component], index: usize, field_name: &str, value: String) {
        if let Some(component) = components.get_mut(index) {
            match component {
                Component::Material { id, .. } => {
                    match field_name {
                        "id" => *id = value,
                        _ => {},
                    }
                },
                Component::Script { path } => {
                    match field_name {
                        "path" => *path = value,
                        _ => {},
                    }
                },
                _ => {},
            }
        }
    }
    
    fn extract_color_component(components: &[Component], index: usize, field_name: &str, component_index: usize) -> f32 {
        components.get(index).and_then(|component| {
            match component {
                Component::Material { color, .. } => {
                    match field_name {
                        "color" if component_index < 4 => Some(color[component_index]),
                        _ => None,
                    }
                },
                _ => None,
            }
        }).unwrap_or(0.0)
    }
    
    fn set_color_component(components: &mut [Component], index: usize, field_name: &str, component_index: usize, value: f32) {
        if let Some(component) = components.get_mut(index) {
            match component {
                Component::Material { color, .. } => {
                    match field_name {
                        "color" if component_index < 4 => color[component_index] = value,
                        _ => {},
                    }
                },
                _ => {},
            }
        }
    }
    
    fn extract_vec3_component(components: &[Component], index: usize, field_name: &str, component_index: usize) -> f32 {
        components.get(index).and_then(|component| {
            match component {
                Component::Collider { shape } => {
                    use crate::level_editor::scene_database::ColliderShape;
                    match (field_name, shape) {
                        ("size", ColliderShape::Box { size }) if component_index < 3 => Some(size[component_index]),
                        _ => None,
                    }
                },
                _ => None,
            }
        }).unwrap_or(0.0)
    }
    
    fn set_vec3_component(components: &mut [Component], index: usize, field_name: &str, component_index: usize, value: f32) {
        if let Some(component) = components.get_mut(index) {
            match component {
                Component::Collider { shape } => {
                    use crate::level_editor::scene_database::ColliderShape;
                    match (field_name, shape) {
                        ("size", ColliderShape::Box { size }) if component_index < 3 => size[component_index] = value,
                        _ => {},
                    }
                },
                _ => {},
            }
        }
    }
}

impl Render for ComponentFieldsSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut section = v_flex()
            .w_full()
            .gap_3()
            .p_3()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(6.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(self.variant_name.clone())
            );
        
        // Render each field using helper methods
        for field in &self.fields {
            section = section.child(match field {
                FieldEntity::F32(entity) => Self::render_f32_field(entity, cx).into_any_element(),
                FieldEntity::Bool(entity) => entity.clone().into_any_element(),
                FieldEntity::String(entity) => Self::render_string_field(entity, cx).into_any_element(),
                FieldEntity::Vec3(entities) => Self::render_vec3_field(entities, cx).into_any_element(),
                FieldEntity::Color(entities) => Self::render_color_field(entities, cx).into_any_element(),
            });
        }
        
        section
    }
}


