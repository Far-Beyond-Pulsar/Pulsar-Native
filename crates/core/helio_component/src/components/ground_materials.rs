//! Ground materials: the material ids of the ground something grows on
//! (foliage's `grows_on`), edited as a line of names or ids ("Grass, Dirt,
//! 22") and serialized as the ids.
//!
//! The names belong to whatever draws the ground: a plugin whose ground has
//! materials registers them ([`GroundMaterialNames`]); without one, a list
//! shows and reads ids only. The inspector edits a list as its line
//! ([`ui_types_common::TextEditedType`]).

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
    deserialize_json_with = deserialize_material_list_json
)]
#[allow(dead_code)]
type RegisteredGroundMaterialList = GroundMaterialList;

// The inspector edits a list as its line of names and ids.
inventory::submit! {
    ui_types_common::TextEditedType {
        type_id: std::any::TypeId::of::<GroundMaterialList>(),
        to_text: |value| value.downcast_ref::<GroundMaterialList>().map(GroundMaterialList::text),
        from_text: |text| {
            GroundMaterialList::parse(text)
                .map(|list| Box::new(list) as Box<dyn std::any::Any + Send>)
        },
    }
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
