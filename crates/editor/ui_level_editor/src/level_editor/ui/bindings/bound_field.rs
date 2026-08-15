//! Bound field components that connect UI inputs to scene data with automatic sync
//!
//! This module provides GPUI components that wrap input fields and bind them to scene data
//! using the FieldBinding system. Each field has its own Entity<InputState> and automatically
//! syncs changes bidirectionally between UI and scene data.

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{
    button::ButtonVariants as _,
    checkbox::Checkbox,
    h_flex,
    input::{InputEvent, InputState, NumberInput, NumberInputEvent, StepAction, TextInput},
    v_flex, ActiveTheme, Sizable,
};

use super::field_bindings::{BoolFieldBinding, F32FieldBinding, FieldBinding, StringFieldBinding};
use crate::level_editor::scene_database::SceneDatabase;

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
    _step_subscription: Subscription,
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
                    InputEvent::Change => {
                        // Parse and update on every keystroke, but don't reformat yet
                        this.input.update(cx, |state, _cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    // `binding.set` -- whether this is
                                    // undo-tracked depends entirely on which
                                    // `FieldBinding` was constructed with
                                    // (Pulsar-Native#561): every live caller
                                    // now uses a `*_with_db` binding whose
                                    // setter runs `execute_command` (see
                                    // `TransformSection`/
                                    // `ObjectHeaderSection`), so it IS
                                    // undo-tracked in practice today -- but
                                    // this type itself doesn't guarantee
                                    // that, since the plain `new()`
                                    // whole-object constructor's `set()`
                                    // still calls `db.update_object`
                                    // directly, which is NOT undo-tracked.
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                }
                            }
                        });
                    }
                    InputEvent::Blur => {
                        // On blur, parse, update, and reformat to canonical form
                        this.input.update(cx, |state, cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    // Update scene database
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                    // Reformat to canonical display
                                    let formatted = this.binding.to_string(&value);
                                    state.set_value(&formatted, window, cx);
                                }
                            }
                        });
                    }
                    _ => {}
                }
            },
        );

        // Subscribe to step events (increment/decrement buttons)
        let step_subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _state, event: &NumberInputEvent, window, cx| {
                match event {
                    NumberInputEvent::Step { action, fine } => {
                        this.input.update(cx, |state, cx| {
                            let text = state.text().to_string();
                            if let Ok(mut value) = this.binding.from_string(&text) {
                                let step = if *fine { 0.1 } else { 1.0 };
                                match action {
                                    StepAction::Increment => value += step,
                                    StepAction::Decrement => value -= step,
                                }

                                if this.binding.validate(&value).is_ok() {
                                    // Update scene database
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                    // Update display
                                    let formatted = this.binding.to_string(&value);
                                    state.set_value(&formatted, window, cx);
                                }
                            }
                        });
                    }
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
            _step_subscription: step_subscription,
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
                    .child(self.label.clone()),
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
    pub input: Entity<InputState>,
    pub label: String,
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
                    InputEvent::Change => {
                        // Update on keystroke without reformatting
                        this.input.update(cx, |state, _cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                }
                            }
                        });
                    }
                    InputEvent::Blur => {
                        // On blur, update and reformat
                        this.input.update(cx, |state, cx| {
                            let text = state.text().to_string();
                            if let Ok(value) = this.binding.from_string(&text) {
                                if this.binding.validate(&value).is_ok() {
                                    // Reformat BEFORE updating (so we can use value after)
                                    let formatted = this.binding.to_string(&value);
                                    this.binding.set(&this.object_id, value, &this.scene_db);
                                    // Update display with canonical form (e.g., trim whitespace)
                                    state.set_value(&formatted, window, cx);
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
                    .child(self.label.clone()),
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
        self.binding
            .set(&self.object_id, self.checked, &self.scene_db);
        cx.notify();
    }
}

impl Render for BoolBoundField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Refresh checked state from scene database
        if let Some(value) = self.binding.get(&self.object_id, &self.scene_db) {
            self.checked = value;
        }

        h_flex().w_full().gap_2().items_center().child(
            Checkbox::new("checkbox")
                .label(self.label.clone())
                .checked(self.checked)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle(cx);
                })),
        )
    }
}
