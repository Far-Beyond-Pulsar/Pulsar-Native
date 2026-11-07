//! A northern lake: Canadian-Shield granite shelving into dark water, pine and
//! fir forest behind it, islands, ferns and moss. Units are metres; the lake
//! surface is y = 0. The terrain, its materials and every plant and rock are
//! built here from the height function, so the water's shore map and the meshes
//! agree.
use crate::common::{spawn_mesh, spawn_object};
use glam::{Mat4, Quat, Vec3};
use helio_mats::texture_mips::{mip_chain, TexelKind};
use pulsar_scenedb::{Entity, World};
use std::path::{Path, PathBuf};

// ── noise ────────────────────────────────────────────────────────────────

fn hash2(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374761393)
        ^ (z as u32).wrapping_mul(668265263)
        ^ seed.wrapping_mul(2246822519);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Smooth value noise, about -1..1.
fn noise(x: f32, z: f32, seed: u32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (fx, fz) = (x - xi, z - zi);
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fz * fz * fz * (fz * (fz * 6.0 - 15.0) + 10.0);
    let (xi, zi) = (xi as i32, zi as i32);
    let a = hash2(xi, zi, seed);
    let b = hash2(xi + 1, zi, seed);
    let c = hash2(xi, zi + 1, seed);
    let d = hash2(xi + 1, zi + 1, seed);
    let top = a + (b - a) * u;
    let bottom = c + (d - c) * u;
    (top + (bottom - top) * v) * 2.0 - 1.0
}

fn fbm(x: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut freq) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        sum += amp * noise(x * freq, z * freq, seed + o * 31);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ── the lake ─────────────────────────────────────────────────────────────

/// Shore radius (m) from the lake's centre at angle `th`.
fn shore_radius(th: f32) -> f32 {
    520.0 + 90.0 * (2.0 * th + 1.0).sin() + 60.0 * (3.0 * th + 0.3).sin()
        + 40.0 * (5.0 * th + 2.0).sin() + 25.0 * (9.0 * th + 0.7).sin()
}

/// (x, z, radius, height above the water) of each island.
const ISLANDS: [(f32, f32, f32, f32); 7] = [
    (-110.0, -90.0, 38.0, 5.0),
    (70.0, -190.0, 26.0, 4.2),
    (190.0, 40.0, 60.0, 6.5),
    (-210.0, 60.0, 24.0, 3.4),
    (30.0, -30.0, 15.0, 2.8),
    (250.0, -140.0, 44.0, 5.2),
    (-60.0, 170.0, 20.0, 3.0),
];

/// Signed distance (m) from the shoreline: positive on land, negative in water.
pub fn shore_s(x: f32, z: f32) -> f32 {
    (x * x + z * z).sqrt() - shore_radius(z.atan2(x))
}

pub fn height(x: f32, z: f32) -> f32 {
    let s = shore_s(x, z);
    let inland = smoothstep(40.0, 280.0, s);
    let mut h = if s < 0.0 {
        // The bed: shelving granite, steeper out in the lake.
        let shelf = (s * 0.062).max(-16.0);
        shelf + 0.5 * fbm(x / 18.0, z / 18.0, 3, 7) * smoothstep(-2.0, -14.0, s)
    } else {
        // Granite shelving up from the water, then forested hills.
        let base = 0.062 * s.min(36.0) + 0.04 * (s - 36.0).max(0.0);
        let ledges = 0.8 * (fbm(x / 6.5, z / 6.5, 3, 11) * 0.5 + 0.5).powi(2)
            * smoothstep(0.0, 12.0, s) * (1.0 - smoothstep(25.0, 90.0, s));
        let hills = 46.0 * inland * (fbm(x / 330.0, z / 330.0, 4, 3) * 0.5 + 0.5)
            + 5.0 * fbm(x / 42.0, z / 42.0, 3, 5) * smoothstep(10.0, 90.0, s);
        base + ledges + hills + 0.5 * fbm(x / 9.0, z / 9.0, 3, 13) * smoothstep(0.0, 20.0, s)
    };
    for &(cx, cz, r, top) in &ISLANDS {
        let d2 = ((x - cx).powi(2) + (z - cz).powi(2)) / (r * r);
        h += (top + 4.0) * (-1.3 * d2).exp()
            + 0.6 * fbm(x / 8.0, z / 8.0, 2, 17) * (-d2).exp();
    }
    h
}

pub struct Lake;
impl helio_pass_water_sim::shoreline::WaterTerrainProvider for Lake {
    fn elevation_y(&self, x: f64, z: f64) -> Option<f64> {
        Some(f64::from(height(x as f32, z as f32)))
    }
}

