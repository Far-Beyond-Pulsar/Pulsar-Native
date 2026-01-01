//! Material Section - Edit material component properties
//!
//! This component provides editable fields for material properties including
//! color (RGBA), metallic, and roughness values.

use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt,
};

use crate::level_editor::scene_database::{SceneDatabase, Component};
use super::bound_field::F32BoundField;

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
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::field_bindings::F32FieldBinding;

        // Helper to find material component
        let find_material = |obj: &crate::level_editor::scene_database::SceneObjectData| {
            obj.components.iter().find_map(|comp| {
                if let Component::Material { color, metallic, roughness, .. } = comp {
                    Some((*color, *metallic, *roughness))
                } else {
                    None
                }
            })
        };

        // Color R
        let color_r = cx.new(|cx| {
            F32BoundField::new(
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(c, _, _)| c[0]).unwrap_or(1.0),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { color, .. } = comp {
                                color[0] = val;
                                break;
                            }
                        }
                    },
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
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(c, _, _)| c[1]).unwrap_or(1.0),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { color, .. } = comp {
                                color[1] = val;
                                break;
                            }
                        }
                    },
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
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(c, _, _)| c[2]).unwrap_or(1.0),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { color, .. } = comp {
                                color[2] = val;
                                break;
                            }
                        }
                    },
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
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(c, _, _)| c[3]).unwrap_or(1.0),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { color, .. } = comp {
                                color[3] = val;
                                break;
                            }
                        }
                    },
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
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(_, m, _)| m).unwrap_or(0.0),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { metallic, .. } = comp {
                                *metallic = val;
                                break;
                            }
                        }
                    },
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
                F32FieldBinding::new(
                    move |obj| find_material(obj).map(|(_, _, r)| r).unwrap_or(0.5),
                    |obj, val| {
                        for comp in &mut obj.components {
                            if let Component::Material { roughness, .. } = comp {
                                *roughness = val;
                                break;
                            }
                        }
                    },
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
        self.color_r.update(cx, |field, cx| field.refresh(window, cx));
        self.color_g.update(cx, |field, cx| field.refresh(window, cx));
        self.color_b.update(cx, |field, cx| field.refresh(window, cx));
        self.color_a.update(cx, |field, cx| field.refresh(window, cx));
        self.metallic.update(cx, |field, cx| field.refresh(window, cx));
        self.roughness.update(cx, |field, cx| field.refresh(window, cx));
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
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(self.render_color_channel(r, Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 }, cx))
                    .child(self.render_color_channel(g, Hsla { h: 120.0, s: 0.8, l: 0.4, a: 1.0 }, cx))
                    .child(self.render_color_channel(b, Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, cx))
                    .child(self.render_color_channel(a, Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0 }, cx))
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
            )
    }
}

impl Render for MaterialSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chevron_icon = if self.collapsed { IconName::ChevronRight } else { IconName::ChevronDown };

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
                        ui::Icon::new(IconName::Palette)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("Material")
                    )
            )
            .when(!self.collapsed, |this| {
                this.child(
                    // Section content
                    div()
                        .w_full()
                        .p_3()
                        .bg(cx.theme().background)
                        .child(
                            v_flex()
                                .gap_3()
                                .child(self.render_color_field("Color", &self.color_r, &self.color_g, &self.color_b, &self.color_a, cx))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .child(self.metallic.clone())
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .child(self.roughness.clone())
                                        )
                                )
                        )
                )
            })
    }
}
