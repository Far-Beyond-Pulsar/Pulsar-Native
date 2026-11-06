//! Deterministic coastal dressing, in metres. The beach and shore bake share height().
//!
//! Every object is a photogrammetry scan placed through SceneDB. Materials keep
//! their authored textures: colour as sRGB, normals flagged and mip-filtered
//! for specular anti-aliasing, roughness and occlusion as data.
use crate::common::{spawn_mesh, spawn_object};
use glam::{Mat4, Quat, Vec3};
use helio_mats::texture_mips::{mip_chain, TexelKind};
use pulsar_scenedb::World;
use std::path::Path;

fn upload_materials(
    world: &mut World,
    scene: &helio_asset_compat::ConvertedScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cap: u32,
) -> Result<Vec<pulsar_scenedb::Entity>, String> {
    let mirror = world.gpu_mirror().cloned().ok_or("no GPU mirror")?;
    let store = mirror.texture_store().ok_or("no texture store")?;

    // How a texture is sampled decides how its mips are filtered.
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
        // Scans ship 2k-4k maps; past `cap` they are halved so ~190 materials
        // fit in VRAM.
        let (mut width, mut height, mut data) = (texture.width, texture.height, texture.data.clone());
        while width.max(height) > cap && width > 1 && height > 1 {
            let img = image::RgbaImage::from_raw(width, height, data).ok_or("invalid RGBA texture")?;
            width /= 2;
            height /= 2;
            data = image::imageops::resize(&img, width, height, image::imageops::FilterType::Triangle).into_raw();
        }
        let chain = mip_chain(&data, width, height, kind);
        let refs: Vec<&[u8]> = chain.iter().map(|l| l.texels.as_slice()).collect();
        let descriptor = wgpu::TextureDescriptor {
            label: texture.label.as_deref(),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: refs.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if kind.is_srgb() {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        slots.push(
            store
                .write()
                .map_err(|e| e.to_string())?
                .register_mips(device, queue, &descriptor, &refs)
                .map_err(|e| format!("{e:?}"))?,
        );
    }

    let mut ids = Vec::new();
    for material in &scene.materials {
        let mut row =
            helio_mats::host_rows::MaterialComponent::from(helio_mats::MaterialRow::from(material.gpu));
        let t = &material.textures;
        if let Some(r) = t.base_color {
            row.tex_base_color = slots[r.texture_index];
        }
        if let Some(r) = t.normal {
            row.tex_normal = slots[r.texture_index];
            row.flags |= helio_mats::FLAG_HAS_NORMAL_MAP;
        }
        if let Some(r) = t.roughness_metallic {
            row.tex_roughness = slots[r.texture_index];
        }
        if let Some(r) = t.occlusion {
            row.tex_occlusion = slots[r.texture_index];
        }
        if let Some(r) = t.emissive {
            row.tex_emissive = slots[r.texture_index];
        }
        let id = world.spawn();
        world.insert(id, row);
        ids.push(id);
    }
    Ok(ids)
}

pub fn height(x: f32, z: f32) -> f32 {
    0.055 * z + 0.15 * (0.18 * x).sin() * (0.15 * z).sin() + 0.8 * (0.025 * x).sin()
}

pub struct Beach;
impl helio_pass_water_sim::shoreline::WaterTerrainProvider for Beach {
    fn elevation_y(&self, x: f64, z: f64) -> Option<f64> {
        Some(height(x as f32, z as f32) as f64)
    }
}

/// (x, z, width, yaw): width is the instance's largest dimension in metres.
type Instance = (f32, f32, f32, f32);

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / 16777216.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
    /// Roughly normal, sd 1 (sum of uniforms).
    fn gauss(&mut self) -> f32 {
        (0..4).map(|_| self.next()).sum::<f32>() - 2.0
    }
}

/// Clustered scatter: `count` instances around `centres`, `spread` metres wide,
/// with `loose` of them dropped uniformly across `area` (x0, x1, z0, z1).
fn scatter(
    rng: &mut Rng,
    count: usize,
    centres: &[(f32, f32)],
    spread: f32,
    loose: f32,
    area: (f32, f32, f32, f32),
    size: (f32, f32),
) -> Vec<Instance> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (x, z) = if rng.next() < loose {
            (rng.range(area.0, area.1), rng.range(area.2, area.3))
        } else {
            let c = centres[(rng.next() * centres.len() as f32) as usize % centres.len()];
            (c.0 + rng.gauss() * spread, c.1 + rng.gauss() * spread * 0.7)
        };
        // Small ones are far more common than large ones.
        let t = rng.next().powf(2.4);
        out.push((x, z, size.0 + t * (size.1 - size.0), rng.next() * std::f32::consts::TAU));
    }
    out
}

