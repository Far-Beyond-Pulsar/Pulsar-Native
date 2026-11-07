//! Mip chains for material textures, filtered the way each kind of texel is
//! sampled.
//!
//! - Colour (sRGB) is averaged in linear light: averaging the encoded bytes
//!   would darken every mip.
//! - Data (roughness, occlusion, metalness, opacity) is averaged as plain
//!   numbers.
//! - A tangent-space normal map's texel at any level is the mean of the
//!   full-resolution normals it covers: its direction, made unit, in xyz,
//!   and its length in alpha. The length is what the normals' spread leaves
//!   of a unit vector: 1 where they all agree, less where the texel averages
//!   a bumpy patch. Shaders widen roughness by it (`specular_aa.wgsl`,
//!   `normal_map_roughness`; Toksvig 2005), so bumps too small to see under
//!   a pixel shine as the rough surface they make up instead of sparkling
//!   from frame to frame. A full-resolution texel is one normal: alpha 1.
//!
//! Every level is computed from the full-resolution texels in `f32` (each
//! level from the unrounded one above), so rounding never builds up down the
//! chain. An odd edge reuses its last row or column.

/// What a texture's texels hold: how its mips are filtered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexelKind {
    /// A colour (base colour, emissive): sRGB, averaged in linear light.
    /// Alpha is linear.
    Colour,
    /// Numbers in the channels, averaged as numbers.
    Data,
    /// A tangent-space normal map (xyz in `[-1, 1]` as `[0, 1]`); its alpha
    /// becomes the mean normal's length (see the module docs).
    Normal,
}

impl TexelKind {
    /// Whether the texture is sampled as sRGB.
    pub fn is_srgb(self) -> bool {
        self == TexelKind::Colour
    }
}

/// One level of a chain: tightly packed RGBA8 rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<u8>,
}

/// The number of levels in a full mip chain for a `width` x `height` image.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// Every level of `rgba` (straight-alpha RGBA8, `width` x `height`),
/// largest first, down to one texel, filtered as `kind` says. Level 0 is
/// `rgba` itself, except that a normal map's alpha is 1 there.
///
/// Panics if `rgba` is not `width * height * 4` bytes or the image is empty.
pub fn mip_chain(rgba: &[u8], width: u32, height: u32, kind: TexelKind) -> Vec<MipLevel> {
    assert!(width > 0 && height > 0, "an image has texels");
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "rgba is width x height RGBA8"
    );
    let codec = Codec::new(kind);
    let mut top: Vec<[f32; 4]> = rgba.chunks_exact(4).map(|t| codec.decode(t)).collect();
    let mut levels = vec![MipLevel {
        width,
        height,
        texels: rgba.to_vec(),
    }];
    if kind == TexelKind::Normal {
        for texel in levels[0].texels.chunks_exact_mut(4) {
            texel[3] = 255;
        }
    }
    let (mut w, mut h) = (width, height);
    while levels.len() < mip_level_count(width, height) as usize {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = Vec::with_capacity(nw as usize * nh as usize);
        for y in 0..nh {
            let (y0, y1) = ((2 * y).min(h - 1), (2 * y + 1).min(h - 1));
            for x in 0..nw {
                let (x0, x1) = ((2 * x).min(w - 1), (2 * x + 1).min(w - 1));
                let mut sum = [0.0f32; 4];
                for (tx, ty) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                    let t = top[(ty * w + tx) as usize];
                    for c in 0..4 {
                        sum[c] += t[c] * 0.25;
                    }
                }
                next.push(sum);
            }
        }
        levels.push(MipLevel {
            width: nw,
            height: nh,
            texels: next.iter().flat_map(|t| codec.encode(t)).collect(),
        });
        top = next;
        (w, h) = (nw, nh);
    }
    levels
}

/// Bytes to the values averaged, and back.
struct Codec {
    kind: TexelKind,
    /// sRGB byte to linear.
    to_linear: [f32; 256],
    /// Linear `[0, 1]` in `SRGB_STEPS` steps to an sRGB byte: fine enough
    /// that the table's own rounding stays under one output step.
    to_srgb: Vec<u8>,
}

const SRGB_STEPS: usize = 16384;

