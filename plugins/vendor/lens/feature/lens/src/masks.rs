//! Mask textures for lens regions and contaminants: a `D2Array` of RGBA8
//! layers, bound as [`crate::MASKS`]. A [`crate::region_kind::MASK`] region
//! reads one channel of one layer as its coverage; a
//! [`crate::element_kind::WATER_DROPS`] element reads the normals of a layer.

use crate::MASK_FORMAT;

/// A `D2Array` view of mask layers for [`crate::LensInputs::masks`]:
/// `layers` are `size` x `size` RGBA8 images (row-major, 4 bytes per texel,
/// y down), one array layer each.
pub fn mask_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    layers: &[&[u8]],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let data: Vec<u8> = layers
        .iter()
        .flat_map(|layer| layer.iter().copied())
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Lens masks"),
                size: wgpu::Extent3d {
                    width: size.max(1),
                    height: size.max(1),
                    depth_or_array_layers: layers.len().max(1) as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MASK_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}

/// SplitMix64: a deterministic generator, so a seed always gives the same drops.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A procedural water-drop layer: `drops` spherical caps scattered over a
/// `size` x `size` texture, radii between `radius[0]` and `radius[1]` as
/// fractions of the width, placed by a generator seeded with `seed`
/// (the same arguments always give the same bytes).
///
/// Channels: red and green hold the surface normal's x and y as `n * 0.5 +
/// 0.5` (`0.5` outside a drop: flat), blue the cap's height, alpha the
/// footprint (1 inside a drop, a texel of antialiasing at its rim). Where
/// drops overlap the higher cap wins. Use alpha as the
/// [`crate::LensRegion::MaskChannel`] footprint of a
/// [`crate::LensElementRow::water_drops`] element on the same layer.
pub fn water_drop_mask(size: u32, drops: u32, radius: [f32; 2], seed: u64) -> Vec<u8> {
    let size = size.max(1);
    let n = size as usize;
    let mut rgba = vec![0u8; n * n * 4];
    for texel in rgba.as_chunks_mut::<4>().0 {
        *texel = [128, 128, 0, 0];
    }
    let mut height = vec![0.0f32; n * n];
    let mut rng = Rng(seed);
    let lo = radius[0].min(radius[1]).max(1.0 / size as f32);
    let hi = radius[0].max(radius[1]).max(lo);
    for _ in 0..drops {
        let (cx, cy) = (rng.unit() * size as f32, rng.unit() * size as f32);
        let r = (lo + (hi - lo) * rng.unit()) * size as f32;
        let (x0, x1) = (
            (cx - r - 1.0).floor().max(0.0) as usize,
            ((cx + r + 1.0).ceil() as usize).min(n),
        );
        let (y0, y1) = (
            (cy - r - 1.0).floor().max(0.0) as usize,
            ((cy + r + 1.0).ceil() as usize).min(n),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= r + 0.5 {
                    continue;
                }
                let cap = (1.0 - (d / r).min(1.0).powi(2)).sqrt();
                let coverage = (r + 0.5 - d).clamp(0.0, 1.0);
                if cap < height[y * n + x] {
                    continue;
                }
                height[y * n + x] = cap;
                let normal = [dx / r, dy / r];
                let texel = &mut rgba[(y * n + x) * 4..][..4];
                texel[0] = ((normal[0] * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                texel[1] = ((normal[1] * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                texel[2] = (cap * 255.0 + 0.5) as u8;
                texel[3] = (coverage * 255.0 + 0.5) as u8;
            }
        }
    }
    rgba
}
