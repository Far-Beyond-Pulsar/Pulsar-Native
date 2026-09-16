//! [f32; 3] primitive type implementation (Vec3)

use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): see bool.rs — `editor` is GPUI-typed and
// only present behind `prims-gpui`.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_vec3_json,
        deserialize_json_with = deserialize_vec3_json,
        editor = vec3_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_vec3_json,
        deserialize_json_with = deserialize_vec3_json
    )
)]
type RegisteredVec3 = [f32; 3];

fn serialize_vec3_json(value: &[f32; 3]) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!([value[0], value[1], value[2]]))
}

fn deserialize_vec3_json(value: serde_json::Value) -> crate::ReflectResult<[f32; 3]> {
    let arr = value
        .as_array()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "[f32; 3]",
            found: format!("{:?}", value),
        })?;

    if arr.len() != 3 {
        return Err(crate::ReflectError::TypeMismatch {
            expected: "[f32; 3]",
            found: format!("array of length {}", arr.len()),
        });
    }

    Ok([
        arr[0].as_f64().unwrap_or(0.0) as f32,
        arr[1].as_f64().unwrap_or(0.0) as f32,
        arr[2].as_f64().unwrap_or(0.0) as f32,
    ])
}

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `[f32; 3]` — read-only display.
///
/// No child entities and no write-back: position/rotation/scale are edited
/// through the viewport gizmo, not this row.
#[cfg(feature = "prims-gpui")]
pub struct Vec3Editor {
    label: String,
    value: [f32; 3],
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for Vec3Editor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use ui::ActiveTheme;

        let [x, y, z] = self.value;
        crate::prims::editor_row(
            &self.label,
            gpui::div()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(format!("{x:.3}, {y:.3}, {z:.3}")),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn vec3_editor(
    args: &crate::PropertyEditorArgs<'_>,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let label = args.display_name.to_string();
    let value = args
        .current_value
        .downcast_ref::<[f32; 3]>()
        .copied()
        .unwrap_or([0.0; 3]);

    let entity = cx.new(|_| Vec3Editor { label, value });
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut Vec3Editor, value: &[f32; 3], _window, cx| {
            if editor.value != *value {
                editor.value = *value;
                cx.notify();
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_vec3_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<[f32; 3]>().unwrap();
        assert_eq!(info.type_name, "[f32; 3]");
        assert_eq!(info.size, 12);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_vec3_serialization() {
        let value: [f32; 3] = [1.0, 2.0, 3.0];
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_f64().unwrap(), 1.0);
        assert_eq!(arr[1].as_f64().unwrap(), 2.0);
        assert_eq!(arr[2].as_f64().unwrap(), 3.0);
    }

    #[test]
    fn test_vec3_deserialization() {
        let json = serde_json::json!([4.0, 5.0, 6.0]);
        let mut deserializer = JsonDeserializer::new(json);
        let value = <[f32; 3]>::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_vec3_clone_any() {
        let value: [f32; 3] = [7.0, 8.0, 9.0];
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<[f32; 3]>().unwrap(), [7.0, 8.0, 9.0]);
    }
}
