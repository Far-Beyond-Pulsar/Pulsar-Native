//! Foliage growing on the voxel planet through the whole default graph: the placement
//! pass asks the planet pass's ground (`chunks::ground`) and the blades it keeps stand on
//! the terrain the CPU planet has, on the ground their types allow, gone where it is
//! dug, still on a still camera.
use std::sync::Arc;

use glam::DVec3;
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_pass_foliage_place::host_rows::FoliageTypeComponent;
use helio_pass_foliage_place::{
    pack_kind_and_flags, unpack_blade, FoliageKind, FoliagePlacePass, GpuBladeInstance,
    GpuFoliageTile, GpuFoliageType, GpuTileFrame, TileState,
};
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_pass_voxel_planet::engine::{PlanetFrame, VOXEL_FRAME};
use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{Brush, BrushOp, BrushShape, Cell, Planet, PlanetRecipe, RayHit};
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

/// The engine's features over Helio's: the voxel terrain's and the
/// foliage that grows on it.
fn voxel_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_voxel_planet::VoxelFeature)
        .with(helio_pass_foliage_gbuffer::FoliageFeature)
}

const SIZE: [u32; 2] = [640, 360];

struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: helio::Renderer,
    scene: pulsar_scenedb::SceneDb,
    view: wgpu::TextureView,
}

/// A blade as placed: its type and its root in world (planet) coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Blade {
    kind: u8,
    root: DVec3,
}

