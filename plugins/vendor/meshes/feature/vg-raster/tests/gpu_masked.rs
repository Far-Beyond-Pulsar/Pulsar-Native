//! Headless GPU tests of the alpha-masked raster tiers: a quad with a
//! checkerboard alpha texture. Discarded pixels must show the background
//! (`VIS_EMPTY` / no SW key), kept pixels the cluster, for the hardware masked
//! pipeline and the 64-bit atomic software masked kernel, for two-sided and
//! single-sided materials. Skips (passes) when no adapter can be created.

use glam::{Mat4, Vec3};
use helio_core::GpuCameraUniforms;
use helio_mats::{MaterialBindingConfig, MaterialTextureBindings, BINDLESS_MATERIAL_FEATURES};
use helio_pass_vg_raster::abi::*;
use helio_pass_vg_raster::stream::PagePool;
use helio_pass_vg_raster::*;
use helio_vg_format::{GpuVgInstance, INST_ENABLED};
use wgpu::util::DeviceExt;

const W: u32 = 64;
const H: u32 = 64;
const CHECKS: u32 = 8;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    int64: bool,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let want = wgpu::Features::INDIRECT_FIRST_INSTANCE;
    if !adapter.features().contains(want) {
        eprintln!("skipping: INDIRECT_FIRST_INSTANCE unavailable");
        return None;
    }
    let opt = optional_features() & adapter.features();
    let mut features = want
        | if supports_int64_atomics(opt) {
            opt
        } else {
            wgpu::Features::empty()
        };
    if adapter.features().contains(BINDLESS_MATERIAL_FEATURES) {
        features |= BINDLESS_MATERIAL_FEATURES;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("vg-raster masked test"),
        required_features: features,
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    device.on_uncaptured_error(std::sync::Arc::new(|e| panic!("wgpu error: {e:?}")));
    Some(Gpu {
        int64: supports_int64_atomics(device.features()),
        device,
        queue,
    })
}

fn buffer_init<T: bytemuck::Pod>(
    g: &Gpu,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    g.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage,
        })
}

fn map(g: &Gpu, rb: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    rb.slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let v = rb.slice(..).get_mapped_range().unwrap().to_vec();
    rb.unmap();
    v
}

fn read_buffer(g: &Gpu, src: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &rb, 0, size);
    g.queue.submit([enc.finish()]);
    map(g, &rb)
}

fn read_vis(g: &Gpu, tex: &wgpu::Texture) -> Vec<u32> {
    let bpr = W * 4;
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vis readback"),
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &rb,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit([enc.finish()]);
    bytemuck::cast_slice(&map(g, &rb)).to_vec()
}

/// A flat n x n grid of triangles on plane z, spanning [x0, x0+size] x [y0, y0+size].
/// Counter-clockwise as seen from +Z unless `flip`. uv = (x - x0, y - y0) / size.
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

/// Material table with one alpha-tested material (row 0) whose base-color
/// texture is a checkerboard (opaque on even checks), plus the texture table.
struct Alpha {
    materials: wgpu::Buffer,
    material_textures: wgpu::Buffer,
    views: Vec<wgpu::TextureView>,
    samplers: Vec<wgpu::Sampler>,
}

