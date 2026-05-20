//! Material Section - Edit material component properties
//!
//! This component provides editable fields for material properties including
//! color (RGBA), metallic, and roughness values.

use gpui::{prelude::*, *};
use serde_json::Value;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable};

use super::bound_field::F32BoundField;
use crate::level_editor::scene_database::SceneDatabase;

/// Material section for editing material component properties
pub struct MaterialSection {
    // Color RGBA components
    color_r: Entity<F32BoundField>,
    color_g: Entity<F32BoundField>,
    color_b: Entity<F32BoundField>,
    color_a: Entity<F32BoundField>,

    // Material properties
    metallic: Entity<F32BoundField>,
    roughness: Entity<F32BoundField>,

    object_id: String,
    collapsed: bool,
}

impl MaterialSection {
    fn get_material_data(scene_db: &SceneDatabase, object_id: &str) -> Option<Value> {
        scene_db
            .get_components(&object_id.to_string())
            .into_iter()
            .find(|c| c.class_name == "MaterialOverride")
            .map(|c| c.data)
    }

    fn get_color_channel(scene_db: &SceneDatabase, object_id: &str, index: usize) -> Option<f32> {
        let data = Self::get_material_data(scene_db, object_id)?;
        let color = data.get("color")?.as_array()?;
        color.get(index)?.as_f64().map(|v| v as f32)
    }

    fn set_color_channel(scene_db: &SceneDatabase, object_id: &str, index: usize, value: f32) -> bool {
        let mut color = [1.0_f32, 1.0_f32, 1.0_f32, 1.0_f32];
        if let Some(data) = Self::get_material_data(scene_db, object_id) {
            if let Some(existing) = data.get("color").and_then(|v| v.as_array()) {
                for (i, c) in color.iter_mut().enumerate() {
                    if let Some(v) = existing.get(i).and_then(|v| v.as_f64()) {
                        *c = v as f32;
                    }
                }
            }
        }
        if index < color.len() {
            color[index] = value;
        }
        scene_db.update_component_property(
            &object_id.to_string(),
            "MaterialOverride",
            "color",
            serde_json::json!([color[0], color[1], color[2], color[3]]),
        );
        true
    }

    fn get_scalar(scene_db: &SceneDatabase, object_id: &str, key: &str) -> Option<f32> {
        Self::get_material_data(scene_db, object_id)?
            .get(key)
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
    }

    fn set_scalar(scene_db: &SceneDatabase, object_id: &str, key: &str, value: f32) -> bool {
        scene_db.update_component_property(
            &object_id.to_string(),
            "MaterialOverride",
            key,
            Value::from(value),
        );
        true
    }

    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::field_bindings::F32FieldBinding;

        // Color R
        let color_r = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_color_channel(db, id, 0).or(Some(1.0)),
                    |id, val, db| Self::set_color_channel(db, id, 0, val),
                ),
                "R",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Color G
        let color_g = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_color_channel(db, id, 1).or(Some(1.0)),
                    |id, val, db| Self::set_color_channel(db, id, 1, val),
                ),
                "G",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Color B
        let color_b = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_color_channel(db, id, 2).or(Some(1.0)),
                    |id, val, db| Self::set_color_channel(db, id, 2, val),
                ),
                "B",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Color A
        let color_a = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_color_channel(db, id, 3).or(Some(1.0)),
                    |id, val, db| Self::set_color_channel(db, id, 3, val),
                ),
                "A",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Metallic
        let metallic = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_scalar(db, id, "metallic").or(Some(0.0)),
                    |id, val, db| Self::set_scalar(db, id, "metallic", val),
                ),
                "Metallic",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Roughness
        let roughness = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new_with_db(
                    |id, db| Self::get_scalar(db, id, "roughness").or(Some(0.5)),
                    |id, val, db| Self::set_scalar(db, id, "roughness", val),
                ),
                "Roughness",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        Self {
            color_r,
            color_g,
            color_b,
            color_a,
            metallic,
            roughness,
            object_id,
            collapsed: false,
        }
    }

    /// Refresh all fields when scene data changes externally
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        self.color_r
            .update(cx, |field, cx| field.refresh(window, cx));
        self.color_g
            .update(cx, |field, cx| field.refresh(window, cx));
        self.color_b
            .update(cx, |field, cx| field.refresh(window, cx));
        self.color_a
            .update(cx, |field, cx| field.refresh(window, cx));
        self.metallic
            .update(cx, |field, cx| field.refresh(window, cx));
        self.roughness
            .update(cx, |field, cx| field.refresh(window, cx));
    }

    fn toggle_collapsed(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    /// Render color field with RGBA components
    fn render_color_field(
        &self,
        label: &str,
        r: &Entity<F32BoundField>,
        g: &Entity<F32BoundField>,
        b: &Entity<F32BoundField>,
        a: &Entity<F32BoundField>,
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
                    .child(self.render_color_channel(
                        r,
                        Hsla {
                            h: 0.0,
                            s: 0.8,
                            l: 0.5,
                            a: 1.0,
                        },
                        cx,
                    ))
                    .child(self.render_color_channel(
                        g,
                        Hsla {
                            h: 120.0,
                            s: 0.8,
                            l: 0.4,
                            a: 1.0,
                        },
                        cx,
                    ))
                    .child(self.render_color_channel(
                        b,
                        Hsla {
                            h: 220.0,
                            s: 0.8,
                            l: 0.55,
                            a: 1.0,
                        },
                        cx,
                    ))
                    .child(self.render_color_channel(
                        a,
                        Hsla {
                            h: 0.0,
                            s: 0.0,
                            l: 0.5,
                            a: 1.0,
                        },
                        cx,
                    )),
            )
    }

    /// Render a single color channel with colored indicator
    fn render_color_channel(
        &self,
        field: &Entity<F32BoundField>,
        channel_color: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                // Channel label with color indicator
                div()
                    .w_6()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(channel_color.opacity(0.2))
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(channel_color)
                            .child(label),
                    ),
            )
            .child(
                div().flex_1().h_full().child(
                    ui::input::NumberInput::new(&input)
                        .appearance(false)
                        .xsmall(),
                ),
            )
    }
}

impl Render for MaterialSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                // Section header
                h_flex()
                    .id("material-section-header")
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
                        ui::Icon::new(IconName::Palette)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("Material"),
                    ),
            )
            .when(!self.collapsed, |this| {
                this.child(
                    // Section content
                    div().w_full().p_3().bg(cx.theme().background).child(
                        v_flex()
                            .gap_3()
                            .child(self.render_color_field(
                                "Color",
                                &self.color_r,
                                &self.color_g,
                                &self.color_b,
                                &self.color_a,
                                cx,
                            ))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .child(div().flex_1().child(self.metallic.clone()))
                                    .child(div().flex_1().child(self.roughness.clone())),
                            ),
                    ),
                )
            })
    }
}
