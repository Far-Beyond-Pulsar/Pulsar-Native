//! Cooked textures: the form an image takes before it reaches the GPU.
//!
//! A source image is decoded, capped in size, given a full mip chain and, where
//! the device can sample it, block-compressed to BC7. All of that is expensive
//! and depends only on the source and a few settings, so the result is one
//! byte blob that SceneDB's derived-data cache stores on disk
//! ([`pulsar_scenedb::derive_cached`]) and its residency cache keeps in memory
//! ([`pulsar_scenedb::Residency`]).
//!
//! The sizes that follow for a 2048 x 2048 image:
//!
//! | form | bytes |
//! |---|---|
//! | RGBA8, one level (what we uploaded before) | 16.0 MiB |
//! | RGBA8 with mips | 21.3 MiB |
//! | BC7 with mips | 5.3 MiB |
//!
//! # What a texture is for
//!
//! The same image can be sampled as a colour or as data, and the two are not
//! the same thing to the GPU. Colour is stored gamma-encoded (sRGB), so the
//! sampler decodes it to linear light on read, and its mips are averaged in
//! linear light. A roughness, occlusion or normal map holds numbers, not light:
//! decoding it as sRGB bends every value, and averaging it as light darkens it.
//! [`TextureUse`] names which one a texture is, and it picks the GPU format's
//! colour space and the mip filter ([`helio_mats::texture_mips`]). A normal
//! map's mips keep the length of their mean normal in alpha, which shaders
//! turn into roughness (specular anti-aliasing).
//!
//! # Container
//!
//! `"PTX2"`, format code, use code, width, height, level count and each level's
//! byte length (all little-endian `u32`), then the levels, largest first. A
//! level is tightly packed rows of texels (RGBA8) or of 4 x 4 blocks (BC7);
//! levels smaller than a block round up to one. [`CookedTexture::from_bytes`]
//! checks every length against what the format and dimensions require, so a
//! damaged or outdated file is refused instead of uploaded.

use helio_mats::texture_mips::{mip_chain, TexelKind};
use intel_tex_2::{bc7, RgbaSurface};

const MAGIC: [u8; 4] = *b"PTX2";
const HEADER_LEN: usize = 24;

/// The version of what [`cook`] produces. Part of the derived-data key: bump
/// it when a change here alters the bytes for the same input.
pub const COOK_VERSION: u32 = 3;

/// How a cooked texture's texels are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CookFormat {
    /// Uncompressed RGBA8: works on every device.
    Rgba8,
    /// Block-compressed BC7: a quarter of the size, needs the
    /// `TEXTURE_COMPRESSION_BC` feature and a width and height divisible by 4.
    Bc7,
}

impl CookFormat {
    pub const ALL: [CookFormat; 2] = [CookFormat::Rgba8, CookFormat::Bc7];

    fn code(self) -> u32 {
        match self {
            CookFormat::Rgba8 => 0,
            CookFormat::Bc7 => 1,
        }
    }

    fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(CookFormat::Rgba8),
            1 => Some(CookFormat::Bc7),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CookFormat::Rgba8 => "rgba8",
            CookFormat::Bc7 => "bc7",
        }
    }

    /// The best format `features` can sample for an image of this size.
    pub fn best_for(features: wgpu::Features, width: u32, height: u32) -> Self {
        if features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
            && width % 4 == 0
            && height % 4 == 0
        {
            CookFormat::Bc7
        } else {
            CookFormat::Rgba8
        }
    }
}

/// What a texture is sampled as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureUse {
    /// A colour: base colour, emissive, a sprite. Stored as sRGB.
    Color,
    /// Numbers in the channels: roughness, metallic, occlusion, opacity.
    /// Stored linear, mips averaged as plain numbers.
    Data,
    /// A tangent-space normal map. Stored linear; each mip level's
    /// direction is made unit again, and its alpha holds the mean normal's
    /// length.
    Normal,
}

impl TextureUse {
    pub const ALL: [TextureUse; 3] = [TextureUse::Color, TextureUse::Data, TextureUse::Normal];

    fn code(self) -> u32 {
        match self {
            TextureUse::Color => 0,
            TextureUse::Data => 1,
            TextureUse::Normal => 2,
        }
    }

    fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(TextureUse::Color),
            1 => Some(TextureUse::Data),
            2 => Some(TextureUse::Normal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TextureUse::Color => "color",
            TextureUse::Data => "data",
            TextureUse::Normal => "normal",
        }
    }

    /// Whether the GPU should decode the texels as sRGB on read.
    pub fn is_srgb(self) -> bool {
        self.texels().is_srgb()
    }

    /// How this use's mips are filtered.
    pub fn texels(self) -> TexelKind {
        match self {
            TextureUse::Color => TexelKind::Colour,
            TextureUse::Data => TexelKind::Data,
            TextureUse::Normal => TexelKind::Normal,
        }
    }

    /// The use that wins when one texture feeds several: colour keeps the
    /// behaviour every texture had before uses existed.
    pub fn strongest(self, other: TextureUse) -> TextureUse {
        let rank = |u: TextureUse| match u {
            TextureUse::Color => 2,
            TextureUse::Normal => 1,
            TextureUse::Data => 0,
        };
        if rank(other) > rank(self) { other } else { self }
    }
}

/// The GPU format for a cooked texture of `format` used as `usage`.
pub fn wgpu_format(format: CookFormat, usage: TextureUse) -> wgpu::TextureFormat {
    match (format, usage.is_srgb()) {
        (CookFormat::Rgba8, true) => wgpu::TextureFormat::Rgba8UnormSrgb,
        (CookFormat::Rgba8, false) => wgpu::TextureFormat::Rgba8Unorm,
        (CookFormat::Bc7, true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        (CookFormat::Bc7, false) => wgpu::TextureFormat::Bc7RgbaUnorm,
    }
}

pub use helio_mats::texture_mips::mip_level_count;

/// Level `level`'s extent in texels.
fn level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

fn round_up_4(value: u32) -> u32 {
    value.div_ceil(4) * 4
}

/// Bytes of one level of `format` at `(width, height)` texels.
fn level_bytes(format: CookFormat, width: u32, height: u32) -> usize {
    match format {
        CookFormat::Rgba8 => width as usize * height as usize * 4,
        CookFormat::Bc7 => (round_up_4(width) / 4) as usize * (round_up_4(height) / 4) as usize * 16,
    }
}

/// A texture ready to upload: its format, use, size and every mip level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedTexture {
    pub format: CookFormat,
    pub usage: TextureUse,
    pub width: u32,
    pub height: u32,
    levels: Vec<std::ops::Range<usize>>,
    bytes: Vec<u8>,
}

impl CookedTexture {
    pub fn mip_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// The GPU format to create this texture with.
    pub fn wgpu_format(&self) -> wgpu::TextureFormat {
        wgpu_format(self.format, self.usage)
    }

    /// Level `level`'s bytes, in the layout `TextureStore::register_mips`
    /// takes.
    pub fn mip(&self, level: u32) -> &[u8] {
        &self.bytes[self.levels[level as usize].clone()]
    }

    pub fn mips(&self) -> Vec<&[u8]> {
        (0..self.mip_count()).map(|level| self.mip(level)).collect()
    }

    /// Bytes held in memory (what the residency budget counts).
    pub fn byte_size(&self) -> usize {
        self.bytes.len()
    }

    /// This texture without its `drop` largest levels: the same image at a
    /// smaller size, the way a streaming system holds a texture it cannot
    /// afford in full. Dropping is limited so a texture always keeps at least
    /// one level, and a block-compressed one keeps a base level whose sides are
    /// whole blocks (4 texels), which the GPU requires.
    pub fn without_top_mips(&self, drop: u32) -> CookedTexture {
        let mut drop = drop.min(self.mip_count().saturating_sub(1));
        if self.format == CookFormat::Bc7 {
            while drop > 0 && ((self.width >> drop) < 4 || (self.height >> drop) < 4) {
                drop -= 1;
            }
        }
        if drop == 0 {
            return self.clone();
        }
        let (width, height) = level_extent(self.width, self.height, drop);
        let levels = (drop..self.mip_count()).map(|level| self.mip(level).to_vec()).collect();
        Self::from_levels(self.format, self.usage, width, height, levels)
    }

    /// Bytes this texture would take on the GPU after dropping `drop` levels.
    pub fn byte_size_without_top_mips(&self, drop: u32) -> usize {
        self.without_top_mips(drop).byte_size()
    }