fn normal_at(x: f32, z: f32) -> Vec3 {
    let e = 0.6;
    let dx = (height(x + e, z) - height(x - e, z)) / (2.0 * e);
    let dz = (height(x, z + e) - height(x, z - e)) / (2.0 * e);
    Vec3::new(-dx, 1.0, -dz).normalize()
}

// ── cameras ──────────────────────────────────────────────────────────────

/// A shot: where the camera stands, where it looks and how it is lensed.
pub struct View {
    pub name: &'static str,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Vertical field of view (radians).
    pub fov: f32,
    /// Focus distance (m).
    pub focus_m: f32,
}

fn yaw_to(from: Vec3, to: Vec3) -> f32 {
    (to.x - from.x).atan2(-(to.z - from.z))
}

fn shore_point(theta: f32, s: f32, eye: f32) -> Vec3 {
    let r = shore_radius(theta) + s;
    let (x, z) = (r * theta.cos(), r * theta.sin());
    Vec3::new(x, height(x, z) + eye, z)
}

/// Every camera, in the order they are captured.
pub fn views() -> Vec<View> {
    let pi = std::f32::consts::PI;
    let hero = shore_point(pi * 0.5, 5.0, 1.7);
    let low = shore_point(0.12, 1.0, 0.5);
    let canopy = shore_point(pi * 0.5 - 0.35, 42.0, 1.6);
    let overtops_ground = shore_point(pi * 1.0, 70.0, 0.0);
    let island = Vec3::new(190.0, height(190.0, 40.0) + 2.2, 40.0);
    let wide = shore_point(pi * 0.5 + 0.5, 8.0, 1.7);
    let through = shore_point(pi * 0.5 + 0.15, 30.0, 1.6);
    let lake = Vec3::new(0.0, 0.0, 0.0);
    let at = |p: Vec3, q: Vec3| yaw_to(p, q);
    vec![
        View { name: "hero", position: hero, yaw: at(hero, lake) + 0.12, pitch: 0.05, fov: 0.78, focus_m: 18.0 },
        View { name: "low", position: low, yaw: at(low, Vec3::new(-120.0, 0.0, -260.0)), pitch: 0.03, fov: 0.78, focus_m: 40.0 },
        View { name: "canopy", position: canopy, yaw: at(canopy, lake) + 2.7, pitch: 0.95, fov: 1.05, focus_m: 25.0 },
        View { name: "overtops", position: Vec3::new(overtops_ground.x, overtops_ground.y + 52.0, overtops_ground.z), yaw: at(overtops_ground, lake), pitch: -0.08, fov: 0.95, focus_m: 400.0 },
        View { name: "island", position: island, yaw: at(island, Vec3::new(0.0, 0.0, -600.0)) - 0.2, pitch: 0.04, fov: 0.8, focus_m: 120.0 },
        View { name: "sky", position: wide, yaw: at(wide, lake) - 0.3, pitch: 0.34, fov: 1.15, focus_m: 150.0 },
        View { name: "forest", position: through, yaw: at(through, lake), pitch: 0.06, fov: 0.8, focus_m: 14.0 },
    ]
}

// ── materials ────────────────────────────────────────────────────────────

fn register(
    store: &std::sync::RwLock<pulsar_scenedb::gpu::TextureStore>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: Option<&str>,
    mut width: u32,
    mut height_px: u32,
    mut data: Vec<u8>,
    kind: TexelKind,
    cap: u32,
) -> Result<u32, String> {
    while width.max(height_px) > cap && width > 1 && height_px > 1 {
        let img = image::RgbaImage::from_raw(width, height_px, data).ok_or("invalid RGBA texture")?;
        width /= 2;
        height_px /= 2;
        data = image::imageops::resize(&img, width, height_px, image::imageops::FilterType::Triangle).into_raw();
    }
    let chain = mip_chain(&data, width, height_px, kind);
    let refs: Vec<&[u8]> = chain.iter().map(|l| l.texels.as_slice()).collect();
    let descriptor = wgpu::TextureDescriptor {
        label,
        size: wgpu::Extent3d { width, height: height_px, depth_or_array_layers: 1 },
        mip_level_count: refs.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if kind.is_srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    store
        .write()
        .map_err(|e| e.to_string())?
        .register_mips(device, queue, &descriptor, &refs)
        .map_err(|e| format!("{e:?}"))
}

fn texture_store(world: &World) -> Result<std::sync::Arc<std::sync::RwLock<pulsar_scenedb::gpu::TextureStore>>, String> {
    let mirror = world.gpu_mirror().cloned().ok_or("no GPU mirror")?;
    mirror.texture_store().ok_or_else(|| "no texture store".to_string())
}

/// A material from a Poly Haven texture set (`diff.jpg`, `nor.jpg`, `arm.jpg`).
fn texture_material(
    world: &mut World,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dir: &Path,
    cap: u32,
) -> Result<Entity, String> {
    let store = texture_store(world)?;
    let load = |name: &str, kind| -> Result<u32, String> {
        let img = image::open(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))?.to_rgba8();
        let (w, h) = img.dimensions();
        register(&store, device, queue, Some(name), w, h, img.into_raw(), kind, cap)
    };
    let mut row = helio_mats::host_rows::MaterialComponent::from(helio_mats::MaterialRow::new(
        [1.0, 1.0, 1.0, 1.0], 1.0, 0.0, [0.0; 3], 0.0,
    ));
    row.tex_base_color = load("diff.jpg", TexelKind::Colour)?;
    row.tex_normal = load("nor.jpg", TexelKind::Normal)?;
    let arm = load("arm.jpg", TexelKind::Data)?;
    row.tex_roughness = arm;
    row.tex_occlusion = arm;
    row.flags |= helio_mats::FLAG_HAS_NORMAL_MAP;
    let id = world.spawn();
    world.insert(id, row);
    Ok(id)
}

