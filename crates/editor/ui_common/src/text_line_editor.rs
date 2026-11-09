//! The inspector's line editor: a property whose type registered
//! [`TextEditedType`] (it reads and shows as one line of text) is edited as
//! that line. A line that does not read is kept unapplied until it does.

use std::any::Any;
use std::sync::Arc;

use gpui::{prelude::*, px, Entity, Subscription, Window};
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyWriteBack};
use ui::input::{Input, InputEvent, InputState};
use ui::Sizable as _;
use ui_types_common::TextEditedType;

/// One property edited as its line of text.
pub struct TextLineEditor {
    label: String,
    kind: &'static TextEditedType,
    /// The line of the value last applied or accepted.
    text: String,
    input: Entity<InputState>,
    write_back: PropertyWriteBack,
    _subs: Vec<Subscription>,
}

impl TextLineEditor {
    fn new(
        args: &PropertyEditorArgs<'_>,
        kind: &'static TextEditedType,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let text = (kind.to_text)(args.current_value).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                    return;
                }
                let Ok(value) = (this.kind.from_text)(&input.read(cx).text().to_string()) else {
                    return;
                };
                let Some(text) = (this.kind.to_text)(value.as_ref() as &dyn Any) else {
                    return;
                };
                if text == this.text {
                    return;
                }
                this.text = text;
                (this.write_back)(value, window, cx);
            },
        )];
        Self {
            label: args.display_name.to_string(),
            kind,
            text,
            input,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Accept a value that changed elsewhere (undo, a preset).
    fn set_value(&mut self, value: &dyn Any, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(text) = (self.kind.to_text)(value) else {
            return;
        };
        if text == self.text {
            return;
        }
        self.text = text.clone();
        self.input
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }
}

impl gpui::Render for TextLineEditor {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        pulsar_reflection::prims::editor_row(
            &self.label,
            Input::new(&self.input).xsmall().w(px(180.0)),
            cx,
        )
    }
}

/// The editor factory for a property whose type registered
/// [`TextEditedType`] (the property editor registry's fallback).
pub fn text_line_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> BoundPropertyEditor {
    let Some(kind) = ui_types_common::text_edited(args.type_info.type_id) else {
        let entity = cx.new(|_| EmptyEditor);
        return BoundPropertyEditor::stateless(entity);
    };
    let entity = cx.new(|cx| TextLineEditor::new(args, kind, window, cx));
    let for_set = entity.clone();
    BoundPropertyEditor {
        view: entity.into(),
        set_value: Arc::new(move |value, window, cx| {
            for_set.update(cx, |editor, cx| editor.set_value(value, window, cx));
        }),
        rows: None,
    }
}

/// What [`text_line_editor`] shows for a type with no text editing (never
/// chosen by the registry).
struct EmptyEditor;

impl gpui::Render for EmptyEditor {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}