pub fn build(
    world: &mut World,
    root: &Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(), String> {
    let sand = helio_asset_compat::load_scene_file_with_config(
        &root.join("coast_sand_02/coast_sand_02_4k.gltf"),
        Default::default(),
    )
    .map_err(|e| e.to_string())?;
    let materials = upload_materials(world, &sand, device, queue, 2048)?;
    let material = *materials.first().ok_or("sand material missing")?;
    // The sand texture tiles every `TILE` metres. Each tile owns its vertices
    // with UVs in 0..1, so the mapping is the same under any sampler wrap mode
    // and no UV loses precision far from the origin.
    const TILE: usize = 2;
    let tiles = 256 / TILE; // 512 m square, 1 m quads
    let per = TILE + 1;
    let mut vertices = Vec::with_capacity(tiles * tiles * per * per);
    let mut indices = Vec::with_capacity(tiles * tiles * TILE * TILE * 6);
    for tj in 0..tiles {
        for ti in 0..tiles {
            let base = vertices.len() as u32;
            for j in 0..per {
                for i in 0..per {
                    let x = (ti * TILE + i) as f32 * 2.0 - 256.0;
                    let z = (tj * TILE + j) as f32 * 2.0 - 256.0;
                    let dx = (height(x + 0.1, z) - height(x - 0.1, z)) / 0.2;
                    let dz = (height(x, z + 0.1) - height(x, z - 0.1)) / 0.2;
                    vertices.push(helio_mesh_api::PackedVertex::from_components(
                        [x, height(x, z), z],
                        Vec3::new(-dx, 1.0, -dz).normalize().to_array(),
                        [i as f32 / TILE as f32, j as f32 / TILE as f32],
                        Vec3::new(1.0, dx, 0.0).normalize().to_array(),
                        1.0,
                    ));
                }
            }
            for j in 0..TILE {
                for i in 0..TILE {
                    let a = base + (j * per + i) as u32;
                    let b = a + per as u32;
                    indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
                }
            }
        }
    }
    let mesh = spawn_mesh(world, helio_mesh_api::MeshUpload { vertices, indices });
    spawn_object(world, mesh, material, Mat4::IDENTITY, 400.0).map_err(str::to_owned)?;

    let mut rng = Rng(1871);
    let area = (-60.0, 60.0, -6.0, 46.0);
    let back = (-75.0, 75.0, 24.0, 62.0);

    // Boulder fields drift along the tide lines; pebbles gather around them.
    let boulder_fields = [
        (-24.0, 6.0), (6.0, -2.0), (30.0, 14.0), (-8.0, 26.0), (44.0, -6.0),
        (-9.0, 11.0), (4.0, 8.0), (-1.0, 14.0), (12.0, 16.0), (-18.0, 17.0),
    ];
    let wrack_line = [(-30.0, 12.0), (-10.0, 14.0), (10.0, 11.0), (28.0, 13.0), (0.0, 18.0), (-5.0, 12.0)];
    // Dune grass and shrubs grow in drifts behind the beach.
    let dunes = [(-50.0, 34.0), (-22.0, 40.0), (8.0, 36.0), (34.0, 44.0), (58.0, 32.0), (-4.0, 52.0)];
    let uniform = |rng: &mut Rng, n: usize, a: (f32, f32, f32, f32), s: (f32, f32)| {
        scatter(rng, n, &[(0.0, 0.0)], 1.0, 1.0, a, s)
    };

    // The Poly Haven kit is optional: a missing download is skipped, not fatal.
    let ph = |id: &str| -> std::path::PathBuf {
        let dir = root.join("ph").join(id);
        std::fs::read_dir(&dir)
            .ok()
            .and_then(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|x| x == "gltf"))
            })
            .unwrap_or(dir.join("missing.gltf"))
    };
    let put = |world: &mut World, path: std::path::PathBuf, instances: &[Instance], cap: u32, rest: Option<f32>| -> Result<(), String> {
        if !path.exists() {
            eprintln!("coast kit: {} missing, skipped", path.display());
            return Ok(());
        }
        if let Err(e) = place(world, device, queue, &path, instances, cap, rest) {
            eprintln!("coast kit: {e}, skipped");
        }
        Ok(())
    };

    // Cliffs close the cove.
    put(world, ph("coastal_cliff_01"), &[(-95.0, 28.0, 80.0, 1.2), (-60.0, 100.0, 95.0, 0.3), (135.0, 70.0, 90.0, 4.1)], 2048, None)?;
    put(world, ph("coastal_cliff_02"), &[(100.0, 16.0, 85.0, -1.0), (25.0, 115.0, 120.0, 3.4), (-130.0, 75.0, 100.0, 0.9)], 2048, None)?;
    put(world, ph("coastal_cliff_04"), &[(-150.0, 20.0, 90.0, 1.7), (170.0, 20.0, 80.0, 4.6)], 2048, None)?;

    // Sea stacks, headlands and shoreline ledges.
    put(world, ph("coast_land_rocks_02"), &[(-46.0, -4.0, 24.0, 0.4), (60.0, -12.0, 28.0, 2.2)], 2048, None)?;
    put(world, ph("coast_land_rocks_03"), &[(-72.0, -30.0, 32.0, 3.0), (24.0, -26.0, 22.0, 1.1)], 2048, None)?;
    put(world, ph("coast_land_rocks_04"), &[(0.0, -46.0, 30.0, 5.0), (-22.0, -30.0, 18.0, 2.4)], 2048, None)?;
    put(world, ph("coast_line_01"), &[(-35.0, -1.0, 42.0, 0.2), (70.0, 8.0, 38.0, 2.9)], 2048, None)?;
    put(world, ph("coast_line_02"), &[(40.0, 2.0, 36.0, 3.2), (-78.0, 14.0, 36.0, 0.5)], 2048, None)?;
    place(
        world, device, queue, &root.join("megascans_nordic_beach_high.glb"),
        &[
            (-13.0, 1.0, 11.0, 0.3), (18.0, -9.0, 13.0, 2.0), (35.0, -25.0, 18.0, 0.8),
            (-34.0, -6.0, 9.0, 3.9), (52.0, 6.0, 8.0, 1.4), (-52.0, 20.0, 12.0, 5.2),
            (4.0, -30.0, 15.0, 4.4),
        ],
        2048, None,
    )?;
    place(
        world, device, queue, &root.join("coast_rocks_01/coast_rocks_01_4k.gltf"),
        &[(-38.0, -15.0, 34.0, 0.2), (48.0, -38.0, 40.0, 2.1), (-4.0, -52.0, 46.0, 1.1)],
        2048, None,
    )?;

    // Boulders and stones across the beach, four rock scans for variety.
    for (k, id) in ["coast_rocks_02", "coast_rocks_03", "coast_rocks_05", "boulder_01"].iter().enumerate() {
        let items = scatter(&mut rng, 26, &boulder_fields, 7.0 + k as f32, 0.2, area, (0.5, 3.4));
        put(world, ph(id), &items, 1024, None)?;
    }
    let mid_rocks = scatter(&mut rng, 70, &boulder_fields, 7.0, 0.2, area, (0.45, 3.0));
    place(world, device, queue, &root.join("coast_rocks_01/coast_rocks_01_4k.gltf"), &mid_rocks, 1024, None)?;
    let stones = scatter(&mut rng, 240, &boulder_fields, 6.0, 0.2, area, (0.35, 2.4));
    place(world, device, queue, &root.join("sand_rocks_small_01/sand_rocks_small_01_4k.gltf"), &stones, 1024, None)?;
    let shells = scatter(&mut rng, 320, &wrack_line, 5.0, 0.2, (-40.0, 40.0, 6.0, 34.0), (0.14, 0.34));
    place(world, device, queue, &root.join("lambis_shell/lambis_shell_4k.gltf"), &shells, 1024, None)?;

    // Wrack line: driftwood, branches, bark.
    let l = scatter(&mut rng, 7, &wrack_line, 9.0, 0.1, area, (2.6, 6.0));
    put(world, ph("dead_tree_trunk"), &l, 1024, None)?;
    let l = scatter(&mut rng, 6, &wrack_line, 9.0, 0.1, area, (2.4, 5.0));
    put(world, ph("dead_tree_trunk_02"), &l, 1024, None)?;
    let l = scatter(&mut rng, 24, &wrack_line, 8.0, 0.2, area, (0.9, 2.4));
    put(world, ph("dry_branches_medium_01"), &l, 1024, None)?;
    let l = scatter(&mut rng, 30, &wrack_line, 8.0, 0.2, area, (0.5, 1.4));
    put(world, ph("bark_debris_01"), &l, 1024, None)?;

    // Dune planting.
    let l = scatter(&mut rng, 150, &dunes, 9.0, 0.15, back, (0.7, 1.7));
    put(world, ph("grass_bermuda_01"), &l, 1024, None)?;
    let l = scatter(&mut rng, 42, &dunes, 9.0, 0.1, back, (0.8, 1.5));
    put(world, ph("fern_02"), &l, 1024, None)?;
    let l = scatter(&mut rng, 42, &dunes, 8.0, 0.1, back, (1.0, 2.2));
    put(world, ph("shrub_sorrel_01"), &l, 1024, None)?;
    let l = uniform(&mut rng, 10, (-70.0, 70.0, 40.0, 70.0), (3.5, 5.5));
    put(world, ph("pachira_aquatica_01"), &l, 1024, None)?;
    for (i, id) in ["island_tree_01", "island_tree_02", "island_tree_03"].iter().enumerate() {
        let l = uniform(&mut rng, 4, (-80.0 + i as f32 * 8.0, 80.0, 48.0, 90.0), (8.0, 13.0));
        put(world, ph(id), &l, 1024, None)?;
    }

    // Props: a beached cache, a pier into the surf, a buoy and a ship offshore.
    put(world, ph("lifebuoy"), &[(3.0, 16.0, 0.9, 0.4), (-14.0, 13.0, 0.9, 2.2)], 1024, None)?;
    put(world, ph("wooden_barrels_01"), &[(-20.0, 20.0, 1.2, 0.5), (14.0, 9.0, 1.1, 2.0)], 1024, None)?;
    put(world, ph("wooden_crate_01"), &[(8.0, 20.0, 1.0, 0.2), (10.0, 21.5, 0.9, 0.9)], 1024, None)?;
    put(world, ph("wooden_crate_02"), &[(-4.0, 19.0, 0.9, 1.4)], 1024, None)?;
    put(world, ph("treasure_chest"), &[(-7.0, 15.0, 0.8, 0.7)], 1024, None)?;
    put(world, ph("modular_wooden_pier"), &[(22.0, -8.0, 18.0, std::f32::consts::FRAC_PI_2)], 1024, None)?;
    put(world, ph("lateral_sea_marker"), &[(-30.0, -45.0, 5.0, 0.0)], 1024, Some(-0.4))?;
    put(world, ph("ship_pinnace"), &[(-45.0, -105.0, 36.0, 0.7)], 1024, Some(-3.4))?;
    Ok(())
}

