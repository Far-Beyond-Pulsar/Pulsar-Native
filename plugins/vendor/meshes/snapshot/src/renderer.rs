use std::path::Path;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use helio::{Camera, GpuLight, GpuMaterial, LightType, Renderer, RendererBuilder, RendererConfig};
use helio_asset_compat::{load_scene_file_with_config, ConvertedTextureRef, LoadConfig};
use helio_components::LightComponent;
use helio_components::{MaterialComponent};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};
use pulsar_scenedb::{Entity, SceneDb, World};
use thiserror::Error;

// ── Public types ──────────────────────────────────────────────────────────────

/// Which direction the camera looks at the model from.
#[derive(Debug, Clone, Copy, Default)]
pub enum ViewDirection {
    /// Slightly above and in front, rotated 45° — good general-purpose preview.
    #[default]
    Isometric,
    /// Straight from +Z (looking toward -Z).
    Front,
    /// Straight from -Z (looking toward +Z).
    Back,
    /// Straight from +X (looking toward -X).
    Right,
    /// Straight from -X (looking toward +X).
    Left,
    /// Straight from above +Y (looking toward -Y).
    Top,
    /// Straight from below -Y (looking toward +Y).
    Bottom,
}

/// Configuration for the snapshot.
pub struct SnapshotConfig {
    pub width: u32,
    pub height: u32,
    pub view: ViewDirection,
    /// Extra margin around the model (1.0 = exact fit, 1.2 = 20% breathing room).
    pub fit_margin: f32,
    /// Vertical field-of-view in degrees.
    pub fov_degrees: f32,
    /// Whether to flip the UV Y-axis when loading the model.
    pub flip_uv_y: bool,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            view: ViewDirection::Isometric,
            fit_margin: 1.2,
            fov_degrees: 45.0,
            flip_uv_y: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("asset loading failed: {0}")]
    Asset(#[from] helio_asset_compat::AssetError),

    #[error("no geometry found in model")]
    EmptyModel,

    #[error("wgpu adapter not found — no GPU available for headless rendering")]
    NoAdapter,

    #[error("wgpu device error: {0}")]
    Device(#[from] wgpu::RequestDeviceError),

    #[error("render error: {0}")]
    Render(String),

    #[error("readback buffer mapping failed: {0}")]
    Readback(#[from] wgpu::BufferAsyncError),
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Load `model_path`, render one snapshot frame, and return an RGBA image.
///
/// The camera is placed automatically so the whole model fits in frame.
/// No window or event loop is required.
pub fn render_snapshot<P: AsRef<Path>>(
    model_path: P,
    config: SnapshotConfig,
) -> Result<image::RgbaImage, SnapshotError> {
    pollster::block_on(render_snapshot_async(model_path, config))
}

/// Render a lightweight CPU mesh preview for editor thumbnails.
///
/// Unlike [`render_snapshot`], this does not initialize a second GPU device or
/// depend on the renderer's full scene pipeline. It loads the same converted
/// mesh data, projects the actual triangles, and applies a simple neutral
/// directional shade. This is useful for asset browsers, where the silhouette
/// matters more than imported material fidelity.
pub fn render_preview<P: AsRef<Path>>(
    model_path: P,
    config: SnapshotConfig,
) -> Result<image::RgbaImage, SnapshotError> {
    let load_config = LoadConfig::default()
        .with_uv_flip(config.flip_uv_y)
        .with_merge_meshes(false);
    let model_path = model_path.as_ref();
    let mut scene = load_scene_file_with_config(model_path, load_config)?;
    attach_sidecar_base_color(model_path, &mut scene);
    let (aabb_min, aabb_max) = compute_aabb(&scene)?;
    Ok(rasterize_preview(&scene, aabb_min, aabb_max, &config))
}

/// Some FBX exporters omit their external color-map connection while leaving
/// the conventional `*_BaseColor`, `*_Albedo`, or `*_Diffuse` image beside the
/// model. Use that map for preview thumbnails when the imported material has
/// no color texture at all.
fn attach_sidecar_base_color(model_path: &Path, scene: &mut helio_asset_compat::ConvertedScene) {
    if scene.materials.is_empty()
        || scene
            .materials
            .iter()
            .all(|material| material.textures.base_color.is_some())
    {
        return;
    }

    let Some(stem) = model_path.file_stem().and_then(|stem| stem.to_str()) else {
        set_preview_fallback_color(scene);
        return;
    };
    let Some(parent) = model_path.parent() else {
        set_preview_fallback_color(scene);
        return;
    };
    let prefix = format!("{stem}_").to_lowercase();
    let mut candidates = std::fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?.to_lowercase();
            let extension = path.extension()?.to_str()?.to_lowercase();
            if !file_name.starts_with(&prefix)
                || !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "tga")
            {
                return None;
            }
            let priority = if file_name.contains("basecolor") || file_name.contains("base_color") {
                0
            } else if file_name.contains("albedo") {
                1
            } else if file_name.contains("diffuse") {
                2
            } else {
                return None;
            };
            Some((priority, path))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let Some((_, texture_path)) = candidates.into_iter().next() else {
        set_preview_fallback_color(scene);
        return;
    };
    let Ok(decoded) = image::open(&texture_path) else {
        set_preview_fallback_color(scene);
        return;
    };
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let texture_index = scene.textures.len();
    scene.textures.push(helio::TextureUpload::rgba8(
        texture_path.to_string_lossy(),
        width,
        height,
        true,
        rgba.into_raw(),
        helio::TextureSamplerDesc::default(),
    ));
    let texture = ConvertedTextureRef {
        texture_index,
        uv_channel: 0,
        transform: Default::default(),
    };
    for material in &mut scene.materials {
        if material.textures.base_color.is_none() {
            material.textures.base_color = Some(texture);
        }
    }
}

fn set_preview_fallback_color(scene: &mut helio_asset_compat::ConvertedScene) {
    for material in &mut scene.materials {
        if material.textures.base_color.is_none() {
            material.gpu.base_color = [0.44, 0.66, 0.89, 1.0];
        }
    }
}

fn rasterize_preview(
    scene: &helio_asset_compat::ConvertedScene,
    aabb_min: Vec3,
    aabb_max: Vec3,
    cfg: &SnapshotConfig,
) -> image::RgbaImage {
    let width = cfg.width.max(1);
    let height = cfg.height.max(1);
    let center = (aabb_min + aabb_max) * 0.5;
    let (view_dir, up) = view_dir_and_up(cfg.view);
    let forward = view_dir.normalize_or_zero();
    let right = forward.cross(up).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();

    let mut projected = Vec::with_capacity(scene.meshes.len());
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for mesh in &scene.meshes {
        let vertices: Vec<(Vec2, f32, Vec3, Vec2, Vec2)> = mesh
            .vertices
            .iter()
            .map(|vertex| {
                let world = mesh
                    .node_transform
                    .transform_point3(Vec3::from_array(vertex.position));
                let relative = world - center;
                let point = Vec2::new(relative.dot(right), relative.dot(up));
                min_x = min_x.min(point.x);
                min_y = min_y.min(point.y);
                max_x = max_x.max(point.x);
                max_y = max_y.max(point.y);
                (
                    point,
                    -relative.dot(forward),
                    world,
                    Vec2::from_array(vertex.tex_coords0),
                    Vec2::from_array(vertex.tex_coords1),
                )
            })
            .collect();
        projected.push(vertices);
    }

    let mut image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    let mut depth = vec![f32::NEG_INFINITY; (width * height) as usize];
    if !min_x.is_finite() || !min_y.is_finite() || max_x <= min_x || max_y <= min_y {
        return image;
    }

    let projected_width = max_x - min_x;
    let projected_height = max_y - min_y;
    let scale =
        (width.min(height) as f32 * 0.78) / projected_width.max(projected_height).max(0.001);
    let offset = Vec2::new(
        (width as f32 - projected_width * scale) * 0.5 - min_x * scale,
        (height as f32 - projected_height * scale) * 0.5 + max_y * scale,
    );
    let light = Vec3::new(-0.35, 0.8, 0.55).normalize();

    for (mesh, vertices) in scene.meshes.iter().zip(projected.iter()) {
        let material = mesh
            .material_index
            .and_then(|index| scene.materials.get(index));
        let base_color = material
            .map(|material| material.gpu.base_color)
            .unwrap_or([0.44, 0.66, 0.89, 1.0]);
        let base_color_texture = material
            .and_then(|material| material.textures.base_color)
            .and_then(|texture| {
                scene
                    .textures
                    .get(texture.texture_index)
                    .map(|image| (image, texture))
            });
        for triangle in mesh.indices.chunks_exact(3) {
            let (Some(&ia), Some(&ib), Some(&ic)) = (
                triangle.first().and_then(|i| vertices.get(*i as usize)),
                triangle.get(1).and_then(|i| vertices.get(*i as usize)),
                triangle.get(2).and_then(|i| vertices.get(*i as usize)),
            ) else {
                continue;
            };
            // Projected Y is up while image Y is down. Negating it here keeps
            // imported meshes upright in the thumbnail.
            let to_screen =
                |point: Vec2| Vec2::new(point.x * scale + offset.x, offset.y - point.y * scale);
            let a = to_screen(ia.0);
            let b = to_screen(ib.0);
            let c = to_screen(ic.0);
            let area = edge(a, b, c);
            if area.abs() < 0.0001 {
                continue;
            }
            let normal = (ib.2 - ia.2).cross(ic.2 - ia.2).normalize_or_zero();
            let shade = 0.34 + 0.66 * normal.dot(light).abs();
            let min_px = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
            let max_px = a.x.max(b.x).max(c.x).ceil().min(width as f32 - 1.0) as u32;
            let min_py = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
            let max_py = a.y.max(b.y).max(c.y).ceil().min(height as f32 - 1.0) as u32;

            for y in min_py..=max_py {
                for x in min_px..=max_px {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let wa = edge(b, c, p) / area;
                    let wb = edge(c, a, p) / area;
                    let wc = edge(a, b, p) / area;
                    if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                        continue;
                    }
                    let z = wa * ia.1 + wb * ib.1 + wc * ic.1;
                    let index = (y * width + x) as usize;
                    if z > depth[index] {
                        depth[index] = z;
                        let texel = base_color_texture.map_or([1.0; 4], |(texture, reference)| {
                            let uv = if reference.uv_channel == 1 {
                                ia.4 * wa + ib.4 * wb + ic.4 * wc
                            } else {
                                ia.3 * wa + ib.3 * wb + ic.3 * wc
                            };
                            let scaled = Vec2::new(
                                uv.x * reference.transform.scale[0],
                                uv.y * reference.transform.scale[1],
                            );
                            let (sin, cos) = reference.transform.rotation_radians.sin_cos();
                            let transformed_uv = Vec2::new(
                                cos * scaled.x - sin * scaled.y + reference.transform.offset[0],
                                sin * scaled.x + cos * scaled.y + reference.transform.offset[1],
                            );
                            sample_texture_rgba(texture, transformed_uv)
                        });
                        let lit = shade;
                        let rgb = [
                            base_color[0] * texel[0] * lit,
                            base_color[1] * texel[1] * lit,
                            base_color[2] * texel[2] * lit,
                        ];
                        image.put_pixel(
                            x,
                            y,
                            image::Rgba([
                                linear_to_srgb(rgb[0]),
                                linear_to_srgb(rgb[1]),
                                linear_to_srgb(rgb[2]),
                                ((base_color[3] * texel[3]).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            ]),
                        );
                    }
                }
            }
        }
    }
    image
}

fn sample_texture_rgba(texture: &helio::TextureUpload, uv: Vec2) -> [f32; 4] {
    if texture.width == 0
        || texture.height == 0
        || !matches!(
            texture.format,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb
        )
        || texture.data.len() < (texture.width * texture.height * 4) as usize
    {
        return [1.0; 4];
    }

    let u = address_coordinate(uv.x, texture.sampler.address_mode_u);
    let v = address_coordinate(uv.y, texture.sampler.address_mode_v);
    let x = u * texture.width as f32 - 0.5;
    let y = v * texture.height as f32 - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x.floor();
    let ty = y - y.floor();
    let pixel = |px: i32, py: i32| {
        let px = address_index(px, texture.width, texture.sampler.address_mode_u);
        let py = address_index(py, texture.height, texture.sampler.address_mode_v);
        let offset = ((py * texture.width + px) * 4) as usize;
        let mut rgba = [0.0; 4];
        for (channel, value) in rgba.iter_mut().enumerate() {
            *value = texture.data[offset + channel] as f32 / 255.0;
        }
        if texture.format == wgpu::TextureFormat::Rgba8UnormSrgb {
            for value in &mut rgba[..3] {
                *value = srgb_to_linear(*value);
            }
        }
        rgba
    };
    let top_left = pixel(x0, y0);
    let top_right = pixel(x0 + 1, y0);
    let bottom_left = pixel(x0, y0 + 1);
    let bottom_right = pixel(x0 + 1, y0 + 1);
    std::array::from_fn(|channel| {
        let top = top_left[channel] * (1.0 - tx) + top_right[channel] * tx;
        let bottom = bottom_left[channel] * (1.0 - tx) + bottom_right[channel] * tx;
        top * (1.0 - ty) + bottom * ty
    })
}

fn address_coordinate(value: f32, mode: wgpu::AddressMode) -> f32 {
    match mode {
        wgpu::AddressMode::ClampToEdge => value.clamp(0.0, 1.0),
        wgpu::AddressMode::ClampToBorder => value.clamp(0.0, 1.0),
        wgpu::AddressMode::Repeat => value.rem_euclid(1.0),
        wgpu::AddressMode::MirrorRepeat => {
            let period = value.rem_euclid(2.0);
            if period <= 1.0 {
                period
            } else {
                2.0 - period
            }
        }
    }
}

fn address_index(index: i32, size: u32, mode: wgpu::AddressMode) -> u32 {
    let size = size as i32;
    match mode {
        wgpu::AddressMode::ClampToEdge => index.clamp(0, size - 1) as u32,
        wgpu::AddressMode::ClampToBorder => index.clamp(0, size - 1) as u32,
        wgpu::AddressMode::Repeat => index.rem_euclid(size) as u32,
        wgpu::AddressMode::MirrorRepeat => {
            let period = size * 2;
            let mirrored = index.rem_euclid(period);
            if mirrored < size {
                mirrored as u32
            } else {
                (period - 1 - mirrored) as u32
            }
        }
    }
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5) as u8
}

fn edge(a: Vec2, b: Vec2, point: Vec2) -> f32 {
    (point.x - a.x) * (b.y - a.y) - (point.y - a.y) * (b.x - a.x)
}

// ── Internals ─────────────────────────────────────────────────────────────────

async fn render_snapshot_async<P: AsRef<Path>>(
    model_path: P,
    cfg: SnapshotConfig,
) -> Result<image::RgbaImage, SnapshotError> {
    // ── 1. Load model ─────────────────────────────────────────────────────────
    let load_cfg = LoadConfig::default()
        .with_uv_flip(cfg.flip_uv_y)
        .with_merge_meshes(false);

    let scene = load_scene_file_with_config(model_path, load_cfg)?;

    // ── 2. Compute AABB over all mesh vertices ────────────────────────────────
    let (aabb_min, aabb_max) = compute_aabb(&scene)?;
    let center = (aabb_min + aabb_max) * 0.5;
    let half_extents = (aabb_max - aabb_min) * 0.5;
    let radius = half_extents.length().max(0.01);

    // ── 3. Initialise headless GPU ────────────────────────────────────────────
    let instance = wgpu::Instance::new(helio::wgpu_instance_descriptor());

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|_| SnapshotError::NoAdapter)?;

    let (device, queue): (wgpu::Device, wgpu::Queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("helio-snapshot"),
            required_features: helio::required_wgpu_features(adapter.features()),
            experimental_features: helio::required_experimental_features(adapter.features()),
            required_limits: helio::required_wgpu_limits(adapter.limits()),
            ..Default::default()
        })
        .await?;

    let device = Arc::new(device);
    let queue = Arc::new(queue);

    // ── 4. Create offscreen render target ─────────────────────────────────────
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot-target"),
        size: wgpu::Extent3d {
            width: cfg.width,
            height: cfg.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // ── 5. Build Helio renderer ───────────────────────────────────────────────
    // Use new_with_external_device so the graph uses deferred (non-blocking)
    // GPU timestamp readback — we drive polling ourselves after the frame.
    let renderer_cfg = RendererConfig::new(cfg.width, cfg.height, FORMAT).with_render_scale(1.0);
    let mut scene_db = new_scene_db(&device, &queue);
    let mut renderer = RendererBuilder::new(renderer_cfg, scene_db_handle(&scene_db))
        .with_external_device()
        .with_pass_build_context(Box::new(
            helio_mesh_host::presets::deferred,
        ))
        .build(device.clone(), queue.clone(), cfg.width, cfg.height, FORMAT);

    // ── 6. Upload all meshes + materials via helio-asset-compat ──────────────
    let (mesh_ids, material_ids) = upload_scene_rows(&mut scene_db.world, &scene);

    // ── 7. Insert a fallback material for meshes with no material ─────────────
    let fallback_mat = {
        let entity = scene_db.world.spawn();
        scene_db.world.insert(entity, fallback_material());
        entity
    };

    // ── 8. Place a renderable object for each uploaded mesh ───────────────────
    for (i, mesh) in scene.meshes.iter().enumerate() {
        let mesh_id = match mesh_ids.get(i) {
            Some(&id) => id,
            None => continue,
        };
        let material_id = mesh
            .material_index
            .and_then(|i| material_ids.get(i).copied())
            .unwrap_or(fallback_mat);
        let transform = mesh.node_transform;
        let world_center = transform.transform_point3(Vec3::ZERO);

        insert_object(
            &mut scene_db.world,
            mesh_id,
            material_id,
            transform,
            [world_center.x, world_center.y, world_center.z, radius],
            radius,
        )?;
    }

    // ── 9. Two-light rig: key (warm directional) + fill (cool fill) ───────────
    insert_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [-0.5_f32.sqrt(), -0.5_f32.sqrt(), 0.0, 0.0],
            color_intensity: [1.0, 0.98, 0.95, 3.0],
            shadow_index: 0,
            light_type: LightType::Directional as u32,
            inner_angle: 0.0,
            _pad: 0,
            god_rays_enabled: 0,
            god_rays_density: 1.0,
            god_rays_weight: 0.6,
            god_rays_decay: 1.0,
            god_rays_exposure: 0.7,
            flare_enabled: 0,
            flare_type: 0,
            flare_intensity: 0.0,
            flare_scale: 0.0,
            flare_tint_r: 0.0,
            flare_tint_g: 0.0,
            flare_tint_b: 0.0,
            ies_profile_index: -1,
            light_function_index: -1,
            ies_angle_scale: 0.0,
            ies_angle_offset: 0.0,
        },
    );
    insert_light(
        &mut scene_db.world,
        GpuLight {
            position_range: [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [0.5_f32.sqrt(), 0.5_f32.sqrt(), 0.0, 0.0],
            color_intensity: [0.5, 0.6, 0.8, 1.2],
            shadow_index: u32::MAX,
            light_type: LightType::Directional as u32,
            inner_angle: 0.0,
            _pad: 0,
            god_rays_enabled: 0,
            god_rays_density: 1.0,
            god_rays_weight: 0.6,
            god_rays_decay: 1.0,
            god_rays_exposure: 0.7,
            flare_enabled: 0,
            flare_type: 0,
            flare_intensity: 0.0,
            flare_scale: 0.0,
            flare_tint_r: 0.0,
            flare_tint_g: 0.0,
            flare_tint_b: 0.0,
            ies_profile_index: -1,
            light_function_index: -1,
            ies_angle_scale: 0.0,
            ies_angle_offset: 0.0,
        },
    );
    scene_db.world.flush_gpu_mirror(&queue);

    // ── 10. Auto-place camera to frame the bounding sphere ────────────────────
    let camera = build_camera(center, radius, &cfg);

    // ── 11. Render one frame ──────────────────────────────────────────────────
    renderer
        .render(&camera, &target_view)
        .map_err(|e| SnapshotError::Render(e.to_string()))?;

    // Flush all submitted GPU work before we copy the texture to the staging buffer.
    // Because we used new_with_external_device the graph never blocks internally —
    // this single poll is the only synchronisation point we need.
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    // ── 12. Read pixels back to CPU ───────────────────────────────────────────
    readback_rgba(&device, &queue, &target_texture, cfg.width, cfg.height).await
}

