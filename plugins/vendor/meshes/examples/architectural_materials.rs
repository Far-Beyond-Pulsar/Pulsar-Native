//! Shared, embedded CC0 architectural materials for interactive and capture paths.
use helio_mats::host_rows::MaterialComponent;
use helio_mats::texture_mips::{mip_chain, TexelKind};
use pulsar_scenedb::{gpu::TextureStore, Entity, World};
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy)]
pub struct StoneMaterial;
#[derive(Clone, Copy)]
pub struct CarvedStoneMaterial;
#[derive(Clone, Copy)]
pub struct FloorMaterial;
#[derive(Clone, Copy)]
pub struct WoodMaterial;

pub fn configure_sampler(renderer: &mut helio::Renderer) {
    renderer.set_feature(helio_mats::MaterialSampler(wgpu::SamplerDescriptor {
        label: Some("Architectural repeat trilinear sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    }));
}

pub fn load(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &mut World,
) -> Option<Arc<RwLock<TextureStore>>> {
    if std::env::var_os("HLFS_NO_STONE_TEXTURES").is_some()
        || std::env::var_os("HLFS_TEXTURE_CHECKER").is_some()
    {
        return None;
    }
    let stone: Vec<_> = world
        .query::<(&StoneMaterial,)>()
        .map(|(entity, _)| entity)
        .collect();
    let floor: Vec<_> = world
        .query::<(&FloorMaterial,)>()
        .map(|(entity, _)| entity)
        .collect();
    let carved_stone: Vec<_> = world
        .query::<(&CarvedStoneMaterial,)>()
        .map(|(entity, _)| entity)
        .collect();
    let wood: Vec<_> = world
        .query::<(&WoodMaterial,)>()
        .map(|(entity, _)| entity)
        .collect();
    if stone.is_empty() && carved_stone.is_empty() && floor.is_empty() && wood.is_empty() {
        return None;
    }
    let mut store = TextureStore::new(12);
    if !stone.is_empty() {
        let base = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/castle_wall_varriation/castle_wall_varriation_diff_1k.png"),
            TexelKind::Colour,
        );
        let normal = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/castle_wall_varriation/castle_wall_varriation_nor_gl_1k.png"),
            TexelKind::Normal,
        );
        let arm = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/castle_wall_varriation/castle_wall_varriation_arm_1k.png"),
            TexelKind::Data,
        );
        apply_material(
            world,
            stone,
            base,
            normal,
            arm,
            std::env::var_os("HLFS_NO_STONE_NORMALS").is_none(),
            1.0,
        );
    }

    if !carved_stone.is_empty() {
        let base = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/plaster_stone_wall_01/plaster_stone_wall_01_diff_2k.jpg"),
            TexelKind::Colour,
        );
        let normal = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/plaster_stone_wall_01/plaster_stone_wall_01_nor_gl_2k.jpg"),
            TexelKind::Normal,
        );
        let arm = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/plaster_stone_wall_01/plaster_stone_wall_01_arm_2k.jpg"),
            TexelKind::Data,
        );
        apply_material(world, carved_stone, base, normal, arm, true, 1.0);
    }

    if !floor.is_empty() {
        let base = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/marble_01/marble_01_diff_2k.jpg"),
            TexelKind::Colour,
        );
        let normal = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/marble_01/marble_01_nor_gl_2k.jpg"),
            TexelKind::Normal,
        );
        let arm = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/marble_01/marble_01_arm_2k.jpg"),
            TexelKind::Data,
        );
        // The source's median roughness is about 0.5. Worn nave paving is
        // less reflective than a newly polished slab; retain map variation.
        apply_material(world, floor, base, normal, arm, true, 1.3);
    }
    if !wood.is_empty() {
        let base = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/fine_grained_wood/fine_grained_wood_col_2k.jpg"),
            TexelKind::Colour,
        );
        let normal = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/fine_grained_wood/fine_grained_wood_nor_gl_2k.jpg"),
            TexelKind::Normal,
        );
        let arm = upload(
            device,
            queue,
            &mut store,
            include_bytes!("assets/fine_grained_wood/fine_grained_wood_arm_2k.jpg"),
            TexelKind::Data,
        );
        apply_material(world, wood, base, normal, arm, true, 1.0);
    }
    Some(Arc::new(RwLock::new(store)))
}

fn apply_material(
    world: &mut World,
    entities: Vec<Entity>,
    base: u32,
    normal: u32,
    arm: u32,
    normal_enabled: bool,
    roughness_scale: f32,
) {
    for entity in entities {
        let mut material = world
            .get_mut::<MaterialComponent>(entity)
            .expect("architectural material");
        material.base_color = [1.0; 4];
        material.roughness_metallic[0] = roughness_scale;
        material.tex_base_color = base;
        material.tex_roughness = arm;
        material.tex_occlusion = arm;
        if normal_enabled {
            material.tex_normal = normal;
            material.flags |= helio_mats::FLAG_HAS_NORMAL_MAP;
        }
    }
}

/// Decodes `bytes` and uploads it with a full mip chain filtered as
/// `texels` says (`helio_mats::texture_mips`: normal maps keep their spread
/// for specular anti-aliasing).
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    store: &mut TextureStore,
    bytes: &[u8],
    texels: TexelKind,
) -> u32 {
    let image = image::load_from_memory(bytes)
        .expect("embedded architectural texture")
        .to_rgba8();
    let (width, height) = image.dimensions();
    let chain = mip_chain(image.as_raw(), width, height, texels);
    let mips: Vec<&[u8]> = chain.iter().map(|level| level.texels.as_slice()).collect();
    store
        .register_mips(
            device,
            queue,
            &wgpu::TextureDescriptor {
                label: Some("CC0 architectural material"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if texels.is_srgb() {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            &mips,
        )
        .expect("register architectural texture")
}