/// Shield granite is grey: the scans' warm sandstone tint is mostly removed.
fn cool_granite(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let l = 0.3 * r + 0.59 * g + 0.11 * b;
        let tint = [0.97, 1.0, 1.03];
        for k in 0..3 {
            let grey = l * tint[k];
            let c = [r, g, b][k];
            p[k] = (grey * 0.9 + c * 0.1).min(255.0) as u8;
        }
    }
}

fn upload_materials(
    world: &mut World,
    scene: &helio_asset_compat::ConvertedScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cap: u32,
    granite: bool,
) -> Result<Vec<Entity>, String> {
    let store = texture_store(world)?;
    let mut kinds = vec![None; scene.textures.len()];
    for material in &scene.materials {
        let t = &material.textures;
        let mut mark = |r: &Option<helio_asset_compat::ConvertedTextureRef>, k: TexelKind| {
            if let Some(r) = r {
                kinds[r.texture_index] = Some(k);
            }
        };
        mark(&t.base_color, TexelKind::Colour);
        mark(&t.emissive, TexelKind::Colour);
        mark(&t.normal, TexelKind::Normal);
        mark(&t.roughness_metallic, TexelKind::Data);
        mark(&t.occlusion, TexelKind::Data);
    }
    let mut slots = Vec::new();
    for (index, texture) in scene.textures.iter().enumerate() {
        let kind = kinds[index].unwrap_or(if texture.format == wgpu::TextureFormat::Rgba8UnormSrgb {
            TexelKind::Colour
        } else {
            TexelKind::Data
        });
        let mut data = texture.data.clone();
        if granite && matches!(kind, TexelKind::Colour) {
            cool_granite(&mut data);
        }
        slots.push(register(
            &store, device, queue, texture.label.as_deref(), texture.width, texture.height,
            data, kind, cap,
        )?);
    }
    let mut ids = Vec::new();
    for material in &scene.materials {
        let mut row = helio_mats::host_rows::MaterialComponent::from(helio_mats::MaterialRow::from(material.gpu));
        let t = &material.textures;
        if let Some(r) = t.base_color { row.tex_base_color = slots[r.texture_index]; }
        if let Some(r) = t.normal { row.tex_normal = slots[r.texture_index]; row.flags |= helio_mats::FLAG_HAS_NORMAL_MAP; }
        if let Some(r) = t.roughness_metallic { row.tex_roughness = slots[r.texture_index]; }
        if let Some(r) = t.occlusion { row.tex_occlusion = slots[r.texture_index]; }
        if let Some(r) = t.emissive { row.tex_emissive = slots[r.texture_index]; }
        // Leaves arrive alpha-blended; cut them out instead (sorted blending of
        // thousands of cards is wrong, and virtual geometry draws only cutouts).
        if row.flags & helio_mats::FLAG_ALPHA_BLEND != 0 {
            row.flags &= !helio_mats::FLAG_ALPHA_BLEND;
            row.flags |= helio_mats::FLAG_ALPHA_TEST | helio_mats::FLAG_DOUBLE_SIDED;
        }
        let id = world.spawn();
        world.insert(id, row);
        ids.push(id);
    }
    Ok(ids)
}