    fn from_levels(
        format: CookFormat,
        usage: TextureUse,
        width: u32,
        height: u32,
        levels: Vec<Vec<u8>>,
    ) -> Self {
        let mut ranges = Vec::with_capacity(levels.len());
        let mut bytes = Vec::with_capacity(levels.iter().map(Vec::len).sum());
        for level in &levels {
            ranges.push(bytes.len()..bytes.len() + level.len());
            bytes.extend_from_slice(level);
        }
        Self { format, usage, width, height, levels: ranges, bytes }
    }

    /// The container (see the module docs).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 4 * self.levels.len() + self.bytes.len());
        out.extend_from_slice(&MAGIC);
        for value in [
            self.format.code(),
            self.usage.code(),
            self.width,
            self.height,
            self.mip_count(),
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for level in &self.levels {
            out.extend_from_slice(&((level.end - level.start) as u32).to_le_bytes());
        }
        out.extend_from_slice(&self.bytes);
        out
    }

    /// Parse the container, refusing anything whose lengths do not match what
    /// its format and size require.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let word = |at: usize| -> Result<u32, String> {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
                .ok_or_else(|| "cooked texture is truncated".to_string())
        };
        if bytes.get(..4) != Some(&MAGIC) {
            return Err("not a cooked texture".into());
        }
        let format = CookFormat::from_code(word(4)?)
            .ok_or_else(|| "cooked texture has an unknown format".to_string())?;
        let usage = TextureUse::from_code(word(8)?)
            .ok_or_else(|| "cooked texture has an unknown use".to_string())?;
        let (width, height, count) = (word(12)?, word(16)?, word(20)?);
        if width == 0 || height == 0 || count != mip_level_count(width, height) {
            return Err("cooked texture has an impossible size".into());
        }
        let data_start = HEADER_LEN + 4 * count as usize;
        let mut offset = data_start;
        let mut levels = Vec::with_capacity(count as usize);
        for level in 0..count {
            let length = word(HEADER_LEN + 4 * level as usize)? as usize;
            let (lw, lh) = level_extent(width, height, level);
            if length != level_bytes(format, lw, lh) {
                return Err(format!("cooked texture level {level} has the wrong length"));
            }
            levels.push(offset - data_start..offset - data_start + length);
            offset += length;
        }
        if bytes.len() != offset {
            return Err("cooked texture has the wrong total length".into());
        }
        Ok(Self {
            format,
            usage,
            width,
            height,
            levels,
            bytes: bytes[data_start..].to_vec(),
        })
    }
}

/// `rgba` (`width` x `height`) padded to a multiple of 4 in both directions by
/// repeating its last row and column, so a partial block encodes cleanly.
fn pad_to_blocks(rgba: &[u8], width: u32, height: u32) -> (u32, u32, Vec<u8>) {
    let (padded_width, padded_height) = (round_up_4(width), round_up_4(height));
    if (padded_width, padded_height) == (width, height) {
        return (width, height, rgba.to_vec());
    }
    let mut out = Vec::with_capacity(padded_width as usize * padded_height as usize * 4);
    for y in 0..padded_height {
        let sy = y.min(height - 1) as usize;
        for x in 0..padded_width {
            let sx = x.min(width - 1) as usize;
            let at = (sy * width as usize + sx) * 4;
            out.extend_from_slice(&rgba[at..at + 4]);
        }
    }
    (padded_width, padded_height, out)
}

fn compress_bc7(rgba: &[u8], width: u32, height: u32, opaque: bool) -> Vec<u8> {
    let (padded_width, padded_height, padded) = pad_to_blocks(rgba, width, height);
    let surface = RgbaSurface {
        data: &padded,
        width: padded_width,
        height: padded_height,
        stride: padded_width * 4,
    };
    let settings = if opaque {
        bc7::opaque_fast_settings()
    } else {
        bc7::alpha_fast_settings()
    };
    bc7::compress_blocks(&settings, &surface)
}

/// Cook a colour texture: [`cook_as`] with [`TextureUse::Color`].
pub fn cook(rgba: &[u8], width: u32, height: u32, format: CookFormat) -> Result<CookedTexture, String> {
    cook_as(rgba, width, height, format, TextureUse::Color)
}

