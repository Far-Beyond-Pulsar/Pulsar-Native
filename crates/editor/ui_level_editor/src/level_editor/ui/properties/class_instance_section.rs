//! Class Instance Section - the placed class of the selected object (#921).
//!
//! For an object carrying a `ClassInstance`: the class name, its script
//! variables and its prefab component slots, with overridden values marked
//! and a "revert to class default" action per value, per slot and per
//! variable. The data comes from `scene_edit::classes`.

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme, Sizable};

use crate::level_editor::scene_edit::classes::{self, ClassInstanceView};
use crate::level_editor::state::LevelEditorState;
use pulsar_class::ClassRegistry;

pub struct ClassInstanceSection {
    object_id: String,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    registry: ClassRegistry,
    view: Option<ClassInstanceView>,
}

impl ClassInstanceSection {
    pub fn new(
        object_id: String,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        let mut section = Self {
            object_id,
            state,
            registry: classes::project_registry(),
            view: None,
        };
        section.reload();
        section
    }

    /// Whether the selected object is a class instance at all.
    pub fn is_class_instance(&self) -> bool {
        self.view.is_some()
    }

    fn reload(&mut self) {
        let state = self.state.read();
        let world = state.scene.world();
        self.view = classes::class_instance_view(&world, &self.object_id, &self.registry);
    }

    /// Re-read the instance after a scene change.
    pub fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let before = self.view.clone();
        self.reload();
        if before != self.view {
            cx.notify();
        }
    }

    fn apply(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut pulsar_scenedb::World, &str, &ClassRegistry) -> bool,
    ) {
        let changed = {
            let state = self.state.read();
            let mut world = state.scene.world_mut();
            let changed = edit(&mut world, &self.object_id, &self.registry);
            if changed {
                // A slot revert rebuilds generated children: arm their rows.
                use engine_backend::scene::SceneWorldExt;
                let mut stack: Vec<_> = world.entity_for(&self.object_id).into_iter().collect();
                while let Some(entity) = stack.pop() {
                    engine_backend::scene::arm_render_row_subscriptions_for_entity(
                        &mut world, entity,
                    );
                    stack.extend(world.children_of(Some(entity)));
                }
            }
            changed
        };
        if changed {
            self.state.write().scene.bump_revision(true);
            self.reload();
            cx.notify();
        }
    }

    fn revert_button(
        id: impl Into<ElementId>,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::new(id)
            .label("Revert")
            .ghost()
            .xsmall()
            .tooltip("Revert to class default")
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
    }
}

fn short(value: &serde_json::Value) -> String {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > 32 {
        format!("{}…", text.chars().take(32).collect::<String>())
    } else {
        text
    }
}

impl Render for ClassInstanceSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.view.clone() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().primary;

        let mut body = v_flex().w_full().gap_1();
        body = body.child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Class: {}", view.class_name)),
                )
                .when(!view.resolved, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child("missing from project"),
                    )
                }),
        );

        if !view.variables.is_empty() {
            body = body.child(div().pt_1().text_xs().text_color(muted).child("Variables"));
            for (i, var) in view.variables.iter().enumerate() {
                let name = var.name.clone();
                let mut row = h_flex()
                    .w_full()
                    .gap_2()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .when(var.overridden, |d| {
                                d.text_color(accent).font_weight(FontWeight::SEMIBOLD)
                            })
                            .child(if var.overridden {
                                format!("● {}", var.name)
                            } else {
                                var.name.clone()
                            }),
                    )
                    .child(div().text_sm().child(short(&var.value)));
                if var.overridden {
                    row = row.child(Self::revert_button(
                        ("class-var-revert", i),
                        cx,
                        move |this, cx| {
                            let name = name.clone();
                            this.apply(cx, move |world, id, _| {
                                classes::revert_variable(world, id, &name)
                            });
                        },
                    ));
                }
                body = body.child(row);
            }
        }

        if !view.slots.is_empty() {
            body = body.child(div().pt_1().text_xs().text_color(muted).child("Components"));
            for (i, slot) in view.slots.iter().enumerate() {
                let overridden = slot.removed || !slot.overridden.is_empty();
                let slot_id = slot.slot_id.clone();
                let mut header = h_flex().w_full().gap_2().justify_between().child(
                    div()
                        .text_sm()
                        .when(overridden, |d| {
                            d.text_color(accent).font_weight(FontWeight::SEMIBOLD)
                        })
                        .child(format!(
                            "{}{} ({}){}",
                            if overridden { "● " } else { "" },
                            slot.class_name,
                            slot.slot_id,
                            if slot.removed { " — removed" } else { "" }
                        )),
                );
                if overridden {
                    header = header.child(Self::revert_button(
                        ("class-slot-revert", i),
                        cx,
                        move |this, cx| {
                            let slot_id = slot_id.clone();
                            this.apply(cx, move |world, id, registry| {
                                classes::revert_slot(world, id, &slot_id, None, registry)
                            });
                        },
                    ));
                }
                body = body.child(header);
                for (j, prop) in slot.overridden.iter().enumerate() {
                    let slot_id = slot.slot_id.clone();
                    let path = prop.path.clone();
                    body = body.child(
                        h_flex()
                            .w_full()
                            .pl_3()
                            .gap_2()
                            .justify_between()
                            .child(div().text_xs().text_color(accent).child(prop.path.clone()))
                            .child(div().text_xs().child(format!(
                                "{} (class: {})",
                                short(&prop.value),
                                short(&prop.default)
                            )))
                            .child(Self::revert_button(
                                SharedString::from(format!("class-prop-revert-{i}-{j}")),
                                cx,
                                move |this, cx| {
                                    let slot_id = slot_id.clone();
                                    let path = path.clone();
                                    this.apply(cx, move |world, id, registry| {
                                        classes::revert_slot(
                                            world,
                                            id,
                                            &slot_id,
                                            Some(&path),
                                            registry,
                                        )
                                    });
                                },
                            )),
                    );
                }
            }
        }

        v_flex()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .child(body)
            .into_any_element()
    }
}
