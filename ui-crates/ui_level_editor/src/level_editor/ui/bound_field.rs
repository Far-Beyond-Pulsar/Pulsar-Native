//! Bound field components that connect UI inputs to scene data with automatic sync
//!
//! This module provides GPUI components that wrap input fields and bind them to scene data
//! using the FieldBinding system. Each field has its own Entity<InputState> and automatically
//! syncs changes bidirectionally between UI and scene data.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
    input::{InputEvent, InputState, NumberInput, TextInput},
    ActiveTheme, IconName, Sizable, StyledExt,
};
use std::sync::Arc;

use crate::level_editor::scene_database::{SceneDatabase, ObjectId};
use super::field_bindings::{FieldBinding, F32FieldBinding, StringFieldBinding, BoolFieldBinding};

// ============================================================================
// F32 Bound Field - For numeric fields
// ============================================================================

/// A UI component that binds a NumberInput to an f32 field in scene data
pub struct F32BoundField {
    binding: Arc<F32FieldBinding>,
    pub input: Entity<InputState>,
    pub label: String,
    object_id: String,
    scene_db: SceneDatabase,
    _subscription: Subscription,
}

impl F32BoundField {
    pub fn new(
        binding: F32FieldBinding,
        label: impl Into<String>,
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let binding = Arc::new(binding);
        let input = cx.new(|cx| InputState::new(window, cx));

        // Initialize input with current value from scene database
        if let Some(value) = binding.get(&object_id, &scene_db) {
            let text = binding.to_string(&value);
            input.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
            });
        }

        // Subscribe to input events for bidirectional sync
        let binding_clone = binding.clone();
        let object_id_clone = object_id.clone();
        let scene_db_clone = scene_db.clone();

        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _state, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change | InputEvent::Blur => {
                        // Parse input text and update scene database
                        this.input.update(cx, |state, _cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    // Update scene database (this records to undo/redo)
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                }
                            }
                        });
                    }
                    _ => {}
                }
            },
        );

        Self {
            binding,
            input,
            label: label.into(),
            object_id,
            scene_db,
            _subscription: subscription,
        }
    }

    /// Update the field when scene data changes externally (e.g., from undo/redo)
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        if let Some(value) = self.binding.get(&self.object_id, &self.scene_db) {
            let text = self.binding.to_string(&value);
            self.input.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
            });
        }
    }
}

impl Render for F32BoundField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone())
            )
            .child(NumberInput::new(&self.input).xsmall())
    }
}

// ============================================================================
// String Bound Field - For text fields
// ============================================================================

/// A UI component that binds a TextInput to a String field in scene data
pub struct StringBoundField {
    binding: Arc<StringFieldBinding>,
    input: Entity<InputState>,
    label: String,
    object_id: String,
    scene_db: SceneDatabase,
    _subscription: Subscription,
}

impl StringBoundField {
    pub fn new(
        binding: StringFieldBinding,
        label: impl Into<String>,
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let binding = Arc::new(binding);
        let input = cx.new(|cx| InputState::new(window, cx));

        // Initialize input with current value
        if let Some(value) = binding.get(&object_id, &scene_db) {
            let text = binding.to_string(&value);
            input.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
            });
        }

        // Subscribe to input events
        let binding_clone = binding.clone();
        let object_id_clone = object_id.clone();
        let scene_db_clone = scene_db.clone();

        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _state, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change | InputEvent::Blur => {
                        this.input.update(cx, |state, _cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                }
                            }
                        });
                    }
                    _ => {}
                }
            },
        );

        Self {
            binding,
            input,
            label: label.into(),
            object_id,
            scene_db,
            _subscription: subscription,
        }
    }

    /// Update the field when scene data changes externally
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        if let Some(value) = self.binding.get(&self.object_id, &self.scene_db) {
            let text = self.binding.to_string(&value);
            self.input.update(cx, |state, cx| {
                state.set_value(&text, window, cx);
            });
        }
    }
}

impl Render for StringBoundField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone())
            )
            .child(TextInput::new(&self.input).flex_1())
    }
}

// ============================================================================
// Bool Bound Field - For boolean fields
// ============================================================================

/// A UI component that binds a Checkbox to a boolean field in scene data
pub struct BoolBoundField {
    binding: Arc<BoolFieldBinding>,
    label: String,
    object_id: String,
    scene_db: SceneDatabase,
    checked: bool,
}

