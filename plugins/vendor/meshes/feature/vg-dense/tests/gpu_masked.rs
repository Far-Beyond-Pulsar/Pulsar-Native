//! Tier-3-NonRT with alpha-masked clusters: an opaque quad behind a masked quad
//! (checkerboard alpha). Where the mask discards, the dense peel must show the
//! quad BEHIND (alpha evaluated per fragment, before the nearest wins), where
//! it keeps, the masked quad. Skips (passes) without an adapter.

use bytemuck::cast_slice;
use helio_mats::{MaterialBindingConfig, MaterialTextureBindings, BINDLESS_MATERIAL_FEATURES};
use helio_pass_vg_dense::*;
use helio_vg_format::testing::{TestCluster, TestPage};
use helio_vg_format::{GpuVgInstance, INST_ENABLED};

const VIEW: [u32; 2] = [64, 64];
const CHECKS: u32 = 8;
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
const BACK_Z: f32 = 0.3;
const FRONT_Z: f32 = 0.6;
/// Unified visible index of the masked cluster (bin 6 range starts here).
const MASKED_AT: u32 = 100;

fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let man = bits & 0x007f_ffff;
    if value == 0.0 {
        return sign;
    }
    if exp <= 0 {
        return sign | (((man | 0x0080_0000) >> (14 - exp) as u32) as u16);
    }
    sign | ((exp as u16) << 10) | (man >> 13) as u16
}

fn buf(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let b = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (bytes.len() as u64).max(16).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    b
}

fn grid(
    x0: f32,
    y0: f32,
    size: f32,
    z: f32,
    n: usize,
    flip: bool,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[u8; 3]>) {
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (fx, fy) = (i as f32 / n as f32, j as f32 / n as f32);
            pos.push([x0 + size * fx, y0 + size * fy, z]);
            uv.push([fx, fy]);
        }
    }
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u8;
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            for t in [
                [at(i, j), at(i + 1, j), at(i, j + 1)],
                [at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)],
            ] {
                tris.push(if flip { [t[0], t[2], t[1]] } else { t });
            }
        }
    }
    (pos, uv, tris)
}

fn expected(px: u32, py: u32) -> Option<bool> {
    let x = (px as f64 + 0.5) / (VIEW[0] as f64 / 2.0) - 1.0;
    let y = 1.0 - (py as f64 + 0.5) / (VIEW[1] as f64 / 2.0);
    let (u, v) = ((x + 1.013) / 2.0, (y + 0.987) / 2.0);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    let (fu, fv) = (u * CHECKS as f64, v * CHECKS as f64);
    if (fu - fu.round()).abs() < 0.08 || (fv - fv.round()).abs() < 0.08 {
        return None;
    }
    Some((fu.floor() as u32 + fv.floor() as u32) % 2 == 0)
}

struct Setup {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn setup() -> Option<Setup> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let features = if adapter.features().contains(BINDLESS_MATERIAL_FEATURES) {
        BINDLESS_MATERIAL_FEATURES
    } else {
        wgpu::Features::empty()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        required_features: features,
        ..Default::default()
    }))
    .ok()?;
    Some(Setup { device, queue })
}

