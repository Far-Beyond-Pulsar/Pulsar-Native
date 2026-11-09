//! The level editor as the editor assembles it: the shell
//! (`ui_level_editor`) with every installed plugin's editor registrations linked,
//! each registering its tool modes, create entries, World Settings
//! sections, undo steps, save hooks and editor data through the shell's
//! extension points (`ui_level_editor::extend`).
//!
//! These tests live here because the plugins' editor crates depend on the
//! shell: linking them into the shell's own unit tests would link a second
//! copy of it.

use std::sync::Arc;

use engine_backend::scene::SceneWorldExt as _;
use installed_plugins::foliage_plugin::foliage_editor::level_editor::{
    domain, edit_foliage, foliage_sets,
};
use installed_plugins::voxel_plugin::voxel_component::{
    presets::VoxelWorldPreset, VoxelTerrainComponent, VoxelTerrainLayersComponent,
};
use installed_plugins::voxel_plugin::voxel_editor::level_editor::add_world::AddVoxelWorld;
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::extend;
use ui_level_editor::scene_edit::{components, level_io, objects, LevelEditorCameraState};
use ui_level_editor::LevelEditorState;

/// The Create menu (#1012): each kind adds one selected object with the
/// components its name promises, as one undo step.
#[test]
fn every_create_kind_adds_one_selected_object_with_its_components() {
    let expected: &[(&str, &[&str])] = &[
        ("empty", &[]),
        ("folder", &[]),
        ("camera", &[]),
        ("light_point", &["LightComponent"]),
        ("light_spot", &["LightComponent"]),
        ("light_directional", &["LightComponent"]),
        ("mesh_cube", &["StaticMeshComponent"]),
        ("mesh_sphere", &["StaticMeshComponent"]),
        ("mesh_cylinder", &["StaticMeshComponent"]),
        ("mesh_plane", &["StaticMeshComponent"]),
        ("sky", &["AtmosphereComponent"]),
        ("wind", &["WindComponent"]),
    ];
    // Menu entries that add no object: the sky's "Sky Panoramas" adds
    // assets to the project (a background download, not run here).
    let no_object = ["sky_panoramas"];
    let mut menu: Vec<&str> = expected.iter().map(|(kind, _)| *kind).collect();
    menu.insert(
        menu.iter().position(|kind| *kind == "sky").unwrap() + 1,
        "sky_panoramas",
    );
    assert_eq!(
        extend::create_entries()
            .into_iter()
            .filter(|entry| entry.menu_label.is_some())
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        menu,
        "the Create menu, in order"
    );
    assert!(no_object.iter().all(|kind| menu.contains(kind)));
    for &(kind, classes) in expected {
        let mut state = LevelEditorState::new();
        let result = extend::create(&mut state, kind);
        assert!(result.changed, "{kind}: {}", result.no_op_reason);
        let id = result.affected_ids[0].clone();
        assert_eq!(state.scene.selected_object(), Some(id.clone()), "{kind}");
        assert_eq!(
            components::get_component_class_names(&state.scene.world(), &id),
            classes,
            "{kind}"
        );

        assert!(state.scene.undo(), "{kind}");
        assert!(
            objects::get_all_objects(&state.scene.world()).is_empty(),
            "{kind}"
        );
    }
}

/// The plugins' tool modes follow Level Edit in the mode picker, and the
/// World Settings sections and environment mixer list the plugins'.
#[test]
fn the_plugins_register_their_modes_sections_and_mixer_entries() {
    let state = LevelEditorState::new();
    let modes: Vec<_> = state
        .editor
        .tool_mode_registry
        .modes()
        .iter()
        .map(|mode| mode.id().0)
        .collect();
    assert_eq!(
        modes,
        ["level_edit", "spline", "voxel_sculpt", "terrain", "foliage"]
    );
    let sections: Vec<_> = extend::level_component_sections()
        .into_iter()
        .map(|section| section.class_name)
        .collect();
    assert_eq!(sections, ["AtmosphereComponent", "WindComponent"]);
    let listed: Vec<_> = extend::environment_listings()
        .into_iter()
        .map(|listing| listing.class_name)
        .collect();
    assert_eq!(
        listed,
        [
            "LightComponent",
            "GlobalFogComponent",
            "LocalFogVolumeComponent",
            "PostProcessVolumeComponent",
            "ReflectionCaptureComponent",
            "WaterVolumeComponent",
        ]
    );
    let presets: Vec<_> = extend::environment_presets()
        .into_iter()
        .map(|preset| preset.id)
        .collect();
    assert_eq!(presets, ["clear_day", "golden_hour", "overcast"]);
    assert!(
        extend::viewport::debug_overlays().any(|overlay| overlay.id == "portal_links"),
        "the portals' overlay"
    );
}