/// Cook `rgba` (straight-alpha RGBA8, `width` x `height`) into a full mip chain
/// in `format`, filtered as `usage` says. BC7 needs a width and height
/// divisible by 4; ask [`CookFormat::best_for`] when the caller is not sure.
pub fn cook_as(
    rgba: &[u8],
    width: u32,
    height: u32,
    format: CookFormat,
    usage: TextureUse,
) -> Result<CookedTexture, String> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return Err("cook was given a buffer that does not match its size".into());
    }
    if format == CookFormat::Bc7 && (width % 4 != 0 || height % 4 != 0) {
        return Err("BC7 needs a width and height divisible by 4".into());
    }
    let levels = mip_chain(rgba, width, height, usage.texels())
        .into_iter()
        .map(|level| match format {
            CookFormat::Rgba8 => level.texels,
            CookFormat::Bc7 => {
                // A normal map's mips are opaque only where its normals agree.
                let opaque = level.texels.chunks_exact(4).all(|texel| texel[3] == 255);
                compress_bc7(&level.texels, level.width, level.height, opaque)
            }
        })
        .collect();
    Ok(CookedTexture::from_levels(format, usage, width, height, levels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, texel: [u8; 4]) -> Vec<u8> {
        (0..width * height).flat_map(|_| texel).collect()
    }

    #[test]
    fn mip_chains_run_down_to_one_texel() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(8, 4), 4);
        assert_eq!(mip_level_count(2048, 2048), 12);
        assert_eq!(mip_level_count(5, 3), 3);
    }

    #[test]
    fn an_rgba8_chain_has_every_level_at_its_size() {
        let cooked = cook(&solid(8, 4, [10, 20, 30, 255]), 8, 4, CookFormat::Rgba8).unwrap();
        assert_eq!(cooked.mip_count(), 4);
        let sizes: Vec<usize> = (0..4).map(|l| cooked.mip(l).len()).collect();
        assert_eq!(sizes, [8 * 4 * 4, 4 * 2 * 4, 2 * 1 * 4, 1 * 1 * 4]);
        assert_eq!(cooked.mip(3), &[10, 20, 30, 255], "a flat image stays flat");
    }

    #[test]
    fn dropping_top_mips_keeps_the_lower_levels_unchanged() {
        let cooked = cook(&solid(16, 8, [10, 20, 30, 255]), 16, 8, CookFormat::Rgba8).unwrap();
        let smaller = cooked.without_top_mips(2);
        assert_eq!((smaller.width, smaller.height), (4, 2));
        assert_eq!(smaller.mip_count(), cooked.mip_count() - 2);
        for level in 0..smaller.mip_count() {
            assert_eq!(smaller.mip(level), cooked.mip(level + 2), "level {level}");
        }
        assert_eq!(cooked.without_top_mips(0), cooked, "dropping nothing changes nothing");
    }

    #[test]
    fn dropping_never_removes_the_last_level_or_breaks_block_alignment() {
        let rgba = cook(&solid(8, 8, [1, 2, 3, 255]), 8, 8, CookFormat::Rgba8).unwrap();
        let one = rgba.without_top_mips(99);
        assert_eq!((one.width, one.height, one.mip_count()), (1, 1, 1));

        let bc7 = cook(&solid(16, 16, [1, 2, 3, 255]), 16, 16, CookFormat::Bc7).unwrap();
        let smallest = bc7.without_top_mips(99);
        assert_eq!((smallest.width, smallest.height), (4, 4), "BC7 keeps a whole-block base");
        assert_eq!(smallest.mip_count(), 3);
        assert!(bc7.byte_size_without_top_mips(1) < bc7.byte_size());
    }

    #[test]
    fn levels_are_the_shared_mip_chain_filtered_as_their_use() {
        // A normal map whose texels lean four ways: its mips carry the
        // spread in alpha, and colour, data and normal filter differently.
        let texels: Vec<u8> = (0..64).flat_map(|i| [[230, 128, 230, 255], [25, 128, 230, 255]][i % 2]).collect();
        for usage in TextureUse::ALL {
            let cooked = cook_as(&texels, 8, 8, CookFormat::Rgba8, usage).unwrap();
            let chain = mip_chain(&texels, 8, 8, usage.texels());
            for level in 0..cooked.mip_count() {
                assert_eq!(cooked.mip(level), chain[level as usize].texels.as_slice(), "{usage:?} level {level}");
            }
        }
        let normal = cook_as(&texels, 8, 8, CookFormat::Rgba8, TextureUse::Normal).unwrap();
        assert!(normal.mip(1)[3] < 200, "spread kept in alpha: {}", normal.mip(1)[3]);
    }

    #[test]
    fn the_use_picks_the_gpu_colour_space() {
        assert_eq!(wgpu_format(CookFormat::Rgba8, TextureUse::Color), wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(wgpu_format(CookFormat::Rgba8, TextureUse::Data), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(wgpu_format(CookFormat::Bc7, TextureUse::Color), wgpu::TextureFormat::Bc7RgbaUnormSrgb);
        assert_eq!(wgpu_format(CookFormat::Bc7, TextureUse::Normal), wgpu::TextureFormat::Bc7RgbaUnorm);
    }

    #[test]
    fn colour_wins_when_one_texture_feeds_several_uses() {
        assert_eq!(TextureUse::Data.strongest(TextureUse::Color), TextureUse::Color);
        assert_eq!(TextureUse::Color.strongest(TextureUse::Data), TextureUse::Color);
        assert_eq!(TextureUse::Data.strongest(TextureUse::Normal), TextureUse::Normal);
        assert_eq!(TextureUse::Data.strongest(TextureUse::Data), TextureUse::Data);
    }

    #[test]
    fn odd_sizes_reuse_their_last_row_and_column() {
        let cooked = cook(&solid(5, 3, [200, 100, 50, 255]), 5, 3, CookFormat::Rgba8).unwrap();
        assert_eq!(cooked.mip_count(), 3);
        assert_eq!(cooked.mip(1).len(), 2 * 1 * 4);
        assert_eq!(&cooked.mip(2)[..4], &[200, 100, 50, 255]);
    }

    #[test]
    fn bc7_levels_are_whole_blocks_and_small_levels_round_up() {
        let cooked = cook(&solid(16, 16, [80, 160, 240, 255]), 16, 16, CookFormat::Bc7).unwrap();
        let sizes: Vec<usize> = (0..cooked.mip_count()).map(|l| cooked.mip(l).len()).collect();
        // 16x16 = 4x4 blocks, then 2x2, 1, and the 2x2, 1x1 tail each one padded block.
        assert_eq!(sizes, [16 * 16, 4 * 16, 16, 16, 16]);
        assert!(cooked.byte_size() < 16 * 16 * 4 * 2, "BC7 is a quarter of RGBA8, with mips");
    }

    #[test]
    fn bc7_refuses_sizes_that_are_not_whole_blocks() {
        assert!(cook(&solid(6, 8, [1, 2, 3, 255]), 6, 8, CookFormat::Bc7).is_err());
        assert_eq!(CookFormat::best_for(wgpu::Features::TEXTURE_COMPRESSION_BC, 6, 8), CookFormat::Rgba8);
        assert_eq!(CookFormat::best_for(wgpu::Features::TEXTURE_COMPRESSION_BC, 8, 8), CookFormat::Bc7);
        assert_eq!(CookFormat::best_for(wgpu::Features::empty(), 8, 8), CookFormat::Rgba8);
    }

    #[test]
    fn the_container_round_trips_with_its_use_and_refuses_damage() {
        for format in CookFormat::ALL {
            for usage in TextureUse::ALL {
                let cooked = cook_as(&solid(8, 8, [9, 8, 7, 255]), 8, 8, format, usage).unwrap();
                let bytes = cooked.to_bytes();
                let read = CookedTexture::from_bytes(&bytes).unwrap();
                assert_eq!(read, cooked);
                assert_eq!(read.usage, usage);

                assert!(CookedTexture::from_bytes(&bytes[..bytes.len() - 1]).is_err(), "truncated");
                let mut extra = bytes.clone();
                extra.push(0);
                assert!(CookedTexture::from_bytes(&extra).is_err(), "trailing bytes");
                let mut wrong_magic = bytes.clone();
                wrong_magic[0] = b'X';
                assert!(CookedTexture::from_bytes(&wrong_magic).is_err());
                let mut wrong_length = bytes.clone();
                wrong_length[HEADER_LEN] ^= 1; // level 0's recorded length
                assert!(CookedTexture::from_bytes(&wrong_length).is_err());
                let mut wrong_use = bytes.clone();
                wrong_use[8] = 9;
                assert!(CookedTexture::from_bytes(&wrong_use).is_err(), "unknown use");
            }
        }
    }

    #[test]
    fn a_container_from_before_uses_existed_is_refused() {
        // Version 1 had the magic "PTX1" and no use field.
        let mut old = vec![b'P', b'T', b'X', b'1'];
        old.extend_from_slice(&[0; 20]);
        assert!(CookedTexture::from_bytes(&old).is_err());
    }
}