// ── SceneDB authoring ─────────────────────────────────────────────────────────

fn new_scene_db(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> SceneDb {
    let mut scene_db = SceneDb::new();
    let ctx = EngineGpuContext::new(device.clone(), queue.clone());
    let config = SceneGpuConfig {
        classes: Vec::new(),
        tombstone_headroom: 0,
        max_cells_metadata: 0,
    };
    let mut store = SceneGpuStore::new(&ctx, config);
    MeshComponent::register_gpu_columns_growable(&mut store, 4096, device);
    MaterialComponent::register_gpu_columns_growable(&mut store, 4096, device);
    StaticObjectComponent::register_gpu_columns_growable(&mut store, 4096, device);
    LightComponent::register_gpu_columns_growable(&mut store, 256, device);
    let mirror = GpuMirrorHandle::new(Arc::new(store), queue.clone());
    scene_db.world.attach_gpu_mirror(mirror);
    scene_db
}

fn upload_scene_rows(
    world: &mut World,
    scene: &helio_asset_compat::ConvertedScene,
) -> (Vec<Entity>, Vec<Entity>) {
    let material_ids = helio_mesh_host::upload_scene_materials(world, scene);
    let mesh_ids = scene
        .meshes
        .iter()
        .map(|mesh| {
            let entity = world.spawn();
            world.insert(
                entity,
                MeshComponent {
                    vertices: mesh.vertices.clone(),
                    indices: mesh.indices.clone(),
                },
            );
            entity
        })
        .collect();
    (mesh_ids, material_ids)
}

fn fallback_material() -> MaterialComponent {
    MaterialComponent::from(helio_mats::MaterialRow::from(GpuMaterial {
        base_color: [0.7, 0.65, 0.55, 1.0],
        emissive: [0.0; 4],
        roughness_metallic: [0.6, 0.0, 1.5, 0.0],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }))
}

fn insert_object(
    world: &mut World,
    mesh: Entity,
    material: Entity,
    transform: glam::Mat4,
    bounds: [f32; 4],
    _radius: f32,
) -> Result<Entity, SnapshotError> {
    let mirror = world
        .gpu_mirror()
        .cloned()
        .ok_or_else(|| SnapshotError::Render("SceneDB GPU mirror is missing".into()))?;
    let vertices = MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index())
        .filter(|handle| handle.count != 0)
        .ok_or_else(|| SnapshotError::Render("mesh has no GPU vertex range".into()))?;
    let indices = MeshComponent::indices_gpu_handle(mirror.store(), mesh.index())
        .filter(|handle| handle.count != 0)
        .ok_or_else(|| SnapshotError::Render("mesh has no GPU index range".into()))?;
    let material_component = world
        .get::<MaterialComponent>(material)
        .ok_or_else(|| SnapshotError::Render("material entity is missing".into()))?;
    let object = StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
        mesh.index(),
        mesh.generation().wrapping_add(1),
        material.index(),
        material.generation().wrapping_add(1),
        transform,
        bounds,
        indices.count,
        indices.offset,
        vertices.offset as i32,
        material_component.material_class,
        0,
        3,
    ));
    let entity = world.spawn();
    world.insert(entity, object);
    Ok(entity)
}