/// Instances `path` (a scan) at each (x, z, largest dimension, yaw). Terrain
/// height places it unless `rest` fixes the y it sits at (things afloat).
fn place(
    world: &mut World,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
    instances: &[Instance],
    cap: u32,
    rest: Option<f32>,
) -> Result<(), String> {
    let scene = helio_asset_compat::load_scene_file_with_config(path, Default::default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let materials = upload_materials(world, &scene, device, queue, cap)?;
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for mesh in &scene.meshes {
        for v in &mesh.vertices {
            let p = mesh.node_transform.transform_point3(Vec3::from_array(v.position));
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let size = (hi - lo).max_element().max(0.001);
    let base = Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5);
    for source in &scene.meshes {
        let mesh = spawn_mesh(
            world,
            helio_mesh_api::MeshUpload {
                vertices: source.vertices.clone(),
                indices: source.indices.clone(),
            },
        );
        let material = *materials
            .get(source.material_index.unwrap_or(0))
            .ok_or("scan material missing")?;
        for &(x, z, width, yaw) in instances {
            // Sunk slightly into the sand so nothing floats on the slope.
            let y = rest.unwrap_or_else(|| height(x, z) - width * 0.035);
            let transform = Mat4::from_scale_rotation_translation(
                Vec3::splat(width / size),
                Quat::from_rotation_y(yaw),
                Vec3::new(x, y, z),
            ) * Mat4::from_translation(-base)
                * source.node_transform;
            spawn_object(world, mesh, material, transform, width * 2.0).map_err(str::to_owned)?;
        }
    }
    Ok(())
}
