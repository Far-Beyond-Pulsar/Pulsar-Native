//! f64 primitive type implementation

use crate::pulsar_type;

#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_f64_json,
        deserialize_json_with = deserialize_f64_json,
        editor = f64_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_f64_json,
        deserialize_json_with = deserialize_f64_json
    )
)]
type RegisteredF64 = f64;

fn serialize_f64_json(value: &f64) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_f64_json(value: serde_json::Value) -> crate::ReflectResult<f64> {
    value
        .as_f64()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "f64",
            found: format!("{:?}", value),
        })
}

// ── Editor ────────────────────────────────────────────────────────────────────

/// Step applied per increment/decrement click on the number input.
#[cfg(feature = "prims-gpui")]
const STEP: f64 = 1.0;

/// Property editor for `f64` — a stepping number input.
#[cfg(feature = "prims-gpui")]
pub struct F64Editor {
    label: String,
    input: gpui::Entity<ui::input::InputState>,
    value: f64,
    write_back: crate::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

#[cfg(feature = "prims-gpui")]
impl F64Editor {
    fn new(
        args: &crate::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui::input::{InputEvent, InputState, NumberInputEvent, StepAction};

        let value = args.current_value.downcast_ref::<f64>().copied().unwrap_or(0.0);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(format_f64(value), window, cx);
        });

        let subs = vec![
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, input, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                        return;
                    }
                    let text = input.read(cx).text().to_string();
                    let Ok(parsed) = text.trim().parse::<f64>() else {
                        return;
                    };
                    this.commit(parsed, window, cx);
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step { action, .. } = event;
                    let stepped = match action {
                        StepAction::Increment => this.value + STEP,
                        StepAction::Decrement => this.value - STEP,
                    };
                    this.value = stepped;
                    this.input.update(cx, |state, cx| {
                        state.set_value(format_f64(stepped), window, cx);
                    });
                    (this.write_back)(Box::new(stepped), window, cx);
                },
            ),
        ];

        Self {
            label: args.display_name.to_string(),
            input,
            value,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    fn commit(&mut self, value: f64, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        (self.write_back)(Box::new(value), window, cx);
    }

    fn set_value(&mut self, value: f64, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        self.input.update(cx, |state, cx| {
            state.set_value(format_f64(value), window, cx);
        });
    }
}

#[cfg(feature = "prims-gpui")]
fn format_f64(value: f64) -> String {
    format!("{:.3}", value)
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for F64Editor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::Styled as _;
        use ui::{Sizable, input::NumberInput};

        crate::prims::editor_row(
            &self.label,
            NumberInput::new(&self.input).xsmall().w(gpui::px(92.0)),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn f64_editor(
    args: &crate::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| F64Editor::new(args, window, cx));
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut F64Editor, value: &f64, window, cx| editor.set_value(*value, window, cx),
    )
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_f64_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<f64>().unwrap();
        assert_eq!(info.type_name, "f64");
        assert_eq!(info.size, 8);
        assert_eq!(info.align, 8);
    }

    #[test]
    fn test_f64_serialization() {
        let value: f64 = std::f64::consts::PI;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_f64().unwrap(), value);
    }

    #[test]
    fn test_f64_deserialization() {
        let expected = std::f64::consts::E;
        let json = serde_json::json!(expected);
        let mut deserializer = JsonDeserializer::new(json);
        let value = f64::deserialize(&mut deserializer).unwrap();
        assert!((value - expected).abs() < 0.00001);
    }

    #[test]
    fn test_f64_clone_any() {
        let value: f64 = 42.0;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<f64>().unwrap(), 42.0);
    }
}