fn alpha_resources(g: &Gpu, double_sided: bool) -> Alpha {
    let binding = MaterialBindingConfig::for_device(&g.device);
    let make_texture = |w: u32, h: u32, texels: &[u8]| {
        let t = g.device.create_texture(&wgpu::TextureDescriptor {
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
        g.queue.write_texture(
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
            let a = if (x + y) % 2 == 0 { 255 } else { 0 };
            checker.extend_from_slice(&[200, 200, 200, a]);
        }
    }
    let checker_tex = make_texture(CHECKS, CHECKS, &checker);
    let white_tex = make_texture(1, 1, &[255, 255, 255, 255]);
    let views: Vec<wgpu::TextureView> = (0..binding.max_textures)
        .map(|i| {
            if i == 0 {
                checker_tex.create_view(&Default::default())
            } else {
                white_tex.create_view(&Default::default())
            }
        })
        .collect();
    let nearest = g.device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let samplers: Vec<wgpu::Sampler> = (0..binding.max_textures)
        .map(|_| {
            g.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        })
        .collect();
    drop(nearest);

    // GpuMaterial row (24 words): base color white, tex_base_color = 0, flags.
    let mut material = [0u32; 24];
    for i in 0..4 {
        material[i] = 1.0f32.to_bits();
    }
    material[12] = 0;
    material[16] = u32::MAX;
    material[13] = u32::MAX;
    material[14] = u32::MAX;
    material[15] = u32::MAX;
    material[18] = helio_mats::FLAG_ALPHA_TEST
        | if double_sided {
            helio_mats::FLAG_DOUBLE_SIDED
        } else {
            0
        };
    // MaterialTextureData single row with params.w = -1: texture ids come
    // straight from the GpuMaterial, cutoff 0.5.
    let mut tex_data = [0u32; 88];
    tex_data[87] = (-1.0f32).to_bits();
    let st = wgpu::BufferUsages::STORAGE;
    Alpha {
        materials: buffer_init(g, "materials", &material, st),
        material_textures: buffer_init(g, "material textures", &tex_data, st),
        views,
        samplers,
    }
}

struct Scene {
    camera: GpuCameraUniforms,
    built: helio_pass_vg_raster::stream::BuiltPool,
}

fn scene(flip: bool) -> Scene {
    let mut pool = PagePool::default();
    let (p, uv, t) = grid(-1.013, -0.987, 2.0, -2.0, 4, flip);
    pool.push_with_uvs(&p, &t, 0, 0, &uv);
    let view = Mat4::IDENTITY;
    let proj = helio_core::depth::perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.1);
    let camera = GpuCameraUniforms::new(
        view,
        proj,
        Vec3::ZERO,
        0.1,
        1000.0,
        0,
        [0.0, 0.0],
        proj * view,
    );
    Scene {
        camera,
        built: pool.build(),
    }
}

/// Expected alpha-kept mask per pixel (`Some`), or `None` for pixels too close
/// to a texel boundary (f16 uv rounding) or off the quad to judge.
fn expected(px: u32, py: u32) -> Option<bool> {
    let ndc_x = (px as f64 + 0.5) / (W as f64 / 2.0) - 1.0;
    let ndc_y = 1.0 - (py as f64 + 0.5) / (H as f64 / 2.0);
    // plane z = -2, fov 90: world = ndc * 2
    let (x, y) = (ndc_x * 2.0, ndc_y * 2.0);
    let (u, v) = ((x + 1.013) / 2.0, (y + 0.987) / 2.0);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    let (fu, fv) = (u * CHECKS as f64, v * CHECKS as f64);
    if (fu - fu.round()).abs() < 0.05 || (fv - fv.round()).abs() < 0.05 {
        return None;
    }
    Some((fu.floor() as u32 + fv.floor() as u32) % 2 == 0)
}

struct Run {
    vis: Vec<u32>,
    sw_keys: Vec<[u32; 2]>,
}

fn run(g: &Gpu, s: &Scene, alpha: &Alpha, software: bool) -> Run {
    let raster = VgRaster::new(&g.device, VgRasterConfig::default());
    let mut targets = VgTargets::new(&g.device, W, H, &raster);
    let st = wgpu::BufferUsages::STORAGE;
    let mut pages = s.built.bytes.clone();
    while pages.len() % 16 != 0 {
        pages.push(0);
    }
    pages.extend_from_slice(&[0u8; 16]);
    let inst = GpuVgInstance {
        asset_slot: 0,
        flags: INST_ENABLED,
        lod_error_scale: 1.0,
        object_row: 0,
    };
    // static_objects row 0: transform identity; word 2 = material slot 0.
    let mut object = [0f32; 63];
    object[4..20].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
    // Indirect: 4 opaque HW draws (empty) + the masked draw at offset 64.
    let mut hw = [DrawIndirectArgs::default(); 5];
    if !software {
        hw[4] = DrawIndirectArgs {
            vertex_count: 384,
            instance_count: 1,
            first_vertex: 0,
            first_instance: 0,
        };
    }
    let sw_count = u32::from(software);
    // sw_meta words: [0,1] opaque software bin (empty), [2,3] masked.
    let sw_meta = [0u32, 0, 0, sw_count];
    let mut sw_dispatch = [0u32; 8];
    sw_dispatch[0..3].copy_from_slice(&[0, 1, 1]);
    sw_dispatch[4..7].copy_from_slice(&sw_dispatch_args(sw_count));

    let camera = buffer_init(g, "camera", &[s.camera], st);
    let pages = buffer_init(g, "pages", &pages, st);
    let quant = buffer_init(g, "quant", &[s.built.quant], st);
    let visible = buffer_init(g, "visible", &s.built.visible, st);
    let instances = buffer_init(g, "instances", &[inst], st);
    let objects = buffer_init(g, "objects", &object, st);
    let hw_indirect = buffer_init(g, "hw indirect", &hw, wgpu::BufferUsages::INDIRECT);
    let sw_meta_buf = buffer_init(g, "sw meta", &sw_meta, st);
    let sw_dispatch_buf = buffer_init(g, "sw dispatch", &sw_dispatch, wgpu::BufferUsages::INDIRECT);
    let view_refs: Vec<&wgpu::TextureView> = alpha.views.iter().collect();
    let sampler_refs: Vec<&wgpu::Sampler> = alpha.samplers.iter().collect();

    let inputs = VgRasterInputs {
        camera: &camera,
        view_index: 0,
        pages: &pages,
        asset_quant: &quant,
        visible: &visible,
        vg_instances: &instances,
        objects: &objects,
        hw_indirect: &hw_indirect,
        hw_indirect_offset: 0,
        sw_meta: &sw_meta_buf,
        sw_meta_offset_words: 0,
        sw_dispatch: &sw_dispatch_buf,
        sw_dispatch_offset: 0,
        masked: Some(VgMaskedInputs {
            materials: &alpha.materials,
            material_textures: MaterialTextureBindings {
                material_textures: &alpha.material_textures,
                texture_views: &view_refs,
                samplers: &sampler_refs,
                version: 0,
            },
            hw_indirect_offset: 64,
            sw_meta_offset_words: 2,
            sw_dispatch_offset: 16,
        }),
        dense: None,
    };
    let mut enc = g.device.create_command_encoder(&Default::default());
    raster.encode(&g.device, &g.queue, &mut enc, &mut targets, &inputs);
    g.queue.submit([enc.finish()]);

    let keys: Vec<u32> =
        bytemuck::cast_slice(&read_buffer(g, &targets.sw_keys, (W * H * 8) as u64)).to_vec();
    Run {
        vis: read_vis(g, &targets.vis),
        sw_keys: keys.chunks(2).map(|c| [c[0], c[1]]).collect(),
    }
}

fn check(label: &str, covered: impl Fn(u32, u32) -> bool, keep_all: bool, none_kept: bool) {
    let mut kept = 0;
    let mut dropped = 0;
    let mut judged = 0;
    for py in 0..H {
        for px in 0..W {
            let Some(want) = expected(px, py) else {
                continue;
            };
            judged += 1;
            let got = covered(px, py);
            let want = if none_kept {
                false
            } else if keep_all {
                true
            } else {
                want
            };
            assert_eq!(
                got,
                want,
                "{label}: pixel ({px},{py}) expected {}",
                if want { "kept" } else { "background" }
            );
            if got {
                kept += 1;
            } else {
                dropped += 1;
            }
        }
    }
    eprintln!("{label}: judged {judged}, kept {kept}, discarded {dropped}");
    if !none_kept && !keep_all {
        assert!(
            kept > 200 && dropped > 200,
            "{label}: the checkerboard must keep and discard many pixels"
        );
    }
}

#[test]
fn hardware_masked_discards_checker_holes() {
    let _lock = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(g) = gpu() else { return };
    let s = scene(false);
    let alpha = alpha_resources(&g, true);
    let r = run(&g, &s, &alpha, false);
    check(
        "hw masked",
        |x, y| r.vis[(y * W + x) as usize] != VIS_EMPTY,
        false,
        false,
    );
    // The software output stays empty.
    assert!(r.sw_keys.iter().all(|k| k[1] == 0));
}

#[test]
fn hardware_masked_two_sided_keeps_back_faces_and_single_sided_drops_them() {
    let _lock = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(g) = gpu() else { return };
    let back = scene(true);
    // Two-sided: the back face is shaded, with the same checker holes.
    let two = alpha_resources(&g, true);
    let r = run(&g, &back, &two, false);
    check(
        "hw masked back, two-sided",
        |x, y| r.vis[(y * W + x) as usize] != VIS_EMPTY,
        false,
        false,
    );
    // Single-sided: the back face of the same quad is discarded entirely.
    let one = alpha_resources(&g, false);
    let r = run(&g, &back, &one, false);
    check(
        "hw masked back, single-sided",
        |x, y| r.vis[(y * W + x) as usize] != VIS_EMPTY,
        false,
        true,
    );
    // Single-sided front face behaves like two-sided.
    let front = scene(false);
    let r = run(&g, &front, &one, false);
    check(
        "hw masked front, single-sided",
        |x, y| r.vis[(y * W + x) as usize] != VIS_EMPTY,
        false,
        false,
    );
}

#[test]
fn software_masked_discards_checker_holes() {
    let _lock = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(g) = gpu() else { return };
    if !g.int64 {
        eprintln!(
            "skipping: no 64-bit atomics (the cull routes masked clusters to the hardware bin)"
        );
        return;
    }
    let alpha = alpha_resources(&g, true);
    let s = scene(false);
    let r = run(&g, &s, &alpha, true);
    check(
        "sw masked",
        |x, y| r.sw_keys[(y * W + x) as usize][1] != 0,
        false,
        false,
    );
    // Nothing from Tier 1.
    assert!(r.vis.iter().all(|v| *v == VIS_EMPTY));
    // The surviving key carries the masked cluster's id (visible index 0).
    let some = r.sw_keys.iter().find(|k| k[1] != 0).unwrap();
    assert_eq!(some[0] >> 7, 0);

    // Back face: single-sided drops it, two-sided keeps it.
    let back = scene(true);
    let one = alpha_resources(&g, false);
    let r = run(&g, &back, &one, true);
    check(
        "sw masked back, single-sided",
        |x, y| r.sw_keys[(y * W + x) as usize][1] != 0,
        false,
        true,
    );
    let r = run(&g, &back, &alpha, true);
    check(
        "sw masked back, two-sided",
        |x, y| r.sw_keys[(y * W + x) as usize][1] != 0,
        false,
        false,
    );
}