impl Harness {
    fn new() -> Option<Self> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
            let adapter = instance.request_adapter(&Default::default()).await.ok()?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: required_wgpu_features(adapter.features()),
                    required_limits: required_wgpu_limits(adapter.limits()),
                    experimental_features: required_experimental_features(adapter.features()),
                    ..Default::default()
                })
                .await
                .ok()?;
            let (device, queue) = (Arc::new(device), Arc::new(queue));
            let mut scene = pulsar_scenedb::SceneDb::new();
            let mut store = SceneGpuStore::new(
                &EngineGpuContext::new(device.clone(), queue.clone()),
                SceneGpuConfig {
                    classes: Vec::new(),
                    tombstone_headroom: 0,
                    max_cells_metadata: 0,
                },
            );
            helio_components::AtmosphereComponent::register_gpu_columns_growable(
                &mut store, 4, &device,
            );
            helio_components::MeshComponent::register_gpu_columns_growable(&mut store, 16, &device);
            helio_components::MaterialComponent::register_gpu_columns_growable(
                &mut store, 16, &device,
            );
            helio_components::StaticObjectComponent::register_gpu_columns_growable(
                &mut store, 16, &device,
            );
            helio_components::LightComponent::register_gpu_columns_growable(
                &mut store, 16, &device,
            );
            scene
                .world
                .attach_gpu_mirror(GpuMirrorHandle::new(Arc::new(store), queue.clone()));
            // Grass on grass, and a type keyed to sand (never seen on grass).
            for (ground, density) in [(material::GRASS, 8.0), (material::SAND, 8.0)] {
                let mut t = GpuFoliageType {
                    density,
                    height_range: [0.1, 0.3],
                    width_range: [0.05, 0.05],
                    slope_range: [-1.0, 1.0],
                    lod_distances: [6.0, 0.0, 0.0, 0.0],
                    lean_range: [0.2, 0.8],
                    kind_and_flags: pack_kind_and_flags(FoliageKind::Block, 0),
                    ground_mask: 1 << ground,
                    ..GpuFoliageType::default()
                };
                t.set_blades_per_tuft([4, 8]);
                let e = scene.world.spawn();
                scene.world.insert(
                    e,
                    FoliageTypeComponent::from(helio_pass_foliage_place::FoliageTypeRow::from(t)),
                );
            }
            let mut config = RendererConfig::new(SIZE[0], SIZE[1], wgpu::TextureFormat::Rgba8Unorm);
            config
                .features
                .edit::<helio_pass_foliage_place::FoliageSettings>()
                .enabled = true;
            config
                .features
                .edit::<helio_pass_foliage_place::FoliageSettings>()
                .tufts_per_m2 = Some(8.0);
            config
                .features
                .edit::<helio_pass_foliage_place::FoliageSettings>()
                .blades_per_tuft = Some(6.0);
            config
                .features
                .edit::<helio_pass_foliage_place::FoliageSettings>()
                .range = Some(60.0);
            let config = config.with_feature(helio_pass_voxel_planet::VoxelSettings::default());
            let mirror = scene.world.gpu_mirror().cloned().unwrap();
            let renderer = RendererBuilder::new(config.clone(), mirror)
                .with_external_device()
                .with_graph(Preset::Deferred.with_features(&voxel_features()))
                .build(
                    device.clone(),
                    queue.clone(),
                    SIZE[0],
                    SIZE[1],
                    config.surface_format,
                );
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: SIZE[0],
                    height: SIZE[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: config.surface_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            Some(Self {
                device,
                queue,
                renderer,
                scene,
                view,
            })
        })
    }

    /// `frames` frames of `planet` seen from `eye` looking along `forward`.
    fn run(&mut self, planet: &Arc<Planet>, eye: DVec3, forward: DVec3, frames: usize) {
        let up = eye.normalize().as_vec3();
        let camera = Camera::perspective_look_to(
            eye,
            forward.as_vec3(),
            up,
            std::f32::consts::FRAC_PI_4,
            SIZE[0] as f32 / SIZE[1] as f32,
            0.05,
            40_000_000.0,
        );
        let frame = PlanetFrame::single(planet.clone(), up);
        self.renderer
            .set_scene_payload(&VOXEL_FRAME, Arc::new(frame));
        for _ in 0..frames {
            self.scene.world.flush_gpu_mirror(&self.queue);
            self.renderer.render(&camera, &self.view).unwrap();
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
        }
    }

    /// Until the planet pass is settled (at most `limit` frames), then `extra` more.
    fn settle(
        &mut self,
        planet: &Arc<Planet>,
        eye: DVec3,
        forward: DVec3,
        limit: usize,
        extra: usize,
    ) {
        for _ in 0..limit {
            self.run(planet, eye, forward, 1);
            if self.renderer.find_pass::<ChunkPass>().unwrap().settled() {
                break;
            }
        }
        self.run(planet, eye, forward, extra);
    }

    fn read(&self, buffer: &wgpu::Buffer) -> Vec<u8> {
        let size = buffer.size();
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let data = staging.slice(..).get_mapped_range().unwrap().to_vec();
        data
    }

    /// The blades every resident tile draws (its band at their distance from `eye`; far bands
    /// hold tufts under the near ones that never draw there), in world space; and the
    /// resident tiles.
    fn blades(&self, eye: DVec3) -> (Vec<Blade>, usize) {
        let origin = self.renderer.world_origin();
        let eye_in_frame = (eye - origin).as_vec3();
        let place = self.renderer.find_pass::<FoliagePlacePass>().unwrap();
        let bands = *place.bands();
        let tiles: Vec<GpuFoliageTile> =
            bytemuck::cast_slice(&self.read(&place.tile_table)).to_vec();
        let frames: Vec<GpuTileFrame> =
            bytemuck::cast_slice(&self.read(&place.tile_frames)).to_vec();
        let arena: Vec<GpuBladeInstance> =
            bytemuck::cast_slice(&self.read(&place.blade_arena)).to_vec();
        let mut out = Vec::new();
        let mut resident = 0;
        for (slot, (tile, frame)) in tiles.iter().zip(&frames).enumerate() {
            if tile.state() != Some(TileState::Resident) {
                continue;
            }
            resident += 1;
            let band = bands.bands[bands.band_of_slot(slot as u32).unwrap()];
            for blade in
                &arena[tile.blade_offset as usize..(tile.blade_offset + tile.blade_count) as usize]
            {
                let p = unpack_blade(blade);
                let root = frame.point(
                    p.tile_uv[0],
                    p.tile_uv[1],
                    tile.bounds_center + p.height_offset,
                );
                if helio_pass_foliage_place::tuft_scale(
                    &bands,
                    &band,
                    (root - eye_in_frame).length(),
                    p.rank,
                ) > 0.0
                {
                    out.push(Blade {
                        kind: p.type_id,
                        root: origin + root.as_dvec3(),
                    });
                }
            }
        }
        (out, resident)
    }
}