/// A starter setup adds a sky and its rig (a sun and a moon) when missing,
/// as undoable steps.
#[test]
fn a_starter_setup_adds_a_sky_and_a_sun() {
    let mut state = LevelEditorState::new();
    let result = extend::apply_environment_preset(&mut state, "golden_hour");
    assert!(result.changed, "{}", result.no_op_reason);
    let world = state.scene.world();
    assert_eq!(
        installed_plugins::sky_plugin::sky_component::atmospheres(&world).len(),
        1
    );
    assert_eq!(
        installed_plugins::lights_plugin::lights_component::directional_lights(&world).len(),
        2,
        "the sky rig's sun and moon"
    );
    drop(world);
    let again = extend::apply_environment_preset(&mut state, "golden_hour");
    assert!(!again.changed, "already applied");
}

fn terrains(state: &LevelEditorState) -> Vec<VoxelTerrainComponent> {
    state
        .scene
        .world()
        .query::<&VoxelTerrainComponent>()
        .map(|(_, terrain)| terrain.clone())
        .collect()
}

/// Create Voxel Terrain: a ready-made world in one undoable step (the voxel
/// plugin's command, run by the shell's `execute_command`).
#[test]
fn a_voxel_world_is_one_undo_step_that_selects_its_terrain_and_moves_the_camera() {
    let mut state = LevelEditorState::new();
    // A level with a sky already: the world's sky takes its place.
    assert!(extend::create(&mut state, "sky").changed);
    let before = state.scene.world().children_of(None).len();

    let created = execute_command(
        &mut state,
        SceneCommand::Extension(Box::new(AddVoxelWorld {
            preset: VoxelWorldPreset::Earth,
        })),
    );
    assert!(created.changed, "{}", created.no_op_reason);
    let terrain = created.affected_ids[0].clone();
    assert_eq!(terrains(&state).len(), 1);
    assert!(terrains(&state)[0].editable, "sculpted at once");
    assert_eq!(state.scene.selected_object(), Some(terrain.clone()));
    let camera = state
        .editor
        .camera_request
        .clone()
        .expect("the camera over the ground");
    assert!(camera.position[1] > 6_371_000.0);
    {
        let world = state.scene.world();
        assert_eq!(
            installed_plugins::sky_plugin::sky_component::atmospheres(&world).len(),
            1
        );
        assert_eq!(
            installed_plugins::foliage_plugin::foliage_component::winds(&world).len(),
            1
        );
        assert_eq!(
            installed_plugins::lights_plugin::lights_component::directional_lights(&world).len(),
            1
        );
    }

    assert!(state.scene.undo());
    assert!(terrains(&state).is_empty(), "one undo removes the world");
    assert_eq!(state.scene.world().children_of(None).len(), before);
    assert_eq!(
        installed_plugins::sky_plugin::sky_component::atmospheres(&state.scene.world()).len(),
        1,
        "the level's sky is back"
    );
    assert!(state.scene.redo());
    assert_eq!(terrains(&state).len(), 1);
    assert_eq!(
        installed_plugins::sky_plugin::sky_component::atmospheres(&state.scene.world()).len(),
        1
    );
}

