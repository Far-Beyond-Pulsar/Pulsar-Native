//! A voxel world's features as the inspector edits them
//! (`VoxelTerrainComponent::features`): spawns of the engine's flora
//! (trees, bushes, shrubs, cacti, boulders, craters), each with its rules of
//! where it grows. Their serialized form is
//! `helio_pass_voxel_planet::feature_recipe::FeatureRecipe`'s, field for
//! field: a level saves the recipe, the world grows it with its seed and
//! voxel size ([`feature_recipe`]).
//!
//! The items are flat sets of scalar, enum and list fields (what the
//! reflected list editor shows); a growth's card shows the fields its form
//! reads.

use engine_class_derive::engine_class;
use gpui::{Entity, Subscription, Window, prelude::*, px};
use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;
use helio_pass_voxel_planet::terrain::material;
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyWriteBack};
use ui::Sizable as _;
use ui::input::{Input, InputEvent, InputState};

/// What a growth grows (`feature_recipe::Form`).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    pulsar_reflection::Reflectable,
)]
pub enum VoxelGrowthForm {
    /// A tree of its species: Material wood, Second material leaves.
    #[default]
    Tree,
    /// A bush: low clusters of Material leaves.
    Bush,
    /// A dry-land shrub: Material stems, Second material leaves.
    Shrub,
    /// A cactus of Material.
    Cactus,
    /// A boulder of Material (a third of them of Second material, unless
    /// 0), Min to Max size across.
    Boulder,
    /// An impact crater Min to Max size across, its rim of Material,
    /// Blocks lumps of Second material thrown out.
    Crater,
}

/// A tree's species (`flora::Species`).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    pulsar_reflection::Reflectable,
)]
pub enum VoxelTreeSpecies {
    #[default]
    Oak,
    DarkOak,
    Birch,
    Spruce,
    Pine,
    Jungle,
}

/// Terrain material ids (`material::GRASS`, ...), edited as a line of names
/// or ids ("Grass, Dirt, 22"). Serialized as the ids.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct VoxelMaterialList(pub Vec<u32>);