impl BoolBoundField {
    pub fn new(
        binding: BoolFieldBinding,
        label: impl Into<String>,
        object_id: String,
        scene_db: SceneDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let binding = Arc::new(binding);

        // Get initial value
        let checked = binding.get(&object_id, &scene_db).unwrap_or(false);

        Self {
            binding,
            label: label.into(),
            object_id,
            scene_db,
            checked,
        }
    }

    /// Update the field when scene data changes externally
    pub fn refresh(&self, _window: &mut Window, _cx: &mut App) {
        // Re-render will happen automatically on next render
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.checked = !self.checked;
        self.binding.set(&self.object_id, self.checked, &self.scene_db);
        cx.notify();
    }
}

impl Render for BoolBoundField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Refresh checked state from scene database
        if let Some(value) = self.binding.get(&self.object_id, &self.scene_db) {
            self.checked = value;
        }

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                Checkbox::new("checkbox")
                    .label(self.label.clone())
                    .checked(self.checked)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.toggle(cx);
                    }))
            )
    }
}

// ============================================================================
// Vec3 Bound Field - For [f32; 3] fields (position, rotation, scale)
// ============================================================================

/// A UI component that renders three F32 inputs for a Vec3 field
pub struct Vec3BoundField {
    x_field: Entity<F32BoundField>,
    y_field: Entity<F32BoundField>,
    z_field: Entity<F32BoundField>,
    label: String,
}

impl Vec3BoundField {
    pub fn new(
        label: impl Into<String>,
        object_id: String,
        scene_db: SceneDatabase,
        get_vec: impl Fn(&crate::level_editor::scene_database::SceneObjectData) -> [f32; 3] + Send + Sync + 'static,
        set_vec: impl Fn(&mut crate::level_editor::scene_database::SceneObjectData, [f32; 3]) + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::field_bindings::F32FieldBinding;
        use std::sync::Arc;

        // Create closures wrapped in Arc for shared access
        let get_vec = Arc::new(get_vec);
        let set_vec = Arc::new(set_vec);

        // Create X field binding
        let get_vec_x = get_vec.clone();
        let get_vec_x2 = get_vec.clone();
        let set_vec_x = set_vec.clone();
        let x_binding = F32FieldBinding::new(
            move |obj| (get_vec_x)(obj)[0],
            move |obj, val| {
                let mut vec = (get_vec_x2)(obj);
                vec[0] = val;
                (set_vec_x)(obj, vec);
            },
        );

        // Create Y field binding
        let get_vec_y = get_vec.clone();
        let get_vec_y2 = get_vec.clone();
        let set_vec_y = set_vec.clone();
        let y_binding = F32FieldBinding::new(
            move |obj| (get_vec_y)(obj)[1],
            move |obj, val| {
                let mut vec = (get_vec_y2)(obj);
                vec[1] = val;
                (set_vec_y)(obj, vec);
            },
        );

        // Create Z field binding
        let get_vec_z = get_vec.clone();
        let get_vec_z2 = get_vec.clone();
        let set_vec_z = set_vec.clone();
        let z_binding = F32FieldBinding::new(
            move |obj| (get_vec_z)(obj)[2],
            move |obj, val| {
                let mut vec = (get_vec_z2)(obj);
                vec[2] = val;
                (set_vec_z)(obj, vec);
            },
        );

        // Create field entities
        let x_field = cx.new(|cx| {
            F32BoundField::new(x_binding, "X", object_id.clone(), scene_db.clone(), window, cx)
        });

        let y_field = cx.new(|cx| {
            F32BoundField::new(y_binding, "Y", object_id.clone(), scene_db.clone(), window, cx)
        });

        let z_field = cx.new(|cx| {
            F32BoundField::new(z_binding, "Z", object_id, scene_db, window, cx)
        });

        Self {
            x_field,
            y_field,
            z_field,
            label: label.into(),
        }
    }

    /// Update all three fields when scene data changes externally
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        self.x_field.update(cx, |field, cx| field.refresh(window, cx));
        self.y_field.update(cx, |field, cx| field.refresh(window, cx));
        self.z_field.update(cx, |field, cx| field.refresh(window, cx));
    }
}

impl Render for Vec3BoundField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone())
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(self.x_field.clone())
                    .child(self.y_field.clone())
                    .child(self.z_field.clone())
            )
    }
}