// ── terrain meshes ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ground {
    /// Lichen-covered granite: the shield's bedrock.
    Granite,
    /// The lake bed: dark, wet pebbles.
    Pebbles,
    /// Grass and clover, the lush edge of the forest.
    Grass,
    /// Needle duff under the conifers.
    Duff,
    /// Dark humus.
    Humus,
    /// Far away the forest is a canopy: a dark green ground stands in for the
    /// crowns between the individual trees.
    Canopy,
}
const GROUNDS: [Ground; 6] = [Ground::Granite, Ground::Pebbles, Ground::Grass, Ground::Duff, Ground::Humus, Ground::Canopy];

fn ground_at(x: f32, z: f32) -> Ground {
    let h = height(x, z);
    let s = shore_s(x, z);
    let slope = 1.0 - normal_at(x, z).y;
    let n = fbm(x / 35.0, z / 35.0, 3, 41);
    let m = fbm(x / 70.0, z / 70.0, 2, 53);
    if s < -1.0 {
        Ground::Pebbles
    } else if s < 0.0 || h < 0.25 || slope > 0.085 || s < 3.0 + 5.0 * n {
        Ground::Granite
    } else if s > 260.0 {
        Ground::Canopy
    } else if s < 70.0 && n > -0.05 {
        Ground::Grass
    } else if m > 0.3 {
        Ground::Humus
    } else if m > -0.12 {
        Ground::Duff
    } else {
        Ground::Grass
    }
}

struct Lists {
    vertices: Vec<helio_mesh_api::PackedVertex>,
    indices: Vec<u32>,
    last_tile: usize,
    base: u32,
}

/// One lattice of `tiles` x `tiles` tiles, each `cells` cells on a side of
/// `cell` metres, centred on the origin. Each tile owns its vertices with UVs in
/// 0..1, so a texture repeats per tile whatever the sampler's wrap mode, and each
/// cell goes to the ground class its centre says. `skip`: tiles centred inside
/// this half extent are left to a finer lattice.
fn lattice(tiles: usize, cells: usize, cell: f32, drop: f32, skip: f32) -> [Lists; 6] {
    let mut lists: [Lists; 6] = std::array::from_fn(|_| Lists { vertices: Vec::new(), indices: Vec::new(), last_tile: usize::MAX, base: 0 });
    let tile = cell * cells as f32;
    let origin = -(tiles as f32) * tile * 0.5;
    let per = cells + 1;
    for tj in 0..tiles {
        for ti in 0..tiles {
            let tile_index = tj * tiles + ti;
            let (cx, cz) = (origin + (ti as f32 + 0.5) * tile, origin + (tj as f32 + 0.5) * tile);
            if cx.abs() < skip && cz.abs() < skip {
                continue;
            }
            let mut positions = Vec::with_capacity(per * per);
            for j in 0..per {
                for i in 0..per {
                    let x = origin + ti as f32 * tile + i as f32 * cell;
                    let z = origin + tj as f32 * tile + j as f32 * cell;
                    positions.push((x, z, height(x, z) - drop));
                }
            }
            for j in 0..cells {
                for i in 0..cells {
                    let (x, z, _) = positions[j * per + i];
                    let class = GROUNDS.iter().position(|g| *g == ground_at(x + cell * 0.5, z + cell * 0.5)).unwrap();
                    let list = &mut lists[class];
                    if list.last_tile != tile_index {
                        list.last_tile = tile_index;
                        list.base = list.vertices.len() as u32;
                        for (k, &(x, z, y)) in positions.iter().enumerate() {
                            let n = normal_at(x, z);
                            list.vertices.push(helio_mesh_api::PackedVertex::from_components(
                                [x, y, z],
                                n.to_array(),
                                [(k % per) as f32 / cells as f32, (k / per) as f32 / cells as f32],
                                Vec3::new(1.0, -n.x / n.y.max(0.2), 0.0).normalize().to_array(),
                                1.0,
                            ));
                        }
                    }
                    let a = list.base + (j * per + i) as u32;
                    let b = a + per as u32;
                    list.indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
                }
            }
        }
    }
    lists
}