/// Peel the two-quad scene; returns `(depth bits, vis id)` per pixel.
fn run(s: &Setup, double_sided: bool, flip_front: bool, with_tables: bool) -> Vec<(u32, u32)> {
    let device = &s.device;
    let queue = &s.queue;
    // Back: opaque quad (no uv). Front: masked quad with uv attributes.
    let (bp, _, bt) = grid(-1.013, -0.987, 2.0, BACK_Z, 4, false);
    let (fp, fuv, ft) = grid(-1.013, -0.987, 2.0, FRONT_Z, 4, flip_front);
    let rows: Vec<[u8; 16]> = fuv
        .iter()
        .map(|uv| {
            let mut r = [0u8; 16];
            r[8..10].copy_from_slice(&f16_bits(uv[0]).to_le_bytes());
            r[10..12].copy_from_slice(&f16_bits(uv[1]).to_le_bytes());
            r
        })
        .collect();
    let page = TestPage::build(
        0,
        &[
            TestCluster {
                positions: &bp,
                tris: &bt,
                material: 0,
                attrs: None,
            },
            TestCluster {
                positions: &fp,
                tris: &ft,
                material: 0,
                attrs: Some(&rows),
            },
        ],
    );

    // Visible list: opaque at unified 0 (bin 0), masked at MASKED_AT (bin 6).
    let garbage = helio_vg_format::GpuVisibleCluster {
        instance: 0,
        cluster_slot: page.slots[0],
    };
    let mut visible = vec![garbage; 128];
    visible[0] = helio_vg_format::GpuVisibleCluster {
        instance: 0,
        cluster_slot: page.slots[0],
    };
    visible[MASKED_AT as usize] = helio_vg_format::GpuVisibleCluster {
        instance: 0,
        cluster_slot: page.slots[1],
    };
    let mut cull_state = vec![0u32; 160];
    let r = helio_vg_format::CULL_STATE_RANGES as usize;
    cull_state[r] = 0; // phase 0 bin 0 {0, 1}
    cull_state[r + 1] = 1;
    let masked_bin = helio_vg_format::CULL_FIRST_MASKED_BIN as usize;
    cull_state[r + masked_bin * 2] = MASKED_AT; // phase 0 bin 6 {100, 1}
    cull_state[r + masked_bin * 2 + 1] = 1;

    let tiles = [VIEW[0].div_ceil(16), VIEW[1].div_ceil(16)];
    let mask = vec![1u32; (tiles[0] * tiles[1]) as usize];

    let tile_mask = buf(device, "mask", cast_slice(&mask));
    queue.write_buffer(&tile_mask, 0, cast_slice(&mask));
    let cull_buf = buf(device, "cull", cast_slice(&cull_state));
    queue.write_buffer(&cull_buf, 0, cast_slice(&cull_state));
    let vis_buf = buf(device, "visible", cast_slice(&visible));
    queue.write_buffer(&vis_buf, 0, cast_slice(&visible));
    let pool = buf(device, "pool", &page.pool());
    queue.write_buffer(&pool, 0, &page.pool());
    let quant = buf(device, "quant", cast_slice(&[page.quant]));
    queue.write_buffer(&quant, 0, cast_slice(&[page.quant]));
    let inst = GpuVgInstance {
        asset_slot: 0,
        flags: INST_ENABLED,
        lod_error_scale: 1.0,
        object_row: 0,
    };
    let instances = buf(device, "instances", cast_slice(&[inst]));
    queue.write_buffer(&instances, 0, cast_slice(&[inst]));
    let mut object = [0f32; 63];
    object[4..20].copy_from_slice(cast_slice(&IDENTITY));
    let objects = buf(device, "objects", cast_slice(&object));
    queue.write_buffer(&objects, 0, cast_slice(&object));

    // Materials: row 0 alpha-tested checkerboard.
    let binding = MaterialBindingConfig::for_device(device);
    let make_texture = |w: u32, h: u32, texels: &[u8]| {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("alpha texture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        t
    };
    let mut checker = Vec::new();
    for y in 0..CHECKS {
        for x in 0..CHECKS {
            checker.extend_from_slice(&[200, 200, 200, if (x + y) % 2 == 0 { 255 } else { 0 }]);
        }
    }
    let checker_tex = make_texture(CHECKS, CHECKS, &checker);
    let white_tex = make_texture(1, 1, &[255; 4]);
    let views: Vec<wgpu::TextureView> = (0..binding.max_textures)
        .map(|i| {
            if i == 0 {
                checker_tex.create_view(&Default::default())
            } else {
                white_tex.create_view(&Default::default())
            }
        })
        .collect();
    let samplers: Vec<wgpu::Sampler> = (0..binding.max_textures)
        .map(|_| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        })
        .collect();
    let mut material = [0u32; 24];
    for w in material.iter_mut().take(4) {
        *w = 1.0f32.to_bits();
    }
    material[12] = 0;
    material[13] = u32::MAX;
    material[14] = u32::MAX;
    material[15] = u32::MAX;
    material[16] = u32::MAX;
    material[18] = helio_mats::FLAG_ALPHA_TEST
        | if double_sided {
            helio_mats::FLAG_DOUBLE_SIDED
        } else {
            0
        };
    let mut tex_data = [0u32; 88];
    tex_data[87] = (-1.0f32).to_bits();
    let materials = buf(device, "materials", cast_slice(&material));
    queue.write_buffer(&materials, 0, cast_slice(&material));
    let material_textures = buf(device, "material textures", cast_slice(&tex_data));
    queue.write_buffer(&material_textures, 0, cast_slice(&tex_data));
    let view_refs: Vec<&wgpu::TextureView> = views.iter().collect();
    let sampler_refs: Vec<&wgpu::Sampler> = samplers.iter().collect();

    let cfg = DenseConfig::default();
    let mut pass = DenseNonRtPass::new(device, cfg).expect("pass");
    if with_tables {
        pass.set_masked(Some((
            &materials,
            MaterialTextureBindings {
                material_textures: &material_textures,
                texture_views: &view_refs,
                samplers: &sampler_refs,
                version: 0,
            },
        )));
    }
    let frame = DenseFrame {
        view_proj: IDENTITY,
        inv_view_proj: IDENTITY,
        camera_pos: [0.0; 3],
        viewport: VIEW,
        visible_capacity: visible.len() as u32,
        ray_t: [0.0, 1.0],
    };
    let mut enc = device.create_command_encoder(&Default::default());
    pass.record(
        device,
        queue,
        &mut enc,
        &frame,
        &DenseNonRtInputs {
            tile_mask: &tile_mask,
            cull_state: &cull_buf,
            visible: &vis_buf,
            page_pool: &pool,
            asset_quant: &quant,
            vg_instances: &instances,
            objects: &objects,
        },
    );
    queue.submit(Some(enc.finish()));
    let b = pass.buffers().unwrap();
    let raw = read_buffer(
        device,
        queue,
        &b.dense_key,
        VIEW[0] as u64 * VIEW[1] as u64 * 8,
    );
    let words: &[u32] = cast_slice(&raw);
    words.chunks(2).map(|c| (c[0], c[1])).collect()
}

