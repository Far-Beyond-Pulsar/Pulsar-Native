//! The voxel plugin's inspector editors: the generator picker's entries and
//! the layer stack's edits.

use helio_pass_voxel_planet::terrain::{GeneratorInfo, generators};
use voxel_component::VoxelGeneratorRef;
use voxel_plugin::generator_editor::generator_items;

fn choice(id: &str, version: u32, name: &str) -> GeneratorInfo {
    GeneratorInfo {
        id: id.into(),
        version,
        name: name.into(),
        description: format!("{name} terrain"),
        settings_component: None,
    }
}

#[test]
fn the_generator_picker_lists_registered_generators_and_searches_them() {
    use ui::dropdown::DropdownItem;
    let titles = |items: &[voxel_plugin::generator_editor::GeneratorItem]| {
        items
            .iter()
            .map(|i| i.title().to_string())
            .collect::<Vec<_>>()
    };
    let items = generator_items(&VoxelGeneratorRef::default(), &generators());
    assert_eq!(titles(&items), ["Terrain layers"]);
    assert!(
        items[0].matches("LAYERS")
            && items[0].matches("helio.terrain")
            && items[0].matches("crater")
    );
    assert!(!items[0].matches("volcano"));

    // A generator from a plugin that is not loaded stays selected; versions
    // of one generator are told apart.
    let current = VoxelGeneratorRef::new("x.moon", 3);
    let items = generator_items(
        &current,
        &[choice("x.caves", 1, "Caves"), choice("x.caves", 2, "Caves")],
    );
    assert_eq!(
        titles(&items),
        ["x.moon v3 (not loaded)", "Caves (v1)", "Caves (v2)"]
    );
    assert_eq!(items[0].value(), &current);
}

/// A layer switched to another kind in the inspector starts from that kind's
/// defaults (the generator's table), keeping its toggle and mask.
#[test]
fn a_kind_edit_takes_the_kinds_defaults() {
    use voxel_component::{VoxelLayerKind, VoxelLayerMask, VoxelTerrainLayer};
    use voxel_plugin::stack_editor::set_layer;
    let craters = VoxelTerrainLayer::new(VoxelLayerKind::Craters);
    let mut layer = VoxelTerrainLayer {
        mask: VoxelLayerMask::Land,
        enabled: false,
        ..VoxelTerrainLayer::new(VoxelLayerKind::Hills)
    };
    // A field edit keeps the kind and just stores the value.
    let edited = VoxelTerrainLayer {
        height_m: 7.0,
        ..layer.clone()
    };
    assert!(!set_layer(&mut layer, edited));
    assert_eq!(layer.height_m, 7.0);
    // A kind edit brings the new kind's numbers, not the old ones.
    let edited = VoxelTerrainLayer {
        kind: VoxelLayerKind::Craters,
        ..layer.clone()
    };
    assert!(set_layer(&mut layer, edited));
    assert_eq!(
        layer,
        VoxelTerrainLayer {
            mask: VoxelLayerMask::Land,
            enabled: false,
            ..craters
        }
    );
}