fn build_terrain(world: &mut World, device: &wgpu::Device, queue: &wgpu::Queue, root: &Path) -> Result<(), String> {
    let tex = root.join("tex");
    let mut materials = [
        texture_material(world, device, queue, &tex.join("mossy_rock"), 2048)?,
        texture_material(world, device, queue, &tex.join("pebbles"), 2048)?,
        texture_material(world, device, queue, &tex.join("leafy_grass"), 2048)?,
        texture_material(world, device, queue, &tex.join("forrest_ground_01"), 2048)?,
        texture_material(world, device, queue, &tex.join("mud_forest"), 2048)?,
        texture_material(world, device, queue, &tex.join("leafy_grass"), 2048)?,
    ];
    // The grass sets are dry and pale: tint them to a lush summer green.
    for (class, tint) in [(2usize, [0.45, 0.72, 0.30, 1.0f32]), (3, [0.50, 0.68, 0.34, 1.0]), (5, [0.16, 0.30, 0.10, 1.0])] {
        if let Some(mut row) = world.get_mut::<helio_mats::host_rows::MaterialComponent>(materials[class]) {
            row.base_color = tint;
        }
    }
    let materials = materials;
    // Fine near the shore (2 m cells, 8 m tiles), coarse out to the horizon
    // (16 m cells, 64 m tiles) and a metre under the fine lattice at its edge.
    let fine = lattice(175, 4, 2.0, 0.0, 0.0);
    let coarse = lattice(141, 4, 16.0, 1.2, 700.0);
    for (lists, radius) in [(fine, 1100.0), (coarse, 7000.0)] {
        for (class, list) in lists.into_iter().enumerate() {
            if list.indices.is_empty() {
                continue;
            }
            let mesh = spawn_mesh(world, helio_mesh_api::MeshUpload { vertices: list.vertices, indices: list.indices });
            spawn_object(world, mesh, materials[class], Mat4::IDENTITY, radius).map_err(str::to_owned)?;
        }
    }
    Ok(())
}

// ── scatter ──────────────────────────────────────────────────────────────

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / 16_777_216.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
    /// Skewed to small: most plants are young, a few are old.
    fn size(&mut self, lo: f32, hi: f32, bias: f32) -> f32 {
        lo + self.next().powf(bias) * (hi - lo)
    }
}

/// One placed object: ground position, its largest dimension, and a pose.
#[derive(Clone, Copy)]
struct Inst {
    x: f32,
    z: f32,
    size: f32,
    yaw: f32,
    /// Roll and pitch (radians): logs lie down, trees lean a little.
    tilt: [f32; 2],
    /// Metres up (or into the ground, negative).
    lift: f32,
}

fn inst(x: f32, z: f32, size: f32, rng: &mut Rng) -> Inst {
    Inst { x, z, size, yaw: rng.range(0.0, std::f32::consts::TAU), tilt: [0.0; 2], lift: 0.0 }
}

fn model_path(root: &Path, id: &str) -> Option<PathBuf> {
    let dir = root.join("models").join(id);
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "gltf"))
}

/// Instances `id`'s scan at each of `instances`; the scan's largest dimension is
/// scaled to the instance's size and its base set on the ground.
fn place(
    world: &mut World,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    root: &Path,
    id: &str,
    instances: &[Inst],
    cap: u32,
    vg: &mut Option<crate::vg_support::Vg>,
    variants: bool,
) -> Result<usize, String> {
    if instances.is_empty() {
        return Ok(0);
    }
    let Some(path) = model_path(root, id) else {
        eprintln!("forest kit: {id} missing, skipped");
        return Ok(0);
    };
    let scene = match helio_asset_compat::load_scene_file_with_config(&path, Default::default()) {
        Ok(scene) => scene,
        Err(e) => {
            eprintln!("forest kit: {id}: {e}, skipped");
            return Ok(0);
        }
    };
    let granite = matches!(id, "boulder_01" | "rock_07" | "rock_09" | "stone_01" | "rock_face_01" | "rock_face_02");
    let materials = upload_materials(world, &scene, device, queue, cap, granite)?;
    // `variants`: the file holds several alternative trees (its source meshes),
    // each instance is one of them. Otherwise the whole file is one object.
    let groups: Vec<Vec<usize>> = if variants {
        let mut ids: Vec<usize> = scene.meshes.iter().map(|m| m.source_mesh).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.iter()
            .map(|id| (0..scene.meshes.len()).filter(|&k| scene.meshes[k].source_mesh == *id).collect())
            .collect()
    } else {
        vec![(0..scene.meshes.len()).collect()]
    };
    let bounds: Vec<(f32, Vec3)> = groups
        .iter()
        .map(|group| {
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for &k in group {
                let mesh = &scene.meshes[k];
                for v in &mesh.vertices {
                    let p = mesh.node_transform.transform_point3(Vec3::from_array(v.position));
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            ((hi - lo).max_element().max(0.001), Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5))
        })
        .collect();
    enum Part {
        Classic(Entity),
        Vg(crate::vg_support::Asset),
    }
    let mut parts: Vec<Option<Part>> = (0..scene.meshes.len()).map(|_| None).collect();
    let mut objects = 0;
    for (n, i) in instances.iter().enumerate() {
        let g = if groups.len() > 1 { ((i.yaw * 997.0) as usize + n) % groups.len() } else { 0 };
        let (size, base) = bounds[g];
        let rotation = Quat::from_rotation_y(i.yaw) * Quat::from_euler(glam::EulerRot::XZY, i.tilt[1], i.tilt[0], 0.0);
        for &k in &groups[g] {
            let source = &scene.meshes[k];
            let triangles = source.indices.len() as u64 / 3;
            if parts[k].is_none() {
                parts[k] = Some(match vg.as_mut() {
                    Some(vg) if triangles >= vg_min_triangles() => {
                        Part::Vg(vg.asset(world, &source.vertices, &source.indices)?)
                    }
                    _ => Part::Classic(spawn_mesh(
                        world,
                        helio_mesh_api::MeshUpload { vertices: source.vertices.clone(), indices: source.indices.clone() },
                    )),
                });
            }
            let material = *materials.get(source.material_index.unwrap_or(0)).ok_or("scan material missing")?;
            let transform = Mat4::from_scale_rotation_translation(
                Vec3::splat(i.size / size),
                rotation,
                Vec3::new(i.x, height(i.x, i.z) + i.lift - i.size * 0.01, i.z),
            ) * Mat4::from_translation(-base)
                * source.node_transform;
            match parts[k].as_ref().unwrap() {
                Part::Classic(mesh) => {
                    spawn_object(world, *mesh, material, transform, i.size * 1.1).map_err(str::to_owned)?;
                }
                Part::Vg(asset) => {
                    vg.as_mut().unwrap().instance(world, *asset, material, transform, i.size * 1.1, triangles)?;
                }
            }
            objects += 1;
        }
    }
    Ok(objects)
}

/// Parts with at least this many triangles go through virtual geometry (when the
/// device has it); smaller ones stay classic.
fn vg_min_triangles() -> u64 {
    std::env::var("MUSKOKA_VG_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000)
}

/// Jittered grid over the square of half extent `half`: one candidate per cell
/// of `cell` metres, kept with the probability `keep(x, z)` gives.
fn candidates(rng: &mut Rng, half: f32, cell: f32, mut keep: impl FnMut(f32, f32, &mut Rng) -> bool) -> Vec<(f32, f32)> {
    let n = (2.0 * half / cell) as i32;
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let x = -half + (i as f32 + rng.next()) * cell;
            let z = -half + (j as f32 + rng.next()) * cell;
            if keep(x, z, rng) {
                out.push((x, z));
            }
        }
    }
    out
}