impl Codec {
    fn new(kind: TexelKind) -> Self {
        let (to_linear, to_srgb) = if kind == TexelKind::Colour {
            (
                std::array::from_fn(|v| {
                    let c = v as f32 / 255.0;
                    if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    }
                }),
                (0..=SRGB_STEPS)
                    .map(|i| {
                        let l = i as f32 / SRGB_STEPS as f32;
                        let c = if l <= 0.003_130_8 {
                            l * 12.92
                        } else {
                            1.055 * l.powf(1.0 / 2.4) - 0.055
                        };
                        (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8
                    })
                    .collect(),
            )
        } else {
            ([0.0; 256], Vec::new())
        };
        Self {
            kind,
            to_linear,
            to_srgb,
        }
    }

    fn decode(&self, t: &[u8]) -> [f32; 4] {
        let unit = |v: u8| f32::from(v) / 255.0;
        match self.kind {
            TexelKind::Colour => [
                self.to_linear[t[0] as usize],
                self.to_linear[t[1] as usize],
                self.to_linear[t[2] as usize],
                unit(t[3]),
            ],
            TexelKind::Data => [unit(t[0]), unit(t[1]), unit(t[2]), unit(t[3])],
            TexelKind::Normal => {
                let v = [
                    unit(t[0]) * 2.0 - 1.0,
                    unit(t[1]) * 2.0 - 1.0,
                    unit(t[2]) * 2.0 - 1.0,
                ];
                let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                // A texel encoding no direction counts as straight out.
                if length > 1.0e-4 {
                    [v[0] / length, v[1] / length, v[2] / length, 0.0]
                } else {
                    [0.0, 0.0, 1.0, 0.0]
                }
            }
        }
    }