fn judge(label: &str, keys: &[(u32, u32)], front_everywhere: bool, back_everywhere: bool) {
    let (mut front, mut back, mut judged) = (0, 0, 0);
    for py in 0..VIEW[1] {
        for px in 0..VIEW[0] {
            let Some(keep) = expected(px, py) else {
                continue;
            };
            judged += 1;
            let (zbits, id) = keys[(py * VIEW[0] + px) as usize];
            let cluster = id >> 7;
            let z = f32::from_bits(zbits);
            let is_front = cluster == MASKED_AT;
            let want_front = if front_everywhere {
                true
            } else if back_everywhere {
                false
            } else {
                keep
            };
            assert_eq!(
                is_front, want_front,
                "{label}: pixel ({px},{py}): cluster {cluster}, depth {z}"
            );
            let want_z = if want_front { FRONT_Z } else { BACK_Z };
            assert!(
                (z - want_z).abs() < 1e-3,
                "{label}: pixel ({px},{py}) depth {z}, want {want_z}"
            );
            if is_front {
                front += 1
            } else {
                back += 1
            }
        }
    }
    eprintln!("{label}: judged {judged}: masked quad {front}, quad behind {back}");
    if !front_everywhere && !back_everywhere {
        assert!(front > 200 && back > 200, "{label}: both layers must show");
    }
}

#[test]
fn dense_peel_evaluates_alpha_per_fragment_and_reveals_the_layer_behind() {
    let Some(s) = setup() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    // Double-sided material: checker holes show the opaque quad behind.
    let keys = run(&s, true, false, true);
    judge("dense masked, two-sided", &keys, false, false);
    // Single-sided, front facing: same result.
    let keys = run(&s, false, false, true);
    judge("dense masked, single-sided front", &keys, false, false);
    // Single-sided seen from behind: the whole masked quad is culled.
    let keys = run(&s, false, true, true);
    judge("dense masked, single-sided back", &keys, false, true);
    // Two-sided seen from behind keeps the holes pattern.
    let keys = run(&s, true, true, true);
    judge("dense masked, two-sided back", &keys, false, false);
}

#[test]
fn without_material_tables_masked_clusters_are_ignored_not_drawn_opaque() {
    let Some(s) = setup() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let keys = run(&s, true, false, false);
    // The opaque quad is all that remains: the masked cluster never covers a
    // pixel as a solid quad.
    judge("dense masked without tables", &keys, false, true);
}