impl VoxelMaterialList {
    /// The list as the editor shows it: built-in materials by name, others
    /// by id.
    pub fn text(&self) -> String {
        self.0
            .iter()
            .map(|&id| match material::NAMES.get((id as usize).wrapping_sub(1)) {
                Some(name) if id > 0 => (*name).to_string(),
                _ => id.to_string(),
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
                    .or_else(|| material::from_name(word))
                    .ok_or_else(|| format!("unknown material {word:?}"))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

fn serialize_material_list_json(
    value: &VoxelMaterialList,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_material_list_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<VoxelMaterialList> {
    serde_json::from_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_reflection::pulsar_type(
    serialize_json_with = serialize_material_list_json,
    deserialize_json_with = deserialize_material_list_json,
    editor = material_list_editor
)]
#[allow(dead_code)]
type RegisteredVoxelMaterialList = VoxelMaterialList;

/// One growth of a spawn: `variants` stamps of a flora generator
/// (`feature_recipe::Growth`).
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[serde(default)]
pub struct VoxelFeatureGrowth {
    #[property]
    pub form: VoxelGrowthForm,
    #[property]
    pub species: VoxelTreeSpecies,
    /// Wood, leaves, body or rim (see the form).
    #[property(min = 0.0, max = 31.0, step = 1.0)]
    pub material: u32,
    /// Leaves, another rock or thrown rocks (see the form; 0: none).
    #[property(min = 0.0, max = 31.0, step = 1.0, label = "Second material")]
    pub second_material: u32,
    /// A boulder's size or a crater's diameter, a range.
    #[property(min = 0.01, max = 1000.0, step = 0.05, label = "Min size (m)")]
    pub min_size_m: f64,
    #[property(min = 0.01, max = 1000.0, step = 0.05, label = "Max size (m)")]
    pub max_size_m: f64,
    /// Lumps a crater throws out.
    #[property(min = 0.0, max = 64.0, step = 1.0)]
    pub blocks: u32,
    /// Salt of its stamps' seeds: growths of different seeds differ.
    #[property]
    pub seed: u64,
    /// Stamps grown.
    #[property(min = 1.0, max = 64.0, step = 1.0)]
    pub variants: u32,
}

impl Default for VoxelFeatureGrowth {
    /// `feature_recipe::Growth::default()`.
    fn default() -> Self {
        Self {
            form: VoxelGrowthForm::Tree,
            species: VoxelTreeSpecies::Oak,
            material: material::WOOD,
            second_material: material::LEAVES,
            min_size_m: 0.5,
            max_size_m: 1.3,
            blocks: 0,
            seed: 0,
            variants: 6,
        }
    }
}

impl PartialEq for VoxelFeatureGrowth {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(other).ok()
    }
}

impl VoxelFeatureGrowth {
    /// Whether its form reads the field `name`.
    pub fn reads(&self, name: &str) -> bool {
        use VoxelGrowthForm::*;
        match name {
            "species" => self.form == Tree,
            "second_material" => matches!(self.form, Tree | Shrub | Boulder | Crater),
            "min_size_m" | "max_size_m" => matches!(self.form, Boulder | Crater),
            "blocks" => self.form == Crater,
            _ => true,
        }
    }
}

impl super::reflected_list::ReflectedListItem for VoxelFeatureGrowth {
    const CLASS_NAME: &'static str = "VoxelFeatureGrowth";
    const NOUN: &'static str = "growth";

    fn title(&self, index: usize) -> String {
        let what = match self.form {
            VoxelGrowthForm::Tree => format!("{:?}", self.species),
            form => format!("{form:?}"),
        };
        format!("{}. {what} x{}", index + 1, self.variants)
    }

    fn shows(&self, prop: &pulsar_reflection::PropertyMetadata) -> bool {
        self.reads(prop.name)
    }
}

crate::register_reflected_list_editor!(VoxelFeatureGrowth);

fn serialize_growth_json(
    value: &VoxelFeatureGrowth,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_growth_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<VoxelFeatureGrowth> {
    serde_json::from_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_reflection::pulsar_type(
    serialize_json_with = serialize_growth_json,
    deserialize_json_with = deserialize_growth_json
)]
#[allow(dead_code)]
type RegisteredVoxelFeatureGrowth = VoxelFeatureGrowth;

/// One spawn: what grows and where (`feature_recipe::SpawnRecipe`).
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[serde(default)]
pub struct VoxelFeatureSpawn {
    #[property]
    pub name: String,
    /// Its stamps: every growth's variants.
    #[property]
    pub growths: Vec<VoxelFeatureGrowth>,
    /// A side of the square each candidate lies in. Spawns go from the
    /// widest spacing to the narrowest.
    #[property(min = 0.1, max = 1000.0, step = 0.5, label = "Spacing (m)")]
    pub spacing_m: f64,
    /// Share of candidates that grow (before the field).
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub density: f32,
    /// The noise field gating it: its scale (0: none) and the threshold it
    /// must pass (-1 to 1).
    #[property(min = 0.0, max = 100000.0, step = 10.0, label = "Field scale (m)")]
    pub field_scale_m: f64,
    #[property(min = -1.0, max = 1.0, step = 0.01)]
    pub field_threshold: f32,
    /// Ground surface materials it grows on (empty: any).
    #[property]
    pub surfaces: VoxelMaterialList,
    /// Ground heights over the datum it grows between.
    #[property(min = -1.0e9, max = 1.0e9, step = 10.0, label = "Min height (m)")]
    pub min_height_m: f64,
    #[property(min = -1.0e9, max = 1.0e9, step = 10.0, label = "Max height (m)")]
    pub max_height_m: f64,
    /// How far its heights wander, and over what distance (0: they do
    /// not): a tree line follows noise, not a contour.
    #[property(min = 0.0, max = 10000.0, step = 10.0, label = "Height wander (m)")]
    pub height_wander_m: f64,
    #[property(min = 0.0, max = 100000.0, step = 10.0, label = "Wander scale (m)")]
    pub height_wander_scale_m: f64,
    /// The climate it grows in: annual mean temperature and moisture (0
    /// dry to 1 wet) at its ground (the terrain's climate).
    #[property(min = -1000.0, max = 1000.0, step = 0.5, label = "Min temperature (C)")]
    pub min_temperature_c: f64,
    #[property(min = -1000.0, max = 1000.0, step = 0.5, label = "Max temperature (C)")]
    pub max_temperature_c: f64,
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub min_moisture: f64,
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub max_moisture: f64,
    /// Most rise between the ground at its anchor and its footprint's
    /// corners (0: any).
    #[property(min = 0.0, max = 1000.0, step = 0.1, label = "Max relief (m)")]
    pub max_relief_m: f64,
    /// How far its anchor lies under the ground's top.
    #[property(min = -100.0, max = 100.0, step = 0.1, label = "Sink (m)")]
    pub sink_m: f64,
    /// Its far form's material (0: none: not drawn past its near levels)
    /// and the canopy's height (0: its stamps' mean height).
    #[property(min = 0.0, max = 127.0, step = 1.0, label = "Far material")]
    pub far_material: u32,
    #[property(min = 0.0, max = 100.0, step = 0.5, label = "Far height (m)")]
    pub far_height_m: f64,
}

impl Default for VoxelFeatureSpawn {
    /// `feature_recipe::SpawnRecipe::default()`.
    fn default() -> Self {
        Self {
            name: String::new(),
            growths: Vec::new(),
            spacing_m: 8.0,
            density: 0.5,
            field_scale_m: 0.0,
            field_threshold: -1.0,
            surfaces: VoxelMaterialList::default(),
            min_height_m: -1e9,
            max_height_m: 1e9,
            height_wander_m: 0.0,
            height_wander_scale_m: 0.0,
            min_temperature_c: -1e3,
            max_temperature_c: 1e3,
            min_moisture: 0.0,
            max_moisture: 1.0,
            max_relief_m: 0.0,
            sink_m: 0.0,
            far_material: 0,
            far_height_m: 0.0,
        }
    }
}

impl PartialEq for VoxelFeatureSpawn {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(other).ok()
    }
}

impl super::reflected_list::ReflectedListItem for VoxelFeatureSpawn {
    const CLASS_NAME: &'static str = "VoxelFeatureSpawn";
    const NOUN: &'static str = "spawn";
    const MAX_ITEMS: usize = helio_pass_voxel_planet::features::MAX_SPAWNS;

    fn title(&self, index: usize) -> String {
        let name = if self.name.is_empty() { "spawn" } else { &self.name };
        format!("{}. {name} ({} m)", index + 1, self.spacing_m)
    }
}

crate::register_reflected_list_editor!(VoxelFeatureSpawn);

fn serialize_spawn_json(
    value: &VoxelFeatureSpawn,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_spawn_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<VoxelFeatureSpawn> {
    serde_json::from_value(value)
        .map_err(|e| pulsar_reflection::ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_reflection::pulsar_type(
    serialize_json_with = serialize_spawn_json,
    deserialize_json_with = deserialize_spawn_json
)]
#[allow(dead_code)]
type RegisteredVoxelFeatureSpawn = VoxelFeatureSpawn;

/// The recipe `spawns` describe (the same JSON).
pub fn feature_recipe(spawns: &[VoxelFeatureSpawn]) -> FeatureRecipe {
    serde_json::to_value(spawns)
        .and_then(serde_json::from_value)
        .expect("feature spawns are a feature recipe")
}

/// The spawns of `recipe`, as the inspector edits them.
pub fn feature_spawns(recipe: &FeatureRecipe) -> Vec<VoxelFeatureSpawn> {
    serde_json::to_value(recipe)
        .and_then(serde_json::from_value)
        .expect("a feature recipe is feature spawns")
}

/// Property editor for [`VoxelMaterialList`]: a line of material names and
/// ids. A line naming an unknown material is kept unapplied until it
/// parses.
pub struct VoxelMaterialListEditor {
    label: String,
    value: VoxelMaterialList,
    input: Entity<InputState>,
    write_back: PropertyWriteBack,
    _subs: Vec<Subscription>,
}

impl VoxelMaterialListEditor {
    fn new(args: &PropertyEditorArgs<'_>, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let value = args
            .current_value
            .downcast_ref::<VoxelMaterialList>()
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
                let Ok(parsed) = VoxelMaterialList::parse(&input.read(cx).text().to_string()) else {
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
    fn set_value(&mut self, value: &VoxelMaterialList, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.value == *value {
            return;
        }
        self.value = value.clone();
        self.input
            .update(cx, |state, cx| state.set_value(value.text(), window, cx));
    }
}

impl gpui::Render for VoxelMaterialListEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        pulsar_reflection::prims::editor_row(&self.label, Input::new(&self.input).xsmall().w(px(180.0)), cx)
    }
}

fn material_list_editor(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> BoundPropertyEditor {
    let entity = cx.new(|cx| VoxelMaterialListEditor::new(args, window, cx));
    BoundPropertyEditor::new(
        entity,
        |editor: &mut VoxelMaterialListEditor, value: &VoxelMaterialList, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_pass_voxel_planet::feature_recipe::{Growth, SpawnRecipe};

    /// The inspector's spawns are the engine's recipe, field for field:
    /// every preset round-trips, and the defaults agree.
    #[test]
    fn spawns_are_the_engines_recipe() {
        for name in FeatureRecipe::PRESETS {
            let recipe = FeatureRecipe::preset(name).unwrap();
            let spawns = feature_spawns(&recipe);
            assert_eq!(feature_recipe(&spawns), recipe, "{name}");
            assert_eq!(
                serde_json::to_value(&spawns).unwrap(),
                serde_json::to_value(&recipe).unwrap()
            );
        }
        assert_eq!(
            serde_json::to_value(VoxelFeatureSpawn::default()).unwrap(),
            serde_json::to_value(SpawnRecipe::default()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(VoxelFeatureGrowth::default()).unwrap(),
            serde_json::to_value(Growth::default()).unwrap()
        );
        let partial: Vec<VoxelFeatureSpawn> = serde_json::from_str(r#"[{"name":"rocks","growths":[{"form":"Boulder"}]}]"#).unwrap();
        assert_eq!(
            feature_recipe(&partial),
            serde_json::from_str(r#"[{"name":"rocks","growths":[{"form":"Boulder"}]}]"#).unwrap(),
            "missing fields take the same defaults"
        );
    }

    #[test]
    fn material_lists_read_names_and_ids() {
        let list = VoxelMaterialList::parse("Grass, dark_stone 22").unwrap();
        assert_eq!(list.0, [material::GRASS, material::DARK_STONE, 22]);
        assert_eq!(list.text(), "Grass, DarkStone, 22");
        assert_eq!(VoxelMaterialList::parse(&list.text()).unwrap(), list);
        assert!(VoxelMaterialList::parse("Grass, Lava").is_err());
        assert_eq!(VoxelMaterialList::parse(" ").unwrap().0, Vec::<u32>::new());
    }

    #[test]
    fn a_growths_card_shows_what_its_form_reads() {
        let tree = VoxelFeatureGrowth::default();
        assert!(tree.reads("species") && !tree.reads("blocks") && !tree.reads("min_size_m"));
        let crater = VoxelFeatureGrowth {
            form: VoxelGrowthForm::Crater,
            ..tree
        };
        assert!(!crater.reads("species") && crater.reads("blocks") && crater.reads("max_size_m"));
    }
}