/// Grassland: a land column whose surroundings are grass.
fn grassland(planet: &Planet) -> DVec3 {
    let grid = *planet.grid();
    let n = f64::from(grid.cells());
    for face in 0..6u8 {
        for t in 0..400 {
            let (a, b) = (
                0.3 + 0.4 * f64::from(t % 20) / 20.0,
                0.3 + 0.4 * f64::from(t / 20) / 20.0,
            );
            let (i, j) = ((a * n) as i32, (b * n) as i32);
            let grass = |di: i32, dj: i32| {
                let top = planet.terrain_top(face, i + di, j + dj);
                top > 20
                    && planet.terrain_material(Cell::new(face, i + di, j + dj, top - 1))
                        == material::GRASS
            };
            if (-12..=12).all(|d| grass(d * 10, 0) && grass(0, d * 10) && grass(d * 7, d * 7)) {
                return grid.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5);
            }
        }
    }
    panic!("no grassland");
}

/// Metres a root may lie from a voxel step's edge and stand on either column: the
/// probe locates cells by its tile's linearised cube-sphere map, the check by f64 rays.
const EDGE: f64 = 5.0e-5;

/// The surface under `root` (down its radial up), if it is within `tolerance` and faces up.
fn on_surface(planet: &Planet, root: DVec3, tolerance: f64) -> Option<RayHit> {
    let up = root.normalize();
    let hit = planet.raycast(root + up * 0.55, -up, 2.0)?;
    let miss = (0.55 - hit.distance).abs();
    (miss <= tolerance && hit.normal.dot(up) >= 0.5).then_some(hit)
}

/// Blades within `near` of `eye` against the CPU planet: (blades checked, roots off the
/// surface by more than `tolerance` m, on ground other than grass, of the sand type).
/// A root within [`EDGE`] of a step's edge stands on whichever column it touches.
fn check(
    planet: &Planet,
    blades: &[Blade],
    eye: DVec3,
    near: f64,
    tolerance: f64,
) -> (usize, usize, usize, usize) {
    let (mut checked, mut off, mut wrong_ground, mut sand, mut edges) = (0, 0, 0, 0, 0);
    for b in blades {
        if (b.root - eye).length() > near {
            continue;
        }
        checked += 1;
        sand += usize::from(b.kind == 1);
        let up = b.root.normalize();
        let side = up.any_orthonormal_vector();
        let hit = on_surface(planet, b.root, tolerance).or_else(|| {
            let other = up.cross(side);
            let edge = [side, -side, other, -other]
                .into_iter()
                .find_map(|d| on_surface(planet, b.root + d * EDGE, tolerance));
            edges += usize::from(edge.is_some());
            edge
        });
        let Some(hit) = hit else {
            off += 1;
            if off < 6 {
                let found = planet
                    .raycast(b.root + up * 0.55, -up, 2.0)
                    .map(|h| 0.55 - h.distance);
                eprintln!("blade at {:?}: surface {found:?} m below", b.root);
            }
            continue;
        };
        wrong_ground += usize::from(planet.material(hit.cell) != material::GRASS);
    }
    eprintln!("{edges} roots on a step's edge");
    (checked, off, wrong_ground, sand)
}