/// The foliage palette is the foliage plugin's editor data: saved with the
/// level under `foliage_sets`, read back on open, kept by world-only saves.
#[test]
fn foliage_palette_round_trips_with_selection_and_placement() {
    use foliage_sets::{FoliageSelection, FoliageSetLibrary};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foliage.level");
    let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
    edit_foliage(&mut state.write(), |foliage| {
        let library = &mut foliage.foliage_sets;
        let set = library.add_set();
        library.set_mut(set).unwrap().name = "Meadow".into();
        let oak = library
            .add_member(set, "assets/trees/oak.glb".into())
            .unwrap();
        let bush = library
            .add_member(set, "assets/plants/bush.fbx".into())
            .unwrap();
        library.member_mut(set, bush).unwrap().enabled = false;
        library
            .member_mut(set, oak)
            .unwrap()
            .placement
            .set_density(27.0);
        library
            .member_mut(set, oak)
            .unwrap()
            .placement
            .set_scale_min(0.7);
        library.selection = Some(FoliageSelection::Member(set, oak));
        let disabled = library.add_set();
        library.set_mut(disabled).unwrap().enabled = false;
        library.set_mut(disabled).unwrap().expanded = false;
        library.selection = Some(FoliageSelection::Member(set, oak));
    });
    assert!(state.read().scene.has_unsaved_changes);
    let expected = domain(&state.read()).foliage_sets.clone();
    extend::save_level_now(&state, &path).unwrap();
    assert!(!state.read().scene.has_unsaved_changes);

    let mut restored = LevelEditorState::new();
    let (editor, _) =
        level_io::load_from_file_with_editor_state(&mut restored.scene.world_mut(), &path).unwrap();
    extend::load_editor_data(&mut restored, Some(&editor));
    let library = domain(&restored).foliage_sets.clone();
    assert_eq!(library, expected);
    assert_eq!(library.paintable_members().count(), 1);
    let mut library = library;
    let added = library.add_set();
    assert!(!expected.sets.iter().any(|set| set.id == added));

    // World-only saves must preserve authoring state even with a fresh camera.
    level_io::save_to_file_with_editor_camera(
        &restored.scene.world(),
        &path,
        Some(LevelEditorCameraState {
            position: [1.0, 2.0, 3.0],
            yaw: 0.0,
            pitch: 0.0,
        }),
    )
    .unwrap();
    let file: ui_level_editor::scene_edit::LevelFile =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let kept: FoliageSetLibrary =
        serde_json::from_value(file.editor.unwrap().plugins["foliage_sets"].clone()).unwrap();
    assert_eq!(kept, expected);

    // Saving an empty palette explicitly removes the old one.
    edit_foliage(&mut state.write(), |foliage| {
        foliage.foliage_sets = Default::default()
    });
    assert!(state.read().scene.has_unsaved_changes);
    extend::save_level_now(&state, &path).unwrap();
    let (editor, _) =
        level_io::load_from_file_with_editor_state(&mut restored.scene.world_mut(), &path).unwrap();
    extend::load_editor_data(&mut restored, Some(&editor));
    assert!(domain(&restored).foliage_sets.sets.is_empty());
}

#[test]
fn old_levels_default_to_an_empty_foliage_palette() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.level");
    let mut state = LevelEditorState::new();
    level_io::save_to_file(&state.scene.world(), &path).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["editor"] =
        serde_json::json!({ "camera": { "position": [0.0, 0.0, 0.0], "yaw": 0.0, "pitch": 0.0 } });
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let (editor, _) =
        level_io::load_from_file_with_editor_state(&mut state.scene.world_mut(), &path).unwrap();
    assert!(editor.camera.is_some());
    edit_foliage(&mut state, |foliage| {
        foliage.foliage_sets.add_set();
    });
    extend::load_editor_data(&mut state, Some(&editor));
    assert!(domain(&state).foliage_sets.sets.is_empty());
}

