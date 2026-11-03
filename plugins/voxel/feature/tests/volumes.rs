//! Volumes (docs/chunk-octree.md#volumes): worlds placed by f64 transforms
//! in one chunk world, against the CPU's casts in each world's own frame
//! (`Volumes`): a turned Earth far from the origin draws as in its own
//! frame, a moon and a ship-sized block draw beside a planet with the
//! nearest hit winning, a moving and spinning volume draws where it is each
//! frame, picks name the volume they hit, a turned volume is lit as in its
//! own frame, and volumes light each other: their sun shadows and the GI
//! probes' rays cross from one into another as CPU rays over every volume
//! do.
mod common;
use common::{gpu, Gpu};
use glam::{DQuat, DVec3, Mat4, Vec3};
use helio_pass_voxel_planet::chunks::{Capacity, ChunkWorld, ResidencyConfig, VolumeId};
use helio_pass_voxel_planet::engine::{PickHit, PickResult, VolumeAnswer, VolumeAnswers};
use helio_pass_voxel_planet::grid::Shape;
use helio_pass_voxel_planet::layers::TerrainLayers;
use helio_pass_voxel_planet::planet::Matter;
use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{
    Brush, BrushOp, BrushShape, Cell, Planet, PlanetRecipe, VolumeTransform, Volumes, VoxelVolume,
};
use std::sync::Arc;

const FOV_Y: f32 = std::f32::consts::FRAC_PI_4;

/// As `tests/chunks.rs`'s: parallel tests fit a 12 GB GPU.
const CAPACITY: Capacity = Capacity {
    chunks: 16384,
    detail_pages: 16384,
    footprints: 1024,
    bricks: 65536,
    kept_bytes: 256 << 20,
    counters: false,
};

/// A camera in the world (eye in world coordinates, its axes the world's).
#[derive(Clone, Copy)]
struct Camera {
    eye: DVec3,
    forward: Vec3,
    up: Vec3,
    size: [u32; 2],
}

impl Camera {
    /// A camera at `eye` looking along `forward` in volume frame `t`
    /// (`up` there), placed in the world.
    fn placed(t: &VolumeTransform, eye: DVec3, forward: DVec3, up: DVec3, size: [u32; 2]) -> Self {
        Self {
            eye: t.to_world(eye),
            forward: t.dir_to_world(forward).normalize().as_vec3(),
            up: t.dir_to_world(up).normalize().as_vec3(),
            size,
        }
    }

    fn view_proj(&self) -> Mat4 {
        let up = if self.forward.dot(self.up).abs() > 0.99 {
            self.up.any_orthonormal_vector()
        } else {
            self.up
        };
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, self.forward, up);
        let proj =
            helio_core::depth::perspective(FOV_Y, self.size[0] as f32 / self.size[1] as f32, 0.05);
        proj * view
    }

    /// The ray through pixel (x, y)'s centre (world).
    fn pixel_dir(&self, view_proj: Mat4, x: u32, y: u32) -> DVec3 {
        let ndc = glam::Vec4::new(
            (x as f32 + 0.5) / self.size[0] as f32 * 2.0 - 1.0,
            1.0 - (y as f32 + 0.5) / self.size[1] as f32 * 2.0,
            0.5,
            1.0,
        );
        let w = view_proj.inverse() * ndc;
        (w.truncate() / w.w).as_dvec3().normalize()
    }

    fn pixel_angle(&self) -> f64 {
        2.0 * f64::from((FOV_Y * 0.5).tan()) / f64::from(self.size[1])
    }

    fn config(&self) -> ResidencyConfig {
        let mut config = ResidencyConfig {
            footprints_per_frame: 512,
            generate_ms: None,
            ..Default::default()
        };
        config.plan.pixel_angle = self.pixel_angle();
        config
    }

    fn cone(&self) -> helio_pass_voxel_planet::chunks::residency::View {
        let tan_y = f64::from((FOV_Y * 0.5).tan());
        let tan_x = tan_y * f64::from(self.size[0]) / f64::from(self.size[1]);
        helio_pass_voxel_planet::chunks::residency::View {
            forward: self.forward.as_dvec3(),
            cos_half: (tan_x * tan_x + tan_y * tan_y).sqrt().atan().cos(),
        }
    }
}

/// A chunk world of several volumes and its frame counter.
/// A tool's picks waiting to be copied (request id, view point 0..1 from
/// the top left) and the inbox they are answered into.
#[derive(Default)]
struct Picks {
    waiting: Vec<(u64, [f32; 2])>,
    answers: Arc<VolumeAnswers>,
}

