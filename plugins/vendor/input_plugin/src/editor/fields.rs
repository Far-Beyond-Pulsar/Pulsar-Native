//! Text, number and colour inputs that live as long as their panel and are
//! pointed at whatever is selected at the moment they fire.
//!
//! A panel keeps one [`Fields`] and asks it for a field by key each time it
//! renders. The first request creates the input and wires it up; later ones
//! return it and bring its text up to date with the model, unless the user is
//! typing in it. Callbacks must therefore read the current selection from
//! the panel when they run, not capture it when the field is made.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use ui::input::{InputEvent, InputState, NumberInputEvent, StepAction};
use crate::editor::chooser::{ChoiceItem, Chooser, ChosenEvent};
use crate::editor::keypicker::KeyPicker;

/// Limits and display for a number field.
#[derive(Clone, Copy, Debug)]
pub struct NumberSpec {
    pub step: f32,
    pub min: f32,
    pub max: f32,
    pub decimals: usize,
}

/// A number as short text: no trailing zeros.
pub fn format_number(v: f32, decimals: usize) -> String {
    let text = format!("{v:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

pub struct Fields<V: 'static> {
    inputs: HashMap<String, Entity<InputState>>,
    choosers: HashMap<String, Entity<Chooser>>,
    key_pickers: HashMap<String, Entity<KeyPicker>>,
    subscriptions: Vec<Subscription>,
    _panel: std::marker::PhantomData<fn() -> V>,
}

impl<V: 'static> Default for Fields<V> {
    fn default() -> Self {
        Self { inputs: HashMap::new(), choosers: HashMap::new(), key_pickers: HashMap::new(), subscriptions: Vec::new(), _panel: std::marker::PhantomData }
    }
}

/// Bring `input` to showing `text`, unless it is being typed in.
fn sync_text<V: 'static>(input: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut Context<V>) {
    let focused = input.read(cx).focus_handle(cx).contains_focused(window, cx);
    if !focused && input.read(cx).value().as_ref() != text {
        input.update(cx, |i, cx| i.set_value(text.to_string(), window, cx));
    }
}

impl<V: 'static> Fields<V> {
    /// A searchable chooser over `items` (with `pinned` ones always on top),
    /// showing `selected` (an item id).
    /// `set` receives the id of the item picked.
    pub fn chooser(
        &mut self,
        key: String,
        items: Vec<ChoiceItem>,
        pinned: Vec<ChoiceItem>,
        selected: &str,
        set: impl Fn(&mut V, String, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<Chooser> {
        if !self.choosers.contains_key(&key) {
            let chooser = cx.new(|cx| Chooser::new(items.clone(), selected, window, cx).with_pinned(pinned, cx));
            self.subscriptions.push(cx.subscribe_in(&chooser, window, move |this: &mut V, _c, event: &ChosenEvent, _w, cx| {
                set(this, event.0.clone(), cx);
            }));
            self.choosers.insert(key.clone(), chooser);
        }
        let chooser = self.choosers[&key].clone();
        chooser.update(cx, |c, cx| c.sync(items, selected, cx));
        chooser
    }

    /// A key picker (search or listen), showing `selected` (a key id).
    pub fn key_picker(
        &mut self,
        key: String,
        selected: &str,
        set: impl Fn(&mut V, String, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<KeyPicker> {
        if !self.key_pickers.contains_key(&key) {
            let picker = cx.new(|cx| KeyPicker::new(selected, window, cx));
            self.subscriptions.push(cx.subscribe_in(&picker, window, move |this: &mut V, _p, event: &ChosenEvent, _w, cx| {
                set(this, event.0.clone(), cx);
            }));
            self.key_pickers.insert(key.clone(), picker);
        }
        let picker = self.key_pickers[&key].clone();
        picker.update(cx, |p, cx| p.sync(selected, cx));
        picker
    }

    /// A number field. `current` is what the model holds now (`None` to show nothing).
    /// `get` re-reads it when the field fires; `set` stores a new value.
    #[allow(clippy::too_many_arguments)]
    pub fn number(
        &mut self,
        key: String,
        spec: NumberSpec,
        current: Option<f32>,
        get: impl Fn(&V, &App) -> Option<f32> + 'static,
        set: impl Fn(&mut V, f32, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<InputState> {
        if !self.inputs.contains_key(&key) {
            let input = cx.new(|cx| InputState::new(window, cx));
            let (get, set) = (Rc::new(get), Rc::new(set));

            let (g, s) = (get.clone(), set.clone());
            self.subscriptions.push(cx.subscribe_in(&input, window, move |this: &mut V, input, event: &InputEvent, _w, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let text = input.read(cx).value().to_string();
                let Ok(value) = text.trim().parse::<f32>() else { return };
                if !value.is_finite() {
                    return;
                }
                // Echo of the model's own value being shown: not an edit.
                if g(this, cx).is_some_and(|c| format_number(c, spec.decimals) == text.trim()) {
                    return;
                }
                s(this, value.clamp(spec.min, spec.max), cx);
            }));

            self.subscriptions.push(cx.subscribe_in(&input, window, move |this: &mut V, _input, event: &NumberInputEvent, _w, cx| {
                let NumberInputEvent::Step { action, fine } = event;
                let step = if *fine { spec.step * 0.1 } else { spec.step };
                let base = get(this, cx).unwrap_or(0.0);
                let next = match action {
                    StepAction::Increment => base + step,
                    StepAction::Decrement => base - step,
                };
                set(this, next.clamp(spec.min, spec.max), cx);
            }));
            self.inputs.insert(key.clone(), input);
        }
        let input = self.inputs[&key].clone();
        sync_text(&input, &current.map(|v| format_number(v, spec.decimals)).unwrap_or_default(), window, cx);
        input
    }

    /// A text field. `get` re-reads the model's text when the field fires.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        key: String,
        multiline: bool,
        placeholder: &str,
        current: &str,
        get: impl Fn(&V, &App) -> Option<String> + 'static,
        set: impl Fn(&mut V, String, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<InputState> {
        if !self.inputs.contains_key(&key) {
            let placeholder = placeholder.to_string();
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx);
                let state = if multiline { state.multi_line().rows(3) } else { state };
                state.placeholder(placeholder)
            });
            self.subscriptions.push(cx.subscribe_in(&input, window, move |this: &mut V, input, event: &InputEvent, _w, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let text = input.read(cx).value().to_string();
                if get(this, cx).is_some_and(|c| c == text) {
                    return;
                }
                set(this, text, cx);
            }));
            self.inputs.insert(key.clone(), input);
        }
        let input = self.inputs[&key].clone();
        sync_text(&input, current, window, cx);
        input
    }

}

#[cfg(test)]
mod tests {
    use super::format_number;

    #[test]
    fn numbers_print_without_trailing_zeros() {
        assert_eq!(format_number(16.0, 2), "16");
        assert_eq!(format_number(1.5, 2), "1.5");
        assert_eq!(format_number(0.126, 2), "0.13");
        assert_eq!(format_number(-0.0, 1), "-0");
        assert_eq!(format_number(100.0, 0), "100");
    }
}
