//! [f32; 4] primitive type implementation (Color)
use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): see bool.rs — `editor` is GPUI-typed and
// only present behind `prims-gpui`.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_color_json,
        deserialize_json_with = deserialize_color_json,
        editor = color_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_color_json,
        deserialize_json_with = deserialize_color_json
    )
)]
type RegisteredColor = [f32; 4];

fn serialize_color_json(value: &[f32; 4]) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!([value[0], value[1], value[2], value[3]]))
}

fn deserialize_color_json(value: serde_json::Value) -> crate::ReflectResult<[f32; 4]> {
    let arr = value
        .as_array()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "[f32; 4]",
            found: format!("{:?}", value),
        })?;

    if arr.len() != 4 {
        return Err(crate::ReflectError::TypeMismatch {
            expected: "[f32; 4]",
            found: format!("array of length {}", arr.len()),
        });
    }

    Ok([
        arr[0].as_f64().unwrap_or(0.0) as f32,
        arr[1].as_f64().unwrap_or(0.0) as f32,
        arr[2].as_f64().unwrap_or(0.0) as f32,
        arr[3].as_f64().unwrap_or(0.0) as f32,
    ])
}

// ── Editor ────────────────────────────────────────────────────────────────────

/// Convert linear RGBA to the HSLA the colour picker speaks.
#[cfg(feature = "prims-gpui")]
pub fn rgba_to_hsla([r, g, b, a]: [f32; 4]) -> gpui::Hsla {
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
    gpui::Hsla { h, s, l, a }
}

/// Convert the colour picker's HSLA back to the linear RGBA we store.
#[cfg(feature = "prims-gpui")]
pub fn hsla_to_rgba(gpui::Hsla { h, s, l, a }: gpui::Hsla) -> [f32; 4] {
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

/// Property editor for `[f32; 4]` — a colour picker.
///
/// Owns its `ColorPickerState` child entity and the subscription that turns
/// picker changes into write-backs.
#[cfg(feature = "prims-gpui")]
pub struct ColorEditor {
    label: String,
    picker: gpui::Entity<ui::color_picker::ColorPickerState>,
    value: [f32; 4],
    write_back: crate::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

#[cfg(feature = "prims-gpui")]
impl ColorEditor {
    fn new(
        args: &crate::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui::color_picker::{ColorPickerEvent, ColorPickerState};

        let value = args
            .current_value
            .downcast_ref::<[f32; 4]>()
            .copied()
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);

        let picker = cx.new(|cx| ColorPickerState::new(window, cx));
        picker.update(cx, |state, cx| {
            state.set_value(rgba_to_hsla(value), window, cx);
        });

        let subs = vec![cx.subscribe_in(
            &picker,
            window,
            |this: &mut Self, _picker, event: &ColorPickerEvent, window, cx| {
                let ColorPickerEvent::Change(Some(hsla)) = event else {
                    return;
                };
                let rgba = hsla_to_rgba(*hsla);
                if this.value == rgba {
                    return;
                }
                this.value = rgba;
                (this.write_back)(Box::new(rgba), window, cx);
            },
        )];

        Self {
            label: args.display_name.to_string(),
            picker,
            value,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Accept a colour that changed elsewhere.  The equality guard stops the
    /// framework's per-render push from echoing back through the subscription.
    fn set_value(
        &mut self,
        value: [f32; 4],
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.value == value {
            return;
        }
        self.value = value;
        self.picker.update(cx, |state, cx| {
            state.set_value(rgba_to_hsla(value), window, cx);
        });
    }
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for ColorEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use ui::color_picker::ColorPicker;

        crate::prims::editor_row(
            &self.label,
            ColorPicker::new(&self.picker).anchor(gpui::Corner::BottomRight),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn color_editor(
    args: &crate::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| ColorEditor::new(args, window, cx));
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut ColorEditor, value: &[f32; 4], window, cx| {
            editor.set_value(*value, window, cx)
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_vec4_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<[f32; 4]>().unwrap();
        assert_eq!(info.type_name, "[f32; 4]");
        assert_eq!(info.size, 16);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_vec4_serialization() {
        let value: [f32; 4] = [0.5, 0.6, 0.7, 1.0];
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 4);
    }

    #[test]
    fn test_vec4_deserialization() {
        let json = serde_json::json!([1.0, 0.5, 0.25, 0.8]);
        let mut deserializer = JsonDeserializer::new(json);
        let value = <[f32; 4]>::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, [1.0, 0.5, 0.25, 0.8]);
    }
}
