//! A level save keeps the voxel renderer's chunks of the terrain's edits
//! with its journal (`VoxelChunkSaves`, `VoxelEditJournal::keep_chunks`):
//! the renderer packs its edited chunks and answers the waiting save; a
//! renderer given the saved journal restores them by copy instead of
//! generating them from the terrain and every edit.

use std::sync::Arc;
use std::time::Duration;

use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::{voxel_world, VoxelTerrainComponent, VoxelTerrainLayersComponent};
use helio_voxel_data::{
    VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape, VoxelEditJournal, VoxelKeptChunks,
};
use pulsar_scenedb::Entity;

const SIZE: [u32; 2] = [320, 180];
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

/// A scene with a flat 512 m terrain, dug where the camera looks when
/// `dig`; its terrain instance.
fn dug_scene(dig: bool) -> (engine_backend::scene::SharedScene, Entity) {
    let scene: engine_backend::scene::SharedScene =
        Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    let terrain = {
        let mut scene = scene.write();
        let world = &mut scene.world;
        let object = world.spawn();
        let terrain =
            pulsar_world_registry::attach_value(world, object, VoxelTerrainComponent::plane(512.0))
                .unwrap();
        pulsar_world_registry::attach_value(world, object, VoxelTerrainLayersComponent::flat(2.0))
            .unwrap();
        if dig {
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
            voxel_world::append_edits(world, terrain, edits).unwrap();
        }
        terrain
    };
    (scene, terrain)
}

struct Viewport {
    renderer: HelioRenderer,
    view: wgpu::TextureView,
}

impl Viewport {
    fn new(device: &wgpu::Device, scene: &engine_backend::scene::SharedScene) -> Self {
        let mut renderer = HelioRenderer::new(scene.clone());
        renderer.set_editor_camera_state(EditorCameraState {
            position: [0.0, 14.0, 0.0],
            yaw: 0.0,
            pitch: -0.5,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
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
        Self { renderer, view }
    }

    /// One editor frame: whether it rendered.
    fn frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let rendered = self
            .renderer
            .render_frame(device, queue, &self.view, SIZE[0], SIZE[1], FORMAT)
            .is_some();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rendered
    }

    /// Render until the viewport goes idle.
    fn settle(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for _ in 0..4000 {
            if !self.frame(device, queue) {
                return;
            }
        }
        panic!(
            "the viewport never went idle: {:?}",
            self.renderer.voxel_diagnostics()
        );
    }

    /// A word of the chunk pass's diagnostics (`name=word`).
    fn word(&self, name: &str) -> String {
        let key = format!(" {name}=");
        self.renderer
            .voxel_diagnostics()
            .iter()
            .find_map(|line| {
                let at = line.find(&key)? + key.len();
                line[at..].split(' ').next().map(str::to_string)
            })
            .unwrap_or_default()
    }
}

fn journal(scene: &engine_backend::scene::SharedScene, terrain: Entity) -> VoxelEditJournal {
    scene
        .read()
        .world
        .get::<VoxelTerrainComponent>(terrain)
        .unwrap()
        .edits
        .clone()
}

#[test]
fn a_saved_journal_keeps_the_edited_chunks_and_a_new_renderer_restores_them() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let (scene, terrain) = dug_scene(true);
    let mut viewport = Viewport::new(&device, &scene);
    viewport.settle(&device, &queue);
    assert_eq!(viewport.word("kept_loaded"), "None", "nothing to load yet");

    // A save asks off the render thread; the idle viewport renders again
    // until it has answered.
    let saves = viewport.renderer.editor_mailbox().voxel_chunk_saves();
    let save = std::thread::spawn(move || saves.kept_chunks(Duration::from_secs(120)));
    let mut frames = 0;
    while !save.is_finished() {
        frames += usize::from(viewport.frame(&device, &queue));
    }
    let saved = save.join().unwrap().expect("the renderer answers a save");
    let edits = journal(&scene, terrain);
    assert_eq!(saved.id.entity_bits, terrain.bits());
    assert_eq!(saved.edits, edits, "the chunks of the shown journal");
    assert!(
        saved.chunks.len() > 10,
        "{} chunks kept",
        saved.chunks.len()
    );
    let bytes = saved.chunks.to_bytes();
    eprintln!(
        "VOXEL_KEPT_SAVED {} chunks, {} KiB, after {frames} frames",
        saved.chunks.len(),
        bytes.len() / 1024
    );

    // Kept with the journal and saved with it; any edit drops them.
    let mut kept = edits.clone();
    assert!(kept.keep_chunks(VoxelKeptChunks::of(&saved.edits, bytes.into())));
    let mut grown = kept.clone();
    grown.push(*edits.get(edits.len() - 1).unwrap());
    assert!(grown.kept_chunks().is_none());
    let loaded: VoxelEditJournal =
        serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
    assert_eq!(loaded.kept_chunks(), kept.kept_chunks());

    // A level loaded with the journal: its renderer's world keeps them.
    let (scene, terrain) = dug_scene(false);
    scene
        .write()
        .world
        .get_mut::<VoxelTerrainComponent>(terrain)
        .unwrap()
        .edits = loaded;
    let mut viewport = Viewport::new(&device, &scene);
    viewport.settle(&device, &queue);
    assert_eq!(
        viewport.word("kept_loaded"),
        format!("Some(Ok({}))", saved.chunks.len())
    );
}