fn near_any(points: &[Vec3], x: f32, z: f32, radius: f32) -> bool {
    points.iter().any(|p| (p.x - x).powi(2) + (p.z - z).powi(2) < radius * radius)
}

/// The whole scene. `density` scales every planting (1.0 is the full forest).
pub fn build(world: &mut World, root: &Path, device: &wgpu::Device, queue: &wgpu::Queue, density: f32, vg: &mut Option<crate::vg_support::Vg>) -> Result<(), String> {
    build_terrain(world, device, queue, root)?;
    let focus: Vec<Vec3> = views().iter().map(|v| v.position).collect();
    let mut rng = Rng(60_606);
    let mut total = 0usize;
    macro_rules! put {
        ($id:expr, $list:expr, $cap:expr) => {{
            let n = place(world, device, queue, root, $id, &$list, $cap, vg, false)?;
            eprintln!("forest: {:>26} x{:<6} ({} objects)", $id, $list.len(), n);
            total += n;
        }};
    }
    let stand = |x: f32, z: f32| (fbm(x / 95.0, z / 95.0, 3, 77) * 0.5 + 0.5).clamp(0.0, 1.0);

    // ── shore rocks: the granite's boulders, along the whole shoreline ──
    let boulder_ids = ["boulder_01", "rock_07", "rock_09", "stone_01", "rock_moss_set_01", "rock_moss_set_02"];
    let mut by_rock: Vec<Vec<Inst>> = vec![Vec::new(); boulder_ids.len()];
    let spots = candidates(&mut rng, 760.0, 5.5, |x, z, r| {
        let s = shore_s(x, z);
        s > -5.0 && s < 22.0 && r.next() < (0.55 * density).min(1.0)
    });
    for (x, z) in spots {
        let k = (rng.next() * boulder_ids.len() as f32) as usize % boulder_ids.len();
        let size = rng.size(0.7, 6.5, 3.2);
        by_rock[k].push(inst(x, z, size, &mut rng));
    }
    for (k, id) in boulder_ids.iter().enumerate() {
        put!(id, by_rock[k], 1024);
    }
    // Big slabs and outcrops where the cameras stand.
    let mut slabs: Vec<Inst> = Vec::new();
    for (x, z) in candidates(&mut rng, 760.0, 19.0, |x, z, r| {
        let s = shore_s(x, z);
        s > -2.0 && s < 16.0 && near_any(&focus, x, z, 260.0) && r.next() < 0.7
    }) {
        slabs.push(inst(x, z, rng.range(9.0, 24.0), &mut rng));
    }
    let half = slabs.len() / 2;
    put!("rock_face_01", slabs[..half], 2048);
    put!("rock_face_02", slabs[half..], 2048);
    let mut islets: Vec<Inst> = Vec::new();
    for &(cx, cz, r, _) in &ISLANDS {
        for _ in 0..((r / 9.0) as usize + 2) {
            let a = rng.range(0.0, std::f32::consts::TAU);
            let d = rng.range(0.2, 1.1) * r;
            islets.push(inst(cx + a.cos() * d, cz + a.sin() * d, rng.size(3.0, 9.0, 1.6), &mut rng));
        }
    }
    put!("rock_09", islets, 2048);
    let mut moss_rocks: Vec<Inst> = Vec::new();
    for (x, z) in candidates(&mut rng, 760.0, 11.0, |x, z, r| {
        let s = shore_s(x, z);
        s > 2.0 && s < 160.0 && near_any(&focus, x, z, 300.0) && r.next() < 0.45 * density
    }) {
        moss_rocks.push(inst(x, z, rng.size(0.6, 3.5, 2.4), &mut rng));
    }
    put!("rock_moss_set_01", moss_rocks[..moss_rocks.len() / 2], 1024);
    put!("rock_moss_set_02", moss_rocks[moss_rocks.len() / 2..], 1024);

    // ── the forest, by distance from the shore ──
    let mut hero_pines: Vec<Inst> = Vec::new();
    let mut hero_firs: Vec<Inst> = Vec::new();
    for (x, z) in candidates(&mut rng, 900.0, 10.0, |x, z, r| {
        let s = shore_s(x, z);
        s > 9.0 && s < 160.0 && near_any(&focus, x, z, 110.0) && r.next() < (0.35 + 0.65 * stand(x, z)) * 0.8 * density
    }) {
        let mut t = inst(x, z, 0.0, &mut rng);
        t.tilt = [rng.range(-0.04, 0.04), rng.range(-0.04, 0.04)];
        if rng.next() < 0.3 {
            t.size = rng.size(17.0, 28.0, 1.3);
            hero_pines.push(t);
        } else {
            t.size = rng.size(18.0, 29.0, 1.3);
            hero_firs.push(t);
        }
    }
    // The scanned old-growth trees: millions of triangles each, so virtual
    // geometry carries them; each file holds three alternative trees.
    for (id, list) in [("pine_tree_01", &hero_pines), ("fir_tree_01", &hero_firs)] {
        let n = place(world, device, queue, root, id, list, 2048, vg, true)?;
        eprintln!("forest: {:>26} x{:<6} ({} objects)", id, list.len(), n);
        total += n;
    }

    let mut mid: [Vec<Inst>; 4] = Default::default();
    for (x, z) in candidates(&mut rng, 1500.0, 14.0, |x, z, r| {
        let s = shore_s(x, z);
        s > 12.0 && s < 380.0 && !near_any(&focus, x, z, 60.0) && r.next() < (0.3 + 0.7 * stand(x, z)) * 0.78 * density
    }) {
        let k = (rng.next() * 4.0) as usize % 4;
        let size = match k { 0 => rng.size(13.0, 24.0, 1.4), 1 => rng.size(14.0, 26.0, 1.4), 2 => rng.size(12.0, 22.0, 1.4), _ => rng.size(11.0, 19.0, 1.4) };
        let mut t = inst(x, z, size, &mut rng);
        t.tilt = [rng.range(-0.05, 0.05), rng.range(-0.05, 0.05)];
        mid[k].push(t);
    }
    let [m0, m1, m2, m3] = mid;
    put!("fir_sapling_medium", m0, 1024);
    put!("tree_small_02", m1, 1024);
    put!("fir_sapling", m2, 1024);
    put!("pine_sapling_small", m3, 1024);

    let mut far: [Vec<Inst>; 2] = Default::default();
    for (x, z) in candidates(&mut rng, 2900.0, 34.0, |x, z, r| {
        let s = shore_s(x, z);
        s > 380.0 && r.next() < (0.35 + 0.65 * stand(x, z)) * 0.8 * density
    }) {
        let k = (rng.next() * 2.0) as usize % 2;
        far[k].push(inst(x, z, rng.size(20.0, 34.0, 1.2), &mut rng));
    }
    let [f0, f1] = far;
    put!("fir_sapling", f0, 512);
    put!("pine_sapling_small", f1, 512);

    // Islands grow their own pines.
    let mut island_trees: Vec<Inst> = Vec::new();
    for &(cx, cz, r, _) in &ISLANDS {
        for _ in 0..((r / 6.0) as usize) {
            let a = rng.range(0.0, std::f32::consts::TAU);
            let d = rng.next().sqrt() * r * 0.8;
            let (x, z) = (cx + a.cos() * d, cz + a.sin() * d);
            if height(x, z) > 0.6 {
                island_trees.push(inst(x, z, rng.size(9.0, 20.0, 1.2), &mut rng));
            }
        }
    }
    put!("fir_sapling_medium", island_trees, 1024);

    // ── understory and ground cover, where the cameras can see it ──
    let ground_ok = |x: f32, z: f32| shore_s(x, z) > 3.0 && height(x, z) > 0.4;
    let mut ferns: Vec<Inst> = Vec::new();
    for (x, z) in candidates(&mut rng, 760.0, 2.0, |x, z, r| {
        ground_ok(x, z) && shore_s(x, z) < 130.0 && near_any(&focus, x, z, 70.0) && r.next() < (0.25 + 0.75 * stand(x, z)) * 0.7 * density
    }) {
        ferns.push(inst(x, z, rng.size(0.5, 1.4, 1.6), &mut rng));
    }
    put!("fern_02", ferns, 1024);
    let mut shrubs: [Vec<Inst>; 4] = Default::default();
    for (x, z) in candidates(&mut rng, 760.0, 6.5, |x, z, r| {
        ground_ok(x, z) && shore_s(x, z) < 160.0 && near_any(&focus, x, z, 120.0) && r.next() < 0.6 * density
    }) {
        let k = (rng.next() * 4.0) as usize % 4;
        shrubs[k].push(inst(x, z, rng.size(0.7, 2.4, 1.6), &mut rng));
    }
    let [s1, s2, s3, s4] = shrubs;
    put!("shrub_01", s1, 1024);
    put!("shrub_02", s2, 1024);
    put!("shrub_03", s3, 1024);
    put!("shrub_04", s4, 1024);
    let mut tufts: [Vec<Inst>; 3] = Default::default();
    for (x, z) in candidates(&mut rng, 760.0, 2.8, |x, z, r| {
        ground_ok(x, z) && shore_s(x, z) < 90.0 && near_any(&focus, x, z, 80.0) && r.next() < 0.4 * density
    }) {
        let k = (rng.next() * 3.0) as usize % 3;
        tufts[k].push(inst(x, z, rng.size(0.5, 1.3, 1.4), &mut rng));
    }
    let [g1, g2, g3] = tufts;
    put!("grass_medium_02", g1, 1024);
    put!("periwinkle_plant", g2, 1024);
    put!("celandine_01", g3, 1024);
    let mut roots: Vec<Inst> = Vec::new();
    let mut logs: Vec<Inst> = Vec::new();
    let mut stumps: Vec<Inst> = Vec::new();
    let mut sticks: Vec<Inst> = Vec::new();
    for (x, z) in candidates(&mut rng, 760.0, 9.0, |x, z, r| {
        ground_ok(x, z) && shore_s(x, z) < 140.0 && near_any(&focus, x, z, 100.0) && r.next() < 0.5 * density
    }) {
        match (rng.next() * 4.0) as usize % 4 {
            0 => roots.push(inst(x, z, rng.size(1.2, 3.2, 1.5), &mut rng)),
            1 => {
                let mut l = inst(x, z, rng.size(3.0, 9.0, 1.4), &mut rng);
                l.tilt = [0.0, rng.range(-0.1, 0.1)];
                logs.push(l);
            }
            2 => stumps.push(inst(x, z, rng.size(0.9, 2.4, 1.5), &mut rng)),
            _ => sticks.push(inst(x, z, rng.size(1.0, 3.0, 1.4), &mut rng)),
        }
    }
    put!("pine_roots", roots, 1024);
    put!("dead_tree_trunk", logs[..logs.len() / 2], 1024);
    put!("dead_tree_trunk_02", logs[logs.len() / 2..], 1024);
    put!("tree_stump_01", stumps[..stumps.len() / 2], 1024);
    put!("tree_stump_02", stumps[stumps.len() / 2..], 1024);
    put!("dry_branches_medium_01", sticks, 1024);

    // A dock on the south shore, where the first camera stands.
    let dock_at = shore_point(std::f32::consts::FRAC_PI_2 + 0.12, 2.0, 0.0);
    let mut dock = inst(dock_at.x, dock_at.z, 14.0, &mut rng);
    dock.yaw = yaw_to(dock_at, Vec3::ZERO);
    dock.lift = -0.3;
    put!("modular_wooden_pier", [dock], 1024);
    eprintln!("forest: {total} objects");
    Ok(())
}