impl Picks {
    /// The answers so far.
    fn results(&self) -> Vec<PickResult> {
        self.answers
            .take()
            .into_iter()
            .filter_map(|a| match a {
                VolumeAnswer::Pick(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}

struct Scene {
    world: ChunkWorld,
    ids: Vec<VolumeId>,
    frame: u64,
    /// The direction to the sun (world).
    sun: Vec3,
    last: Option<Mat4>,
    /// GI rays to trace each frame (`gi_rays`), from a clipmap around the
    /// eye.
    gi: Option<[wgpu::Buffer; 4]>,
}

impl Scene {
    fn new(gpu: &Gpu, volumes: &Volumes) -> Self {
        Self::with_capacity(gpu, volumes, CAPACITY)
    }

    fn with_capacity(gpu: &Gpu, volumes: &Volumes, capacity: Capacity) -> Self {
        let mut world = ChunkWorld::with_capacity(&gpu.device, &gpu.queue, capacity);
        // Each volume added, timed (its form's pipelines; the second the
        // passes lighting across volumes).
        let ids = volumes
            .volumes
            .iter()
            .map(|v| {
                let clock = std::time::Instant::now();
                let id = world.add_volume(v.id, &v.planet, v.transform);
                eprintln!(
                    "volume {} added in {:.1} s",
                    v.id,
                    clock.elapsed().as_secs_f64()
                );
                id
            })
            .collect();
        Self {
            world,
            ids,
            frame: 0,
            sun: Vec3::new(0.3, 0.8, 0.4).normalize(),
            last: None,
            gi: None,
        }
    }

    /// The volumes' transforms now.
    fn place(&mut self, volumes: &Volumes) {
        for (id, v) in self.ids.iter().zip(&volumes.volumes) {
            self.world.set_transform(*id, v.transform);
        }
    }

    /// One frame: residency, the draw, `picks` copied; its residency.
    fn frame(
        &mut self,
        gpu: &Gpu,
        camera: &Camera,
        picks: Option<&mut Picks>,
    ) -> helio_pass_voxel_planet::chunks::UpdateStats {
        let view_proj = camera.view_proj();
        let prev = self.last.replace(view_proj).unwrap_or(view_proj);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut recorder = helio_core::CommandRecorder::from_encoder(&mut encoder);
        let stats = self.world.update(
            &mut recorder,
            camera.eye,
            DVec3::ZERO,
            Some(camera.cone()),
            self.frame,
            &camera.config(),
        );
        if self.gi.is_some() {
            self.world.follow_gi(
                helio_gi_api::Clipmap::around(camera.eye, helio_gi_api::DEFAULT_FINEST_SPACING),
                camera.eye,
                camera.eye,
            );
        }
        let gi = self.gi.as_ref().map(|[frame, rays, hits, sun]| {
            helio_pass_voxel_planet::chunks::GiRays {
                frame,
                rays,
                hits,
                sun,
            }
        });
        self.world.draw(
            &mut recorder,
            self.frame,
            camera.eye,
            view_proj,
            prev,
            [glam::Vec2::ZERO; 2],
            0.05,
            self.sun,
            camera.size,
            None,
            None,
            gi.as_ref(),
        );
        if let Some(picks) = picks {
            let taken = self
                .world
                .copy_picks(&mut recorder, &picks.waiting, &picks.answers);
            picks.waiting.drain(..taken);
        }
        drop(recorder);
        gpu.queue.submit([encoder.finish()]);
        self.world.submitted();
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        self.frame += 1;
        stats
    }

    /// Frames until residency settles; every pixel's hit then.
    fn settle(&mut self, gpu: &Gpu, camera: &Camera) -> Vec<Option<PickHit>> {
        let started = self.frame;
        loop {
            let stats = self.frame(gpu, camera, None);
            if self.world.settled() {
                break;
            }
            if (self.frame - started) % 500 == 0 {
                eprintln!(
                    "frame {}: volumes (id, tag, resident, drawn, waiting) {:?}; {stats:?}",
                    self.frame - started,
                    self.world.volume_stats()
                );
            }
            assert!(
                self.frame - started < 4000,
                "residency did not settle: {stats:?}"
            );
        }
        eprintln!(
            "settled in {} frames: {} chunks drawn",
            self.frame - started,
            self.world.chunk_count()
        );
        self.world.read_hits()
    }
}

/// How drawn hits compare with the CPU's nearest over the volumes: pixels
/// compared, mismatched, and compared by volume (`Volumes::volumes` order).
#[derive(Debug, Default)]
struct Agreement {
    compared: usize,
    mismatched: usize,
    by_volume: Vec<usize>,
    /// Pixels whose drawn hit is in another volume than the CPU's (or none
    /// where it has one, or the reverse), of those checked coarsely.
    other_volume: usize,
    coarse: usize,
}

/// Drawn hits against CPU casts (`Volumes::raycast`, each in its world's
/// frame) at every second pixel. A hit within the level-0 range of its
/// volume (cells of a pixel or more) must be drawn at level 0 in the same
/// volume at the same cell (or its neighbour across an edge the ray runs
/// along) and distance; one past it but within the casts' reach (the
/// farthest volume's level-0 range), in the same volume within a few of the
/// drawn level's cells. Past the casts' reach a drawn hit must lie in the
/// bounds of the volume it names, along the ray.
fn compare(hits: &[Option<PickHit>], volumes: &Volumes, camera: &Camera) -> Agreement {
    let view_proj = camera.view_proj();
    let mut out = Agreement {
        by_volume: vec![0; volumes.volumes.len()],
        ..Default::default()
    };
    let near_of = |v: &VoxelVolume| 0.7 * v.planet.grid().voxel_size() / camera.pixel_angle();
    let reach = volumes.volumes.iter().map(near_of).fold(0.0, f64::max);
    for y in (0..camera.size[1]).step_by(2) {
        for x in (0..camera.size[0]).step_by(2) {
            let dir = camera.pixel_dir(view_proj, x, y);
            let cpu = volumes.raycast(camera.eye, dir, reach, Matter::All);
            let drawn = hits[(y * camera.size[0] + x) as usize];
            let Some(cpu) = cpu else {
                // Past the casts' reach: the named volume's bounds hold the
                // drawn hit.
                out.coarse += 1;
                let plausible = drawn.is_none_or(|h| {
                    h.distance >= reach * 0.99
                        && volumes
                            .volumes
                            .iter()
                            .filter(|v| v.id == h.volume)
                            .any(|v| {
                                let r = helio_pass_voxel_planet::volume::bounding_radius(&v.planet)
                                    .unwrap_or(f64::INFINITY);
                                helio_pass_voxel_planet::volume::ray_ball(
                                    camera.eye,
                                    dir,
                                    v.transform.position,
                                    r,
                                )
                                .is_some_and(|(enter, leave)| {
                                    h.distance >= enter - 1.0 && h.distance <= leave + 1.0
                                })
                            })
                });
                if !plausible && out.other_volume < 4 {
                    eprintln!("pixel {x},{y}: drawn {drawn:?} past the casts' reach");
                }
                out.other_volume += usize::from(!plausible);
                continue;
            };
            let v = &volumes.volumes[cpu.index];
            let planet = &v.planet;
            let voxel = planet.grid().voxel_size();
            if cpu.hit.distance >= near_of(v) {
                out.coarse += 1;
                let same = drawn.is_some_and(|h| {
                    let cell = voxel * f64::from(1u32 << h.level);
                    h.volume == cpu.id
                        && (h.distance - cpu.hit.distance).abs()
                            <= 4.0 * cell + 1e-4 * cpu.hit.distance
                });
                if !same && out.other_volume < 4 {
                    eprintln!(
                        "pixel {x},{y}: drawn {drawn:?}, cpu volume {} at {} (coarse)",
                        cpu.id, cpu.hit.distance
                    );
                }
                out.other_volume += usize::from(!same);
                continue;
            }
            out.compared += 1;
            out.by_volume[cpu.index] += 1;
            let frame = if planet.grid().is_plane() {
                0.0
            } else {
                4.0 * 512.0 * voxel * voxel / planet.grid().radius()
            };
            // A ray along the edge between two solid cells meets both at
            // once: either is exact.
            let tie = |c: Cell| {
                c.face == cpu.hit.cell.face
                    && (c.i - cpu.hit.cell.i).abs()
                        + (c.j - cpu.hit.cell.j).abs()
                        + (c.k - cpu.hit.cell.k).abs()
                        == 1
                    && planet.solid(c)
            };
            let exact = drawn.is_some_and(|h| {
                h.volume == cpu.id
                    && h.level == 0
                    && (h.cell == cpu.hit.cell || tie(h.cell))
                    && (h.distance - cpu.hit.distance).abs()
                        <= 2e-3 + cpu.hit.distance * 1e-5 + frame
            });
            if !exact {
                out.mismatched += 1;
                if out.mismatched < 8 {
                    eprintln!(
                        "pixel {x},{y}: drawn {drawn:?}, cpu volume {} {:?} at {}",
                        cpu.id, cpu.hit.cell, cpu.hit.distance
                    );
                }
            }
        }
    }
    eprintln!("{out:?}");
    out
}

/// A land direction (surface above sea level) near `face` coordinates.
fn land(planet: &Planet, face: u8, fi: f64, fj: f64) -> DVec3 {
    let grid = planet.grid();
    let n = f64::from(grid.cells());
    for step in 0..400 {
        let a = fi + 0.013 * f64::from(step % 20);
        let b = fj + 0.017 * f64::from(step / 20);
        let (i, j) = ((a.fract() * n) as i32, (b.fract() * n) as i32);
        if planet.column_top(face, i, j, 0) > 40 {
            return grid.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5);
        }
    }
    panic!("no land found");
}

/// A transform far from the origin, turned about an oblique axis.
fn far_turned() -> VolumeTransform {
    VolumeTransform::new(
        DVec3::new(2.0e7, -1.3e7, 4.1e6),
        DQuat::from_axis_angle(DVec3::new(0.3, 0.8, -0.5).normalize(), 2.1),
    )
}

/// An Earth turned and moved 2.4e7 m from the origin draws its ground view
/// as an Earth at the origin does: level 0 at the same cells and distances
/// (the eye's f64 position, the frame's rotation and the chunks' frames
/// composed without losing the Earth's precision).
#[test]
fn a_turned_earth_far_from_the_origin_renders_exactly() {
    let Some(gpu) = gpu() else { return };
    let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let t = far_turned();
    let dir = land(&planet, 4, 0.37, 0.61);
    let eye = planet.surface_point(dir, 1.7);
    let up = eye.normalize();
    let forward = (up.any_orthonormal_vector() - up * 0.35).normalize();
    let camera = Camera::placed(&t, eye, forward, up, [320, 180]);
    let volumes = Volumes::new(vec![VoxelVolume::new(3, planet, t)]);
    let mut scene = Scene::new(gpu, &volumes);
    let hits = scene.settle(gpu, &camera);
    let a = compare(&hits, &volumes, &camera);
    assert!(a.compared > 1000, "{a:?}");
    assert!(a.mismatched * 1000 <= a.compared, "{a:?}");
}

/// A ship-sized block: a 12.8 m plane of 0.1 m voxels, its ground a slab
/// 1.2 m thick, a cabin 2.4 m across on it.
fn block() -> Planet {
    let mut planet = Planet::new(PlanetRecipe {
        shape: Shape::Plane,
        plane_size_m: 12.8,
        terrain: TerrainLayers::flat_at(1.2).source(3),
        ..Default::default()
    })
    .unwrap();
    planet
        .apply(Brush {
            center: [0.0, 2.4, 0.0],
            radius: 1.2,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: material::PLANKS,
            height: 0.0,
        })
        .unwrap();
    planet
}

/// A moon 600 km across, 0.5 m voxels, the Moon's ground. (A sphere keeps
/// about 1024 cells across a face at its coarsest level, all its roots
/// resident: a 10 km moon with the Moon's kilometres of relief held 30 000
/// root chunks.)
fn moon() -> Planet {
    Planet::new(PlanetRecipe {
        radius_m: 300_000.0,
        voxel_size_m: 0.5,
        terrain: TerrainLayers::moon().source(11),
        ..Default::default()
    })
    .unwrap()
}

/// A planet, a moon high over its horizon and a block hovering a few
/// metres in front of the eye, each placed and turned: every pixel is the
/// nearest volume's hit (the CPU's, in each one's frame), the block's and
/// the ground's exactly.
#[test]
fn a_planet_a_moon_and_a_block_draw_as_one_scene() {
    let Some(gpu) = gpu() else { return };
    let earth = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let dir = land(&earth, 4, 0.37, 0.61);
    let eye = earth.surface_point(dir, 3.0);
    let up = eye.normalize();
    let across = up.any_orthonormal_vector();
    // Looking out over the ground, a little down.
    let forward = (across - up * 0.15).normalize();
    // The moon 1 800 km away, high over the horizon, turned.
    let moon_at = eye + (across + up * 0.35).normalize() * 1_800_000.0;
    let moon = VoxelVolume::new(
        2,
        Arc::new(moon()),
        VolumeTransform::new(
            moon_at,
            DQuat::from_rotation_z(0.4) * DQuat::from_rotation_x(1.1),
        ),
    );
    // The block set into the ground 9 m ahead and 5 m to the right, turned
    // and leaning a little: its slab's top, sides and cabin show, and the
    // ground to its left whatever the ground's relief. (A plane's underside
    // is no world's: the CPU's casts clip a plane to its terrain's band, the
    // draw to its root chunks.)
    let right = forward.cross(up).normalize();
    let ground = earth.surface_point(eye + across * 9.0 + right * 5.0, 0.0);
    let block_at = ground - up * 1.0;
    let block = VoxelVolume::new(
        5,
        Arc::new(block()),
        VolumeTransform::new(
            block_at,
            DQuat::from_rotation_arc(DVec3::Y, (up + across * 0.05).normalize())
                * DQuat::from_rotation_y(0.6),
        ),
    );
    let volumes = Volumes::new(vec![
        VoxelVolume::new(1, earth, VolumeTransform::IDENTITY),
        moon,
        block,
    ]);
    let camera = Camera {
        eye,
        forward: forward.as_vec3(),
        up: up.as_vec3(),
        size: [320, 180],
    };
    // The horizon's chunks, the moon's and the block's: twice the slots.
    let mut scene = Scene::with_capacity(
        gpu,
        &volumes,
        Capacity {
            chunks: 2 * CAPACITY.chunks,
            ..CAPACITY
        },
    );
    let hits = scene.settle(gpu, &camera);
    let a = compare(&hits, &volumes, &camera);
    assert!(a.by_volume[0] > 500 && a.by_volume[2] > 500, "{a:?}");
    assert!(a.mismatched * 1000 <= a.compared, "{a:?}");
    // The moon's pixels (and the sky's) are the moon's (none) but at its
    // coarse silhouette.
    let moon_pixels = hits.iter().flatten().filter(|h| h.volume == 2).count();
    assert!(moon_pixels > 400, "{moon_pixels} moon pixels");
    assert!(a.other_volume * 50 <= a.coarse.max(1), "{a:?}");
}

/// A block flying and spinning past a camera that rides with it: each
/// frame its hits are where this frame's transform puts it (no frame of
/// lag), level 0 at its cells; then still, with the camera standing, its
/// hits after each move are its cells at its new place.
#[test]
fn a_moving_spinning_volume_renders_where_it_is_each_frame() {
    let Some(gpu) = gpu() else { return };
    let planet = Arc::new(block());
    // The camera 6 m off the block, looking at its cabin, in its frame.
    let eye = DVec3::new(4.5, 4.5, 4.5);
    let forward = (DVec3::new(0.0, 2.0, 0.0) - eye).normalize();
    let mut volumes = Volumes::new(vec![VoxelVolume::new(9, planet, VolumeTransform::IDENTITY)]);
    let mut scene = Scene::new(gpu, &volumes);
    let at = |frame: u32| {
        let s = f64::from(frame) * 0.05;
        VolumeTransform::new(
            DVec3::new(1.0e6 + 400.0 * s, 25.0 * s.sin(), -3.0e5 - 90.0 * s),
            DQuat::from_rotation_y(s * 1.3) * DQuat::from_rotation_x(s * 0.4),
        )
    };
    // Settled at the first place.
    volumes.volumes[0].transform = at(0);
    scene.place(&volumes);
    let camera = Camera::placed(&at(0), eye, forward, DVec3::Y, [256, 160]);
    let settled = scene.settle(gpu, &camera);
    let first = compare(&settled, &volumes, &camera);
    assert!(first.compared > 2000 && first.mismatched == 0, "{first:?}");
    // Flying: every frame's hits are the CPU's at that frame's place.
    for frame in 1..12 {
        volumes.volumes[0].transform = at(frame);
        scene.place(&volumes);
        let camera = Camera::placed(&at(frame), eye, forward, DVec3::Y, [256, 160]);
        scene.frame(gpu, &camera, None);
        let a = compare(&scene.world.read_hits(), &volumes, &camera);
        assert!(
            a.compared > 2000 && a.mismatched == 0,
            "frame {frame}: {a:?}"
        );
    }
    // A standing camera: the block moved and turned under it.
    let standing = Camera::placed(&at(11), eye, forward, DVec3::Y, [256, 160]);
    for frame in [13, 17] {
        // Turned as at `frame`, moved so the camera's point of the block's
        // frame stays where it was: the block in view.
        let mut t = at(frame);
        t.position += standing.eye - t.to_world(eye);
        volumes.volumes[0].transform = t;
        scene.place(&volumes);
        let hits = scene.settle(gpu, &standing);
        let a = compare(&hits, &volumes, &standing);
        assert!(
            a.compared > 500 && a.mismatched * 200 <= a.compared,
            "frame {frame}: {a:?}"
        );
    }
}

/// Picks name the volume they hit, at the hit the draw shows.
#[test]
fn picks_name_the_volume_they_hit() {
    let Some(gpu) = gpu() else { return };
    let earth = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let dir = land(&earth, 4, 0.37, 0.61);
    let eye = earth.surface_point(dir, 3.0);
    let up = eye.normalize();
    let across = up.any_orthonormal_vector();
    let forward = (across - up * 0.2).normalize();
    // The block on the ground ahead and to the right, the ground beside it.
    let block_at = eye + across * 14.0 + across.cross(up) * 5.0 - up * 2.5;
    let volumes = Volumes::new(vec![
        VoxelVolume::new(1, earth, VolumeTransform::IDENTITY),
        VoxelVolume::new(
            5,
            Arc::new(block()),
            VolumeTransform::new(block_at, DQuat::from_rotation_arc(DVec3::Y, up)),
        ),
    ]);
    let camera = Camera {
        eye,
        forward: forward.as_vec3(),
        up: up.as_vec3(),
        size: [256, 160],
    };
    let mut scene = Scene::new(gpu, &volumes);
    let hits = scene.settle(gpu, &camera);
    let mut picks = Picks::default();
    // A pixel each volume shows, by the middle of the most it shows.
    let at: Vec<[f32; 2]> = [1u64, 5]
        .iter()
        .map(|&id| {
            let shown: Vec<usize> = (0..hits.len())
                .filter(|&n| hits[n].is_some_and(|h| h.volume == id))
                .collect();
            assert!(
                shown.len() > 100,
                "volume {id} shows {} pixels",
                shown.len()
            );
            let n = shown[shown.len() / 2];
            [
                ((n % 256) as f32 + 0.5) / 256.0,
                ((n / 256) as f32 + 0.5) / 160.0,
            ]
        })
        .collect();
    picks.waiting = at
        .iter()
        .enumerate()
        .map(|(n, uv)| (n as u64, *uv))
        .collect();
    for _ in 0..6 {
        scene.frame(gpu, &camera, Some(&mut picks));
    }
    let results = picks.results();
    assert_eq!(results.len(), at.len(), "{results:?}");
    let mut volumes_hit = std::collections::BTreeSet::new();
    for r in results {
        let [u, v] = at[r.id as usize];
        let (x, y) = (((u * 256.0) as u32).min(255), ((v * 160.0) as u32).min(159));
        let drawn = hits[(y * 256 + x) as usize];
        assert_eq!(
            r.hit.map(|h| (h.volume, h.cell)),
            drawn.map(|h| (h.volume, h.cell)),
            "pick {r:?}"
        );
        if let Some(h) = r.hit {
            volumes_hit.insert(h.volume);
        }
    }
    assert_eq!(volumes_hit.into_iter().collect::<Vec<_>>(), vec![1, 5]);
}

/// Channel `channel` of an Rgba16Float target, row by row.
fn read_rgba16(gpu: &Gpu, texture: &wgpu::Texture, channel: usize) -> Vec<f32> {
    let [w, h] = [texture.width(), texture.height()];
    let row = (w * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let bytes = staging.slice(..).get_mapped_range().unwrap().to_vec();
    let half = |bits: u16| {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exponent = i32::from((bits >> 10) & 0x1f);
        let mantissa = f32::from(bits & 0x3ff);
        sign * match exponent {
            0 => mantissa / 1024.0 * 2f32.powi(-14),
            31 => f32::INFINITY,
            e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
        }
    };
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| {
            let at = (y * row + x * 8) as usize + channel * 2;
            half(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
        })
        .collect()
}

/// A volume turned and moved is lit as in its own frame: the same ground
/// view of a planet at the origin, sun `s`, and of the planet turned and
/// moved, sun turned with it, shows the same hits, the same sun (steady
/// shadows: every hit's own walks over the sun's disk) and normals turned
/// with it.
#[test]
fn a_turned_volume_is_lit_as_in_its_own_frame() {
    let Some(gpu) = gpu() else { return };
    let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let dir = land(&planet, 2, 0.41, 0.33);
    let eye = planet.surface_point(dir, 2.0);
    let up = eye.normalize();
    let forward = (up.any_orthonormal_vector() - up * 0.4).normalize();
    // A low sun: long shadows.
    let sun = (up * 0.25 + up.any_orthonormal_vector().cross(up)).normalize();
    let mut images = Vec::new();
    for t in [VolumeTransform::IDENTITY, far_turned()] {
        let volumes = Volumes::new(vec![VoxelVolume::new(1, planet.clone(), t)]);
        let camera = Camera::placed(&t, eye, forward, up, [256, 160]);
        let mut scene = Scene::new(gpu, &volumes);
        scene.sun = t.dir_to_world(sun).as_vec3();
        scene.world.set_steady_shadows(true);
        let hits = scene.settle(gpu, &camera);
        let targets = scene.world.targets().unwrap();
        let lit = read_rgba16(gpu, &targets.sun, 0);
        let normals: Vec<DVec3> = (0..3)
            .map(|c| read_rgba16(gpu, &targets.normal, c))
            .collect::<Vec<_>>()
            .chain_normals();
        // Back into the volume's frame.
        let normals: Vec<DVec3> = normals.iter().map(|n| t.dir_to_local(*n)).collect();
        images.push((hits, lit, normals));
    }
    let (a, b) = (&images[0], &images[1]);
    let (mut hits, mut same_hit, mut same_sun, mut same_normal) = (0, 0, 0, 0);
    for i in 0..a.0.len() {
        let (Some(ha), Some(hb)) = (a.0[i], b.0[i]) else {
            continue;
        };
        hits += 1;
        same_hit += usize::from(ha.cell == hb.cell && (ha.distance - hb.distance).abs() < 1e-2);
        same_sun += usize::from((a.1[i] - b.1[i]).abs() < 0.05);
        same_normal += usize::from(a.2[i].dot(b.2[i]) > 0.999);
    }
    eprintln!("hits {hits}: same hit {same_hit}, sun {same_sun}, normal {same_normal}");
    assert!(hits > 10_000);
    assert!(same_hit * 1000 >= hits * 995, "{same_hit}/{hits}");
    assert!(same_sun * 1000 >= hits * 990, "{same_sun}/{hits}");
    assert!(same_normal * 1000 >= hits * 990, "{same_normal}/{hits}");
}

/// Three channels' rows as vectors.
trait ChainNormals {
    fn chain_normals(self) -> Vec<DVec3>;
}

impl ChainNormals for Vec<Vec<f32>> {
    fn chain_normals(self) -> Vec<DVec3> {
        (0..self[0].len())
            .map(|i| {
                DVec3::new(
                    f64::from(self[0][i]),
                    f64::from(self[1][i]),
                    f64::from(self[2][i]),
                )
            })
            .collect()
    }
}

/// A world with a wall of brick on the sun's side of a spot of land, and a
/// ship-sized block (`block`) hovering a few metres over the ground beside
/// it: the ground's world, the block's transform, the land's point, up, and
/// the direction to the wall.
fn wall_and_block() -> (Arc<Planet>, VolumeTransform, DVec3, DVec3, DVec3) {
    let mut earth = Planet::new(PlanetRecipe::default()).unwrap();
    let dir = land(&earth, 4, 0.37, 0.61);
    let ground = earth.surface_point(dir, 0.0);
    let up = earth.grid().up(ground);
    let side = up.any_orthonormal_vector();
    earth
        .apply(Brush {
            center: (ground + side * 10.5 + up * 2.0).to_array(),
            radius: 3.0,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: material::BRICK,
            height: 0.0,
        })
        .unwrap();
    // The block's underside in its frame (its slab's), then 2.5 m over the
    // ground.
    let bottom = block()
        .raycast_of(DVec3::new(0.3, -50.0, 0.2), DVec3::Y, 100.0, Matter::All)
        .map_or(0.0, |h| h.distance - 50.0);
    let at = VolumeTransform::new(
        ground + up * (2.5 - bottom),
        DQuat::from_rotation_arc(DVec3::Y, up),
    );
    (Arc::new(earth), at, ground, up, side)
}

/// The sun at the hits of a settled view of `volumes` against CPU rays over
/// every volume: of hits facing the sun within the level-0 range whose
/// drawn place is the CPU's (same volume, distance within a few
/// centimetres), those compared, those whose drawn sun (steady shadows:
/// each hit's walks over the sun's disk) differs from the CPU ray's by more
/// than half (by receiver: lit, shadowed by itself, by another volume), and
/// those the CPU finds shadowed by another volume than their own, by
/// receiver. Hits whose CPU ray meets its caster farther than `reach` are
/// left out (casters out of the view: the drawn cut holds them coarse, or
/// not at all).
#[derive(Debug, Default)]
struct CrossShadows {
    compared: usize,
    wrong: usize,
    wrong_by: Vec<[usize; 3]>,
    shadowed_by_other: Vec<usize>,
}

fn cross_shadows(
    gpu: &Gpu,
    volumes: &Volumes,
    camera: &Camera,
    sun: DVec3,
    reach: f64,
) -> CrossShadows {
    let mut scene = Scene::with_capacity(
        gpu,
        volumes,
        Capacity {
            chunks: 2 * CAPACITY.chunks,
            ..CAPACITY
        },
    );
    scene.sun = sun.as_vec3();
    scene.world.set_steady_shadows(true);
    let hits = scene.settle(gpu, camera);
    let lit = read_rgba16(gpu, &scene.world.targets().unwrap().sun, 0);
    let view_proj = camera.view_proj();
    let near = 0.7 * 0.1 / camera.pixel_angle();
    let mut out = CrossShadows {
        wrong_by: vec![[0; 3]; volumes.volumes.len()],
        shadowed_by_other: vec![0; volumes.volumes.len()],
        ..Default::default()
    };
    for y in (0..camera.size[1]).step_by(2) {
        for x in (0..camera.size[0]).step_by(2) {
            let i = (y * camera.size[0] + x) as usize;
            let dir = camera.pixel_dir(view_proj, x, y);
            let (Some(drawn), Some(cpu)) =
                (hits[i], volumes.raycast(camera.eye, dir, near, Matter::All))
            else {
                continue;
            };
            if drawn.volume != cpu.id
                || (drawn.distance - cpu.hit.distance).abs() > 0.03 + 1e-3 * cpu.hit.distance
                || cpu.normal.dot(sun) < 0.15
            {
                continue;
            }
            let at = camera.eye + dir * cpu.hit.distance + cpu.normal * 1e-3;
            let blocker = volumes.raycast(at, sun, 1000.0, Matter::All);
            if blocker.is_some_and(|b| b.hit.distance > reach) {
                continue;
            }
            let cpu_lit = blocker.is_none();
            out.compared += 1;
            let by = blocker.map_or(0, |b| if b.index == cpu.index { 1 } else { 2 });
            out.shadowed_by_other[cpu.index] += usize::from(by == 2);
            if (f64::from(lit[i]) - if cpu_lit { 1.0 } else { 0.0 }).abs() > 0.5 {
                out.wrong += 1;
                out.wrong_by[cpu.index][by] += 1;
                if by == 0 && out.wrong_by[cpu.index][by] <= 12 {
                    // Where the sun ray passes the other volumes (their
                    // frames): its nearest point to each one's origin.
                    for v in &volumes.volumes {
                        let (o, d) = (v.transform.to_local(at), v.transform.dir_to_local(sun));
                        let t = (-o.dot(d)).max(0.0);
                        eprintln!(
                            "  vs {}: local origin {o:.2} dir {d:.3} nearest {:.2}",
                            v.id,
                            o + d * t
                        );
                    }
                }
                if out.wrong_by[cpu.index][by] <= 3 {
                    eprintln!(
                        "pixel {x},{y}: volume {} drawn sun {} cpu {} (blocker {:?})",
                        cpu.id,
                        lit[i],
                        u8::from(cpu_lit),
                        blocker.map(|b| b.id)
                    );
                }
            }
        }
    }
    eprintln!("{out:?}");
    out
}

/// Volumes shadow each other: under a low sun a wall of the ground's
/// shades the block hovering beside it, and the block shades the ground
/// under it; every sunward hit near the eye is in the sun where a CPU ray
/// toward it over every volume meets nothing, but a fiftieth (cells along
/// a shadow's edge, the sun's disk).
#[test]
fn volumes_shadow_each_other() {
    let Some(gpu) = gpu() else { return };
    let (earth, at, ground, up, side) = wall_and_block();
    let volumes = Volumes::new(vec![
        VoxelVolume::new(1, earth, VolumeTransform::IDENTITY),
        VoxelVolume::new(5, Arc::new(block()), at),
    ]);
    // The sun 20 degrees up over the wall; the eye back and up, looking
    // down at the block and the ground its shadow falls on.
    let sun = (side + up * 0.364).normalize();
    let fwd = up.cross(side);
    let eye = ground - fwd * 18.0 + up * 12.0 - side * 2.0;
    let camera = Camera {
        eye,
        forward: ((ground - side * 2.0) - eye).normalize().as_vec3(),
        up: up.as_vec3(),
        size: [640, 360],
    };
    let c = cross_shadows(gpu, &volumes, &camera, sun, 40.0);
    assert!(c.compared > 10_000, "{c:?}");
    assert!(
        c.shadowed_by_other[0] > 300 && c.shadowed_by_other[1] > 50,
        "{c:?}"
    );
    assert!(c.wrong * 50 <= c.compared, "{c:?}");
    for (wrong, shadowed) in c.wrong_by.iter().zip(&c.shadowed_by_other) {
        assert!(wrong[2] * 50 <= *shadowed, "{c:?}");
    }
}

/// GI rays from the probes about a block hovering over the ground cross
/// into it (`gi_trace_cross`) and keep the nearer hit: every ray whose CPU
/// hit over every volume is in view near the eye (level 0 drawn there) hits
/// at the CPU's distance with the CPU's face, the block's and the ground's
/// alike; a hundredth may differ (grazing cells).
#[test]
fn gi_rays_cross_into_other_volumes() {
    let Some(gpu) = gpu() else { return };
    let (earth, at, ground, up, side) = wall_and_block();
    let volumes = Volumes::new(vec![
        VoxelVolume::new(1, earth, VolumeTransform::IDENTITY),
        VoxelVolume::new(5, Arc::new(block()), at),
    ]);
    let fwd = up.cross(side);
    let eye = ground - fwd * 9.0 + up * 2.0 - side * 3.0;
    let camera = Camera {
        eye,
        forward: (at.position - eye).normalize().as_vec3(),
        up: up.as_vec3(),
        size: [320, 180],
    };
    // The finest level's probes within 8 m of the block, out of every
    // volume's rock; 64 random rays each.
    let clipmap = helio_gi_api::Clipmap::around(eye, helio_gi_api::DEFAULT_FINEST_SPACING);
    let probes: Vec<u32> = (0..helio_gi_api::LEVEL_PROBES)
        .filter(|&p| {
            let o = clipmap.position(p);
            o.distance(at.position) < 8.0 && !volumes.solid(o, Matter::All)
        })
        .take(400)
        .collect();
    let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
    let mut unit = || loop {
        let v =
            DVec3::new(next() as f64, next() as f64, next() as f64) / u64::MAX as f64 * 2.0 - 1.0;
        if v.length_squared() > 1e-4 && v.length_squared() <= 1.0 {
            return v.normalize();
        }
    };
    let dirs: Vec<DVec3> = (0..probes.len() * helio_gi_api::RAYS as usize)
        .map(|_| unit())
        .collect();
    let mut scene = Scene::new(gpu, &volumes);
    scene.gi = Some(gi_rays(
        gpu,
        eye,
        &probes,
        &dirs.iter().map(|d| d.as_vec3()).collect::<Vec<_>>(),
        scene.sun,
    ));
    scene.settle(gpu, &camera);
    // One frame's hits (the GI pass clears them each frame).
    let hits_buffer = &scene.gi.as_ref().unwrap()[2];
    gpu.queue
        .write_buffer(hits_buffer, 0, &vec![0u8; hits_buffer.size() as usize]);
    scene.frame(gpu, &camera, None);
    let hits = read_words(gpu, &scene.gi.as_ref().unwrap()[2]);
    let view_proj = camera.view_proj();
    let near = 0.7 * 0.1 / camera.pixel_angle();
    let (mut compared, mut wrong, mut by_volume) = (0, 0, [0usize; 2]);
    for (k, d) in dirs.iter().enumerate() {
        let origin = clipmap.position(probes[k / helio_gi_api::RAYS as usize]);
        let Some(cpu) = volumes.raycast(origin, *d, 30.0, Matter::All) else {
            continue;
        };
        let hit_at = origin + *d * cpu.hit.distance;
        let clip = view_proj * (hit_at - eye).as_vec3().extend(1.0);
        let ndc = clip.truncate() / clip.w;
        if clip.w <= 0.0
            || ndc.x.abs() > 0.95
            || ndc.y.abs() > 0.95
            || (hit_at - eye).length() > near
        {
            continue;
        }
        compared += 1;
        by_volume[cpu.index] += 1;
        let hit = &hits[k * 4..k * 4 + 4];
        let t = f32::from_bits(hit[0]);
        let n = (DVec3::new(
            f64::from(hit[1] & 1023),
            f64::from((hit[1] >> 10) & 1023),
            f64::from((hit[1] >> 20) & 1023),
        ) / 1023.0
            * 2.0
            - 1.0)
            .normalize_or_zero();
        let exact = t > 0.0
            && (f64::from(t) - cpu.hit.distance).abs() <= 2e-3 + cpu.hit.distance * 1e-5
            && n.dot(cpu.normal) > 0.99;
        if !exact {
            wrong += 1;
            if wrong < 8 {
                eprintln!(
                    "ray {k}: drawn t {t} n {n}, cpu volume {} at {} n {}",
                    cpu.id, cpu.hit.distance, cpu.normal
                );
            }
        }
    }
    eprintln!(
        "GI rays: compared {compared} (ground {}, block {}), wrong {wrong}",
        by_volume[0], by_volume[1]
    );
    assert!(by_volume[0] > 500 && by_volume[1] > 500, "{by_volume:?}");
    assert!(wrong * 100 <= compared, "{wrong} of {compared}");
}

/// GI's buffers for rays from `probes` (storage indices of a clipmap around
/// `eye`) along `dirs` (`RAYS` a probe), the sun towards `sun` (as
/// tests/chunks.rs's).
fn gi_rays(gpu: &Gpu, eye: DVec3, probes: &[u32], dirs: &[Vec3], sun: Vec3) -> [wgpu::Buffer; 4] {
    use wgpu::util::DeviceExt;
    let clipmap = helio_gi_api::Clipmap::around(eye, helio_gi_api::DEFAULT_FINEST_SPACING);
    let frame = helio_gi_api::GpuFrame {
        levels: clipmap.levels(eye),
        sky: [0.0; 4],
        ground: [0.0; 4],
        counts: [probes.len() as u32, 0, 0, 0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        prev_rotation: [0.0, 0.0, 0.0, 1.0],
    };
    let mut rays = Vec::with_capacity(dirs.len() * 8);
    for (k, d) in dirs.iter().enumerate() {
        let probe = probes[k / helio_gi_api::RAYS as usize];
        let o = (clipmap.position(probe) - eye).as_vec3();
        rays.extend_from_slice(&[
            o.x.to_bits(),
            o.y.to_bits(),
            o.z.to_bits(),
            1e30f32.to_bits(),
            d.x.to_bits(),
            d.y.to_bits(),
            d.z.to_bits(),
            probe,
        ]);
    }
    let init = |label, contents: &[u8], usage| {
        gpu.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
    };
    let storage =
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    [
        init(
            "gi frame",
            bytemuck::bytes_of(&frame),
            wgpu::BufferUsages::UNIFORM,
        ),
        init("gi rays", bytemuck::cast_slice(&rays), storage),
        init("gi hits", &vec![0u8; dirs.len() * 16], storage),
        init(
            "gi sun",
            bytemuck::cast_slice(&[sun.x, sun.y, sun.z, 0.0, 1.0, 1.0, 1.0, 0.0]),
            storage,
        ),
    ]
}

fn read_words(gpu: &Gpu, buffer: &wgpu::Buffer) -> Vec<u32> {
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    gpu.queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec()
}

fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}