    fn encode(&self, v: &[f32; 4]) -> [u8; 4] {
        let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        match self.kind {
            TexelKind::Colour => {
                let srgb =
                    |x: f32| self.to_srgb[(x.clamp(0.0, 1.0) * SRGB_STEPS as f32 + 0.5) as usize];
                [srgb(v[0]), srgb(v[1]), srgb(v[2]), byte(v[3])]
            }
            TexelKind::Data => [byte(v[0]), byte(v[1]), byte(v[2]), byte(v[3])],
            TexelKind::Normal => {
                let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                // Normals that cancel out leave no direction: straight out,
                // and the length (0) says they spread every way.
                let unit = if length > 1.0e-4 {
                    [v[0] / length, v[1] / length, v[2] / length]
                } else {
                    [0.0, 0.0, 1.0]
                };
                [
                    byte(unit[0] * 0.5 + 0.5),
                    byte(unit[1] * 0.5 + 0.5),
                    byte(unit[2] * 0.5 + 0.5),
                    byte(length),
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, texel: [u8; 4]) -> Vec<u8> {
        (0..width * height).flat_map(|_| texel).collect()
    }

    fn checker_2x2() -> Vec<u8> {
        [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        ]
        .concat()
    }

    /// A normal map texel tilted by `(x, y)`.
    fn tilt(x: f32, y: f32) -> [u8; 4] {
        let z = (1.0 - x * x - y * y).sqrt();
        let byte = |v: f32| ((v * 0.5 + 0.5) * 255.0 + 0.5) as u8;
        [byte(x), byte(y), byte(z), 0]
    }

    fn normal_of(texel: &[u8]) -> [f32; 4] {
        std::array::from_fn(|c| {
            if c < 3 {
                f32::from(texel[c]) / 255.0 * 2.0 - 1.0
            } else {
                f32::from(texel[c]) / 255.0
            }
        })
    }

    #[test]
    fn chains_run_down_to_one_texel() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(8, 4), 4);
        assert_eq!(mip_level_count(2048, 2048), 12);
        let chain = mip_chain(&solid(8, 4, [10, 20, 30, 255]), 8, 4, TexelKind::Colour);
        let sizes: Vec<_> = chain
            .iter()
            .map(|l| (l.width, l.height, l.texels.len()))
            .collect();
        assert_eq!(sizes, [(8, 4, 128), (4, 2, 32), (2, 1, 8), (1, 1, 4)]);
        for kind in [TexelKind::Colour, TexelKind::Data] {
            let chain = mip_chain(&solid(8, 4, [10, 20, 30, 255]), 8, 4, kind);
            assert!(
                chain
                    .iter()
                    .all(|l| l.texels.chunks(4).all(|t| t == [10, 20, 30, 255])),
                "{kind:?}: a flat image stays flat"
            );
        }
    }

    #[test]
    fn odd_sizes_reuse_their_last_row_and_column() {
        let chain = mip_chain(&solid(5, 3, [200, 100, 50, 255]), 5, 3, TexelKind::Colour);
        assert_eq!(chain.len(), 3);
        assert_eq!((chain[1].width, chain[1].height), (2, 1));
        assert_eq!(chain[2].texels, [200, 100, 50, 255]);
    }

    #[test]
    fn colour_is_averaged_in_linear_light() {
        // Half black, half white: half the light is about 188 as an sRGB
        // byte, not 127.
        let top = &mip_chain(&checker_2x2(), 2, 2, TexelKind::Colour)[1].texels;
        assert!(
            (186..=190).contains(&top[0]),
            "linear-light average, got {}",
            top[0]
        );
        assert_eq!(top[3], 255);
    }

    #[test]
    fn data_is_averaged_as_plain_numbers() {
        let top = &mip_chain(&checker_2x2(), 2, 2, TexelKind::Data)[1].texels;
        assert!(
            (127..=128).contains(&top[0]),
            "plain average, got {}",
            top[0]
        );
    }

    #[test]
    fn rounding_does_not_build_up_down_the_chain() {
        // Three 2x2 blocks half 0, half 1 and one all 0: level 1 is 0.5,
        // 0.5, 0.5, 0 (bytes 1, 1, 1, 0) and level 2 the exact mean 0.375,
        // byte 0 (averaging level 1's bytes would give 1).
        let rgba: Vec<u8> = (0..16u32)
            .flat_map(|i| {
                let (x, y) = (i % 4, i / 4);
                let one = (x / 2, y / 2) != (1, 1) && x % 2 == 0;
                [u8::from(one); 4]
            })
            .collect();
        let chain = mip_chain(&rgba, 4, 4, TexelKind::Data);
        assert_eq!(
            chain[1].texels.chunks(4).map(|t| t[0]).collect::<Vec<_>>(),
            [1, 1, 1, 0]
        );
        assert_eq!(chain[2].texels, [0, 0, 0, 0]);
    }

    #[test]
    fn a_normal_maps_alpha_is_its_mean_normals_length() {
        // Four texels tilted four ways around straight out: the mean is
        // straight out, 0.8 long.
        let texels = [
            tilt(0.6, 0.0),
            tilt(-0.6, 0.0),
            tilt(0.0, 0.6),
            tilt(0.0, -0.6),
        ]
        .concat();
        let chain = mip_chain(&texels, 2, 2, TexelKind::Normal);
        assert!(
            chain[0].texels.chunks(4).all(|t| t[3] == 255),
            "a full-resolution texel is one normal"
        );
        let n = normal_of(&chain[1].texels);
        assert!(
            n[2] > 0.99 && n[0].abs() < 0.01 && n[1].abs() < 0.01,
            "the tilts cancel: {n:?}"
        );
        assert!(
            (n[3] - 0.8).abs() < 0.01,
            "length of the mean, got {}",
            n[3]
        );
    }

    #[test]
    fn a_normal_maps_spread_is_measured_from_full_resolution() {
        // 4x4 of two tilts in stripes: level 1's texels each hold one of
        // each (mean straight out, shorter than 1), and level 2's alpha is
        // the full-resolution mean's length, not the length of its
        // renormalised level-1 texels' mean (1).
        let a = tilt(0.6, 0.0);
        let b = tilt(-0.6, 0.0);
        let texels: Vec<u8> = (0..16)
            .flat_map(|i| if i % 2 == 0 { a } else { b })
            .collect();
        let chain = mip_chain(&texels, 4, 4, TexelKind::Normal);
        for level in [1, 2] {
            let n = normal_of(&chain[level].texels);
            assert!((n[3] - 0.8).abs() < 0.01, "level {level}: length {}", n[3]);
        }
    }

    #[test]
    fn a_flat_normal_map_keeps_full_length() {
        let chain = mip_chain(&solid(8, 8, [128, 128, 255, 0]), 8, 8, TexelKind::Normal);
        assert!(chain
            .iter()
            .all(|l| l.texels.chunks(4).all(|t| t[3] == 255)));
    }
}
