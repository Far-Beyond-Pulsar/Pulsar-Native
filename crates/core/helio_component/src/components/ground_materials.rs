//! Ground materials: the material ids of the ground something grows on
//! (foliage's `grows_on`), edited as a line of names or ids ("Grass, Dirt,
//! 22") and serialized as the ids.
//!
//! The names belong to whatever draws the ground: a plugin whose ground has
//! materials registers them ([`GroundMaterialNames`]); without one, a list
//! shows and reads ids only.

use gpui::{Entity, Subscription, Window, prelude::*, px};
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyWriteBack};
use ui::Sizable as _;
use ui::input::{Input, InputEvent, InputState};

/// Names of ground materials, registered by the plugin whose ground has
/// them (`inventory::submit!`).
pub struct GroundMaterialNames {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// The name of material `id`, if this registration names it.
    pub name: fn(u32) -> Option<&'static str>,
    /// The material a word names, if this registration knows it.
    pub id: fn(&str) -> Option<u32>,
}

impl pulsar_plugins::Contribution for GroundMaterialNames {
    const CATEGORY: &'static str = "scene.ground_material_names";

    fn origin(&self) -> &'static str {
        self.origin
    }

    /// One naming per plugin (a second is a catalog problem).
    fn id(&self) -> String {
        "ground_material_names".into()
    }
}

pulsar_plugins::registration!(GroundMaterialNames);

/// The registered name of ground material `id`.
pub fn ground_material_name(id: u32) -> Option<&'static str> {
    pulsar_plugins::registered::<GroundMaterialNames>().find_map(|names| (names.name)(id))
}

/// The ground material a registered name stands for.
pub fn ground_material_id(word: &str) -> Option<u32> {
    pulsar_plugins::registered::<GroundMaterialNames>().find_map(|names| (names.id)(word))
}

/// Ground material ids, edited as a line of names or ids. Serialized as the
/// ids.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct GroundMaterialList(pub Vec<u32>);

impl GroundMaterialList {
    /// The list as the editor shows it: registered materials by name,
    /// others by id.
    pub fn text(&self) -> String {
        self.0
            .iter()
            .map(|&id| match ground_material_name(id) {
                Some(name) => name.to_string(),
                None => id.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A line of names and ids, or the first word that is neither.
    pub fn parse(text: &str) -> Result<Self, String> {
        text.split([',', ' ', ';'])
            .filter(|word| !word.is_empty())
            .map(|word| {
                word.parse::<u32>()
                    .ok()
                    .or_else(|| ground_material_id(word))
                    .ok_or_else(|| format!("unknown material {word:?}"))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

fn serialize_material_list_json(
    value: &GroundMaterialList,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_material_list_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<GroundMaterialList> {
    serde_json::from_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_reflection::pulsar_type(
    serialize_json_with = serialize_material_list_json,
    deserialize_json_with = deserialize_material_list_json,
    editor = material_list_editor
)]
#[allow(dead_code)]
type RegisteredGroundMaterialList = GroundMaterialList;

/// Property editor for [`GroundMaterialList`]: a line of material names and
/// ids. A line naming an unknown material is kept unapplied until it
/// parses.
pub struct GroundMaterialListEditor {
    label: String,
    value: GroundMaterialList,
    input: Entity<InputState>,
    write_back: PropertyWriteBack,
    _subs: Vec<Subscription>,
}

impl GroundMaterialListEditor {
    fn new(
        args: &PropertyEditorArgs<'_>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let value = args
            .current_value
            .downcast_ref::<GroundMaterialList>()
            .cloned()
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| state.set_value(value.text(), window, cx));
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                    return;
                }
                let Ok(parsed) = GroundMaterialList::parse(&input.read(cx).text().to_string())
                else {
                    return;
                };
                if parsed == this.value {
                    return;
                }
                this.value = parsed.clone();
                (this.write_back)(Box::new(parsed), window, cx);
            },
        )];
        Self {
            label: args.display_name.to_string(),
            value,
            input,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Accept a value that changed elsewhere (undo, a preset).
    fn set_value(
        &mut self,
        value: &GroundMaterialList,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.value == *value {
            return;
        }
        self.value = value.clone();
        self.input
            .update(cx, |state, cx| state.set_value(value.text(), window, cx));
    }
}

impl gpui::Render for GroundMaterialListEditor {
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

fn material_list_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| GroundMaterialListEditor::new(args, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut GroundMaterialListEditor, value: &GroundMaterialList, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a registered name a list is ids; it reads and shows them,
    /// and serializes as the ids.
    #[test]
    fn material_lists_read_ids() {
        let list = GroundMaterialList::parse("3, 22 7").unwrap();
        assert_eq!(list.0, [3, 22, 7]);
        assert_eq!(list.text(), "3, 22, 7");
        assert_eq!(GroundMaterialList::parse(&list.text()).unwrap(), list);
        assert!(GroundMaterialList::parse("3, Lava").is_err());
        assert_eq!(GroundMaterialList::parse(" ").unwrap().0, Vec::<u32>::new());
        assert_eq!(serde_json::to_string(&list).unwrap(), "[3,22,7]");
    }
}