#[test]
fn foliage_grows_on_the_voxel_ground() {
    let Some(mut h) = Harness::new() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: foliage on voxels");
        return;
    };
    let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
    let dir = grassland(&planet);
    let eye = planet.surface_point(dir, 1.7);
    let up = eye.normalize();
    let forward = (up.any_orthonormal_vector() - up * 0.6).normalize();
    let shared = Arc::new(planet.clone());
    h.settle(&shared, eye, forward, 600, 120);
    let (blades, resident) = h.blades(eye);
    // Within the level-0 range of the view (cells of a pixel or more) the ground is the
    // CPU's exactly; further, a coarser level's surface (a cell's height).
    let near = 0.7 * 0.1 / (2.0 * (std::f64::consts::FRAC_PI_8).tan() / f64::from(SIZE[1]));
    let (checked, off, wrong_ground, sand) = check(&planet, &blades, eye, near, 0.002);
    let empty_near = blades.is_empty();
    eprintln!("{} blades on {resident} tiles; within {near:.1} m: {checked} checked, {off} off the surface, {wrong_ground} not on grass, {sand} of the sand type", blades.len());
    assert!(
        !empty_near && checked > 2000,
        "{checked} blades near the eye"
    );
    assert_eq!(off, 0, "blades off the surface");
    assert_eq!(wrong_ground + sand, 0, "blades off their ground");
    // Every tile around the eye holds blades (grassland all round).
    let coverage = {
        let mut cells = std::collections::HashSet::new();
        for b in &blades {
            let d = b.root - eye;
            if d.length() < 12.0 {
                let side = up.any_orthonormal_vector();
                let other = up.cross(side);
                cells.insert((
                    (d.dot(side) / 2.0).floor() as i32,
                    (d.dot(other) / 2.0).floor() as i32,
                ));
            }
        }
        cells.len()
    };
    eprintln!("2 m squares with blades within 12 m: {coverage}");
    assert!(
        coverage > 100,
        "bare ground near the eye: {coverage} squares with blades"
    );
    // As dense near the eye as the types ask (8 tufts a square metre of 4 to 8 blades):
    // the ground answers every candidate.
    let within = |r: f64| {
        blades
            .iter()
            .filter(|b| (b.root - eye).length() < r)
            .count() as f64
    };
    let density = (within(10.0) - within(3.0)) / (std::f64::consts::PI * (100.0 - 9.0));
    eprintln!(
        "records a square metre 3 to 10 m out: {density:.1} (planned {:.1})",
        8.0 * 6.0
    );
    assert!(
        density > 0.7 * 8.0 * 6.0,
        "sparse near the eye: {density:.1} records a square metre"
    );

    // Still: the same blades frame after frame.
    h.run(&shared, eye, forward, 20);
    let (again, _) = h.blades(eye);
    assert_eq!(again, blades, "blades changed under a still camera");
    // Placed again against the same ground: the same tufts (in any order).
    {
        let link = h
            .renderer
            .find_pass::<ChunkPass>()
            .unwrap()
            .foliage_ground();
        link.push_changes([[eye - DVec3::splat(30.0), eye + DVec3::splat(30.0)]]);
    }
    h.run(&shared, eye, forward, 12);
    let sorted = |mut v: Vec<Blade>| {
        v.sort_by(|a, b| {
            a.root
                .to_array()
                .partial_cmp(&b.root.to_array())
                .unwrap()
                .then(a.kind.cmp(&b.kind))
        });
        v
    };
    let (replaced, _) = h.blades(eye);
    let (replaced, before) = (sorted(replaced), sorted(blades.clone()));
    let moved = replaced.iter().zip(&before).filter(|(a, b)| a != b).count();
    eprintln!(
        "placed again: {} blades against {}, {moved} differ",
        replaced.len(),
        before.len()
    );
    assert!(
        replaced == before,
        "placing the same ground again changed {moved} blades"
    );

    // A dig 3 m in front: the blades over it go within a few frames of its chunks.
    let ground =
        planet.surface_point(dir, 0.0) + (forward - up * forward.dot(up)).normalize() * 3.0;
    let radius = 1.2;
    planet
        .apply(Brush {
            center: ground.to_array(),
            radius,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        })
        .unwrap();
    let dug = Arc::new(planet.clone());
    let over = |blades: &[Blade]| {
        blades
            .iter()
            .filter(|b| {
                (b.root - ground).length() < radius - 0.15 && (b.root - ground).dot(up) > -0.3
            })
            .count()
    };
    assert!(
        over(&blades) > 20,
        "{} blades over the dig site before it",
        over(&blades)
    );
    // Frames until nothing stands over the hole, and until every blade near stands on
    // the dug ground again (the crater's rim re-placed).
    let (mut gone, mut settled) = (None, None);
    for frame in 1..=60 {
        h.run(&dug, eye, forward, 1);
        let (now, _) = h.blades(eye);
        if gone.is_none() && over(&now) == 0 {
            gone = Some(frame);
        }
        let (checked, off, wrong_ground, _) = check(&planet, &now, eye, near, 0.002);
        eprintln!("dig, frame {frame}: {} over the hole; {checked} checked, {off} off, {wrong_ground} off grass", over(&now));
        if gone.is_some() && off + wrong_ground == 0 {
            settled = Some(frame);
            break;
        }
    }
    eprintln!(
        "dig: blades over it gone after {gone:?} frames, all on the ground after {settled:?}"
    );
    assert!(
        gone.is_some_and(|f| f <= 8),
        "blades over the dig took {gone:?} frames to go"
    );
    assert!(
        settled.is_some_and(|f| f <= 8),
        "blades off the dug ground for {settled:?} frames"
    );
}