/// A level save keeps the viewport renderer's chunks of the voxel
/// terrain's edits with the terrain's journal (the voxel plugin's save
/// hook, asked while the renderer keeps drawing), and loading the level
/// gets them back.
#[test]
fn a_level_save_keeps_the_renderers_voxel_chunks() {
    use engine_backend::scene::SceneWorldExt as _;
    use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
    use installed_plugins::voxel_plugin::voxel_component::data::{
        VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape,
    };
    use installed_plugins::voxel_plugin::voxel_component::world as voxel_world;
    use ui_level_editor::scene_edit::{ObjectType, SceneObjectData, Transform};
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .unwrap();

    // A flat terrain dug where the camera looks.
    let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
    let terrain = {
        let state = state.read();
        let mut world = state.scene.world_mut();
        let id = objects::add_object(
            &mut world,
            SceneObjectData {
                id: String::new(),
                name: "Terrain".into(),
                object_type: ObjectType::Empty,
                transform: Transform::default(),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            None,
        );
        components::add_component(
            &mut world,
            &id,
            "VoxelTerrainComponent".into(),
            serde_json::to_value(VoxelTerrainComponent::plane(512.0)).unwrap(),
        );
        components::add_component(
            &mut world,
            &id,
            "VoxelTerrainLayersComponent".into(),
            serde_json::to_value(VoxelTerrainLayersComponent::flat(2.0)).unwrap(),
        );
        let owner = world.entity_for(&id).unwrap();
        let terrain = engine_backend::scene::attachments::enabled_components_of::<
            VoxelTerrainComponent,
        >(&world, owner)[0]
            .0;
        let edits = (0..64)
            .map(|k| VoxelBrushEdit {
                center: [
                    f64::from(k % 8) * 3.0 - 10.0,
                    2.0,
                    f64::from(k / 8) * 3.0 - 30.0,
                ],
                radius: 2.5,
                shape: VoxelBrushShape::Sphere,
                op: VoxelBrushOp::Remove,
                material: 0,
                height: 0.0,
            })
            .collect();
        voxel_world::append_edits(&mut world, terrain, edits).unwrap();
        terrain
    };

    // The editor's viewport renderer, settled on the dig.
    let scene = state.read().scene.shared_scene();
    let mut renderer = HelioRenderer::new(scene.clone());
    renderer.set_editor_camera_state(EditorCameraState {
        position: [0.0, 14.0, 0.0],
        yaw: 0.0,
        pitch: -0.5,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 320,
            height: 180,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut frame = || {
        let rendered = renderer
            .render_frame(&device, &queue, &view, 320, 180, FORMAT)
            .is_some();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rendered
    };
    for _ in 0..4000 {
        if !frame() {
            break;
        }
    }

    // Saved on another thread while the viewport keeps drawing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dug.level");
    let saving = {
        let (state, path) = (state.clone(), path.clone());
        std::thread::spawn(move || extend::save_level_now(&state, &path))
    };
    while !saving.is_finished() {
        frame();
    }
    saving.join().unwrap().unwrap();
    let saved = scene
        .read()
        .world
        .get::<VoxelTerrainComponent>(terrain)
        .unwrap()
        .edits
        .clone();
    let kept = saved
        .kept_chunks()
        .expect("the save kept the renderer's chunks");
    eprintln!("SAVE_KEPT {} KiB of chunks", kept.bytes.len() / 1024);

    let mut loaded = engine_backend::scene::new_scene();
    engine_backend::scene::RuntimeLevel::load_into(&path, &mut loaded.world).unwrap();
    let journals: Vec<_> = loaded
        .world
        .query::<&VoxelTerrainComponent>()
        .into_iter()
        .map(|(_, terrain)| terrain.edits.clone())
        .collect();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0], saved);
    assert_eq!(journals[0].kept_chunks(), Some(kept));
}

/// The assistant's spline tools are the splines plugin's
/// (`ui_level_editor::extend::AiToolNamespace`): reached through the
/// level editor's tool entry point like the shell's own.
#[test]
fn the_assistants_spline_tools_edit_an_open_level() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let level = dir.path().join("splines.level");
    std::fs::write(&level, "{}").unwrap();
    let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
    state.write().scene.current_scene = Some(level.clone());
    ui_level_editor::ai_sessions::register_editor(&state);
    let call = |tool: &str, args: serde_json::Value| {
        ui_level_editor::ai_tools::execute_ai_tool(&level, tool, args)
            .unwrap_or_else(|e| panic!("{tool}: {e}"))
    };
    let spline = call(
        "level_editor_create_spline",
        json!({ "points": [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]] }),
    );
    let spline_id = spline["id"].as_str().unwrap().to_string();
    let edited = call(
        "level_editor_edit_spline",
        json!({ "id": spline_id, "append_points": [[0.0, 0.0, 10.0]], "closed": true }),
    );
    assert_eq!(edited["point_count"], 4);
    assert_eq!(call("level_editor_list_splines", json!({}))["count"], 1);
    assert!(state.write().scene.undo(), "the edit is undoable");
    ui_level_editor::ai_sessions::unregister_editor(&state);
}