fn insert_light(world: &mut World, light: GpuLight) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LightComponent::from(helio_pass_light_cull::LightRow::from(light)),
    );
    entity
}

fn scene_db_handle(scene_db: &SceneDb) -> helio::SceneDbHandle {
    scene_db
        .world
        .gpu_mirror()
        .cloned()
        .expect("SceneDB GPU mirror is attached during initialization")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn compute_aabb(scene: &helio_asset_compat::ConvertedScene) -> Result<(Vec3, Vec3), SnapshotError> {
    let mut aabb_min = Vec3::splat(f32::MAX);
    let mut aabb_max = Vec3::splat(f32::MIN);

    for mesh in &scene.meshes {
        let t = mesh.node_transform;
        for v in &mesh.vertices {
            let world = t.transform_point3(Vec3::from(v.position));
            aabb_min = aabb_min.min(world);
            aabb_max = aabb_max.max(world);
        }
    }

    if aabb_min.x > aabb_max.x {
        return Err(SnapshotError::EmptyModel);
    }
    Ok((aabb_min, aabb_max))
}

fn build_camera(center: Vec3, radius: f32, cfg: &SnapshotConfig) -> Camera {
    let fov = cfg.fov_degrees.to_radians();
    let aspect = cfg.width as f32 / cfg.height as f32;

    // Distance so the bounding sphere fills the FOV, with the requested margin.
    let distance = (radius / (fov * 0.5).tan()) * cfg.fit_margin;

    let (view_dir, up) = view_dir_and_up(cfg.view);
    let eye = center - view_dir * distance;

    let near = (distance - radius * 1.05).max(0.01);
    let far = distance + radius * 2.0;

    Camera::perspective_look_at(eye, center, up, fov, aspect, near, far)
}

fn view_dir_and_up(dir: ViewDirection) -> (Vec3, Vec3) {
    match dir {
        ViewDirection::Isometric => (Vec3::new(1.0, 0.8, 1.0).normalize(), Vec3::Y),
        ViewDirection::Front => (Vec3::Z, Vec3::Y),
        ViewDirection::Back => (-Vec3::Z, Vec3::Y),
        ViewDirection::Right => (Vec3::X, Vec3::Y),
        ViewDirection::Left => (-Vec3::X, Vec3::Y),
        ViewDirection::Top => (Vec3::Y, Vec3::Z),
        ViewDirection::Bottom => (-Vec3::Y, -Vec3::Z),
    }
}

async fn readback_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<image::RgbaImage, SnapshotError> {
    // Row stride must be aligned to 256 bytes per wgpu spec.
    let bytes_per_row = align_up(width * 4, 256);

    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("snapshot-staging"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("snapshot-readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    // Map and wait.
    let slice = staging.slice(..);
    let (tx, rx) = futures_channel::oneshot::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.await.unwrap()?;

    // Strip the 256-byte row padding before building the image.
    let data = slice
        .get_mapped_range()
        .map_err(|_| SnapshotError::Render("readback buffer mapping failed".into()))?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        pixels.extend_from_slice(&data[start..end]);
    }
    drop(data);
    staging.unmap();

    image::RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| SnapshotError::Render("image buffer size mismatch".into()))
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

/// Returns (key, fill, rim) directional light travel vectors derived from the
/// camera's own axes so the rig always illuminates the visible faces regardless
/// of ViewDirection.
fn camera_light_rig(camera: &Camera, target: Vec3) -> (Vec3, Vec3, Vec3) {
    let forward = (target - camera.eye.as_vec3()).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward).normalize();

    // Key: slightly right + elevated, in the forward hemisphere
    let key_dir = (forward + right * 0.45 + up * 0.55).normalize();
    // Fill: mirrored left, lower elevation
    let fill_dir = (forward - right * 0.55 + up * 0.20).normalize();
    // Rim: from behind, adds depth separation
    let rim_dir = (-forward + up * 0.30).normalize();

    (key_dir, fill_dir, rim_dir)
}

// ── SnapshotBatch ─────────────────────────────────────────────────────────────

/// A persistent batch renderer — GPU and Helio are initialised once, then
/// [`render`](SnapshotBatch::render) can be called for thousands of models.
///
/// SceneDB rows for each model are despawned after readback, so there is no
/// authored-scene growth across models.
///
/// # Example
/// ```no_run
/// use helio_snapshot::{SnapshotBatch, SnapshotConfig};
///
/// let mut batch = SnapshotBatch::new(SnapshotConfig::default()).unwrap();
/// for path in &["a.fbx", "b.fbx", "c.fbx"] {
///     let img = batch.render(path).unwrap();
///     img.save(format!("{path}.png")).unwrap();
/// }
/// ```
pub struct SnapshotBatch {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene_db: SceneDb,
    renderer: Renderer,
    target_texture: wgpu::Texture,
    target_view: wgpu::TextureView,
    config: SnapshotConfig,
    live_entities: Vec<Entity>,
}

impl SnapshotBatch {
    /// Initialise GPU and Helio once.  All subsequent [`render`](Self::render)
    /// calls share this device/queue/renderer.
    pub fn new(config: SnapshotConfig) -> Result<Self, SnapshotError> {
        pollster::block_on(Self::new_async(config))
    }

    async fn new_async(config: SnapshotConfig) -> Result<Self, SnapshotError> {
        let instance = wgpu::Instance::new(helio::wgpu_instance_descriptor());

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|_| SnapshotError::NoAdapter)?;

        let (device, queue): (wgpu::Device, wgpu::Queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("helio-snapshot-batch"),
                required_features: helio::required_wgpu_features(adapter.features()),
                experimental_features: helio::required_experimental_features(adapter.features()),
                required_limits: helio::required_wgpu_limits(adapter.limits()),
                ..Default::default()
            })
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

        let target_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("batch-snapshot-target"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let renderer_cfg =
            RendererConfig::new(config.width, config.height, FORMAT).with_render_scale(1.0);
        let scene_db = new_scene_db(&device, &queue);
        let renderer = RendererBuilder::new(renderer_cfg, scene_db_handle(&scene_db))
            .with_external_device()
            .with_pass_build_context(Box::new(
                helio_mesh_host::presets::deferred,
            ))
            .build(
                device.clone(),
                queue.clone(),
                config.width,
                config.height,
                FORMAT,
            );

        Ok(Self {
            device,
            queue,
            scene_db,
            renderer,
            target_texture,
            target_view,
            config,
            live_entities: Vec::new(),
        })
    }

    /// Render `model_path` and return an RGBA image.
    ///
    /// The scene is wiped after each call, so this can be called in a tight
    /// loop without any GPU memory growth.
    pub fn render<P: AsRef<Path>>(
        &mut self,
        model_path: P,
    ) -> Result<image::RgbaImage, SnapshotError> {
        pollster::block_on(self.render_async(model_path))
    }

    async fn render_async<P: AsRef<Path>>(
        &mut self,
        model_path: P,
    ) -> Result<image::RgbaImage, SnapshotError> {
        // ── Load + upload ─────────────────────────────────────────────────────
        let load_cfg = LoadConfig::default()
            .with_uv_flip(self.config.flip_uv_y)
            .with_merge_meshes(false);

        let scene = load_scene_file_with_config(model_path, load_cfg)?;

        let (aabb_min, aabb_max) = compute_aabb(&scene)?;
        let center = (aabb_min + aabb_max) * 0.5;
        let radius = ((aabb_max - aabb_min) * 0.5).length().max(0.01);
        let camera = build_camera(center, radius, &self.config);

        let (mesh_ids, material_ids) = upload_scene_rows(&mut self.scene_db.world, &scene);
        self.live_entities
            .extend(mesh_ids.iter().chain(material_ids.iter()).copied());
        let fallback_mat = self.scene_db.world.spawn();
        self.scene_db
            .world
            .insert(fallback_mat, fallback_material());
        self.live_entities.push(fallback_mat);

        for (i, mesh) in scene.meshes.iter().enumerate() {
            let Some(&mesh_id) = mesh_ids.get(i) else {
                continue;
            };
            let material_id = mesh
                .material_index
                .and_then(|i| material_ids.get(i).copied())
                .unwrap_or(fallback_mat);
            let transform = mesh.node_transform;
            let world_center = transform.transform_point3(Vec3::ZERO);

            let object = insert_object(
                &mut self.scene_db.world,
                mesh_id,
                material_id,
                transform,
                [world_center.x, world_center.y, world_center.z, radius],
                radius,
            )?;
            self.live_entities.push(object);
        }

        // ── Camera-relative three-point light rig ─────────────────────────────
        let (key_dir, fill_dir, rim_dir) = camera_light_rig(&camera, center);
        for (dir, color, intensity, shadow) in [
            (key_dir, [1.00_f32, 0.97, 0.92], 3.5_f32, 0_u32),
            (fill_dir, [0.55, 0.65, 0.85], 1.2, u32::MAX),
            (rim_dir, [0.90, 0.95, 1.00], 0.8, u32::MAX),
        ] {
            let light = self.scene_db.world.spawn();
            self.scene_db.world.insert(
                light,
                LightComponent::from(helio_pass_light_cull::LightRow::from(GpuLight {
                    position_range: [0.0, 0.0, 0.0, f32::MAX],
                    direction_outer: [dir.x, dir.y, dir.z, 0.0],
                    color_intensity: [color[0], color[1], color[2], intensity],
                    shadow_index: shadow,
                    light_type: LightType::Directional as u32,
                    inner_angle: 0.0,
                    _pad: 0,
                    god_rays_enabled: 0,
                    god_rays_density: 1.0,
                    god_rays_weight: 0.6,
                    god_rays_decay: 1.0,
                    god_rays_exposure: 0.7,
                    flare_enabled: 0,
                    flare_type: 0,
                    flare_intensity: 0.0,
                    flare_scale: 0.0,
                    flare_tint_r: 0.0,
                    flare_tint_g: 0.0,
                    flare_tint_b: 0.0,
                    ies_profile_index: -1,
                    light_function_index: -1,
                    ies_angle_scale: 0.0,
                    ies_angle_offset: 0.0,
                })),
            );
            self.live_entities.push(light);
        }

        self.scene_db.world.flush_gpu_mirror(&self.queue);

        // ── Render ────────────────────────────────────────────────────────────
        self.renderer
            .render(&camera, &self.target_view)
            .map_err(|e| SnapshotError::Render(e.to_string()))?;

        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        // ── Readback ──────────────────────────────────────────────────────────
        let img = readback_rgba(
            &self.device,
            &self.queue,
            &self.target_texture,
            self.config.width,
            self.config.height,
        )
        .await?;

        // Remove the per-model rows from the authoritative SceneDB world.
        // Recreate the world/mirror on the next render so all GPU pools and
        // entity generations remain consistent without a renderer-side clear.
        for entity in self.live_entities.drain(..) {
            self.scene_db.world.despawn(entity);
        }
        self.scene_db.world.flush_gpu_mirror(&self.queue);

        Ok(img)
    }
}
