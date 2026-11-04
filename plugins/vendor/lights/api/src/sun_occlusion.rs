//! The sun's occlusion by terrain that does not draw into the shadow maps
//! (voxel terrain, which is traced, not rasterised): a virtual clipmap of
//! orthographic depth pages along the sun, as Unreal's virtual shadow maps
//! page a directional light. Level k's texels are 2^k of the finest wide,
//! each level a window of pages around the eye; only pages a reader asked
//! for are traced, into physical pages of one atlas, and kept until the
//! terrain under them changes or the sun turns. A terrain pass renders it
//! and publishes [`SUN_OCCLUSION_MAPS`] (the atlas: `R32Float`, each texel
//! the distance along the sun from its page's near plane to the first
//! occluder, 0 where there is none), [`SUN_OCCLUSION_PAGES`] (the page
//! tables, then each physical page's near plane) and [`SUN_OCCLUSION_FRAME`]
//! (a [`SunOcclusionFrame`] uniform). Passes lighting anything the terrain
//! should shade from the sun (volumetric fog, GI hits, ...) opt into
//! [`SUN_OCCLUSION_SNIPPET`] (`//!use sun_occlusion`) and multiply the sun
//! by `sun_occlusion(p, n, footprint)`: the level whose texel is about the
//! footprint they resolve, or the nearest coarser one resident.

use bytemuck::{Pod, Zeroable};

/// Graph key of the atlas of physical pages (`R32Float` 2D texture).
pub const SUN_OCCLUSION_MAPS: &str = "sun_occlusion_maps";
/// Graph key of the page tables and near planes (storage buffer of words:
/// [`SUN_OCCLUSION_TABLE_WORDS`] entries, then a near plane a physical page).
pub const SUN_OCCLUSION_PAGES: &str = "sun_occlusion_pages";
/// Graph key of the frame (uniform buffer, [`SunOcclusionFrame`]).
pub const SUN_OCCLUSION_FRAME: &str = "sun_occlusion_frame";
/// Levels of the clipmap: from the finest texel up 2^19 times.
pub const SUN_OCCLUSION_LEVELS: usize = 20;
/// Epochs read at once: the current sun's, and the last one's until the
/// current has its own pages.
pub const SUN_OCCLUSION_EPOCHS: usize = 2;
/// Texels along a page's side.
pub const SUN_OCCLUSION_PAGE: u32 = 1 << 6;
/// Pages along a level's window side (centred on the eye's page).
pub const SUN_OCCLUSION_WINDOW: u32 = 1 << 7;
/// Physical pages along the atlas's side (the atlas is this many pages
/// square, `SUN_OCCLUSION_ATLAS_PAGES^2` in all).
pub const SUN_OCCLUSION_ATLAS_PAGES: u32 = 64;
/// Texels along the atlas's side.
pub const SUN_OCCLUSION_ATLAS_SIDE: u32 = SUN_OCCLUSION_ATLAS_PAGES * SUN_OCCLUSION_PAGE;
/// Physical pages.
pub const SUN_OCCLUSION_PHYSICAL: u32 = SUN_OCCLUSION_ATLAS_PAGES * SUN_OCCLUSION_ATLAS_PAGES;
/// Words of the page tables: an entry a window page, level and epoch.
pub const SUN_OCCLUSION_TABLE_WORDS: u32 = (SUN_OCCLUSION_EPOCHS * SUN_OCCLUSION_LEVELS) as u32
    * SUN_OCCLUSION_WINDOW
    * SUN_OCCLUSION_WINDOW;
/// Bytes of [`SUN_OCCLUSION_PAGES`].
pub const SUN_OCCLUSION_PAGES_BYTES: u64 =
    (SUN_OCCLUSION_TABLE_WORDS as u64 + SUN_OCCLUSION_PHYSICAL as u64) * 4;
/// Texel format of the atlas.
pub const SUN_OCCLUSION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// Marker opting a shader into [`SUN_OCCLUSION_SNIPPET`].
pub const SUN_OCCLUSION_MARKER: &str = "//!use sun_occlusion";
/// `SunOcclusionFrame` and `sun_occlusion(p, n, footprint)`
/// (`sun_occlusion.wgsl`).
pub const SUN_OCCLUSION_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(SUN_OCCLUSION_MARKER, "sun_occlusion.wgsl");

/// A level of a [`SunOcclusionEpoch`]: the eye's texel in it, counted from
/// the epoch's anchor along right and down, as a whole texel and the
/// fraction past it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SunOcclusionLevel {
    pub texel: [i32; 2],
    pub frac: [f32; 2],
}

/// An epoch of [`SunOcclusionFrame`]: the sun its pages were traced for, as
/// their grid's axes (right and down across, forward from the sun; right's
/// w its page table's slot + 1, 0 for no epoch), and the eye in each level.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SunOcclusionEpoch {
    pub right: [f32; 4],
    pub down: [f32; 4],
    pub forward: [f32; 4],
    pub levels: [SunOcclusionLevel; SUN_OCCLUSION_LEVELS],
}

/// `SunOcclusionFrame` (`sun_occlusion.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SunOcclusionFrame {
    /// x: the finest texel (m; 0: none published), y: the levels, z:
    /// texels along a page, w: pages along a window.
    pub info: [f32; 4],
    /// x: physical pages along the atlas's side, y: the first near plane's
    /// word in [`SUN_OCCLUSION_PAGES`].
    pub atlas: [u32; 4],
    /// The current epoch, then the last one.
    pub epochs: [SunOcclusionEpoch; SUN_OCCLUSION_EPOCHS],
}

pub const SUN_OCCLUSION_FRAME_BYTES: u64 = std::mem::size_of::<SunOcclusionFrame>() as u64;
const _: () = assert!(SUN_OCCLUSION_FRAME_BYTES == 768);

/// The table word of `page` (level coordinates) of a level in table `slot`:
/// where `sun_occlusion_physical` looks it up.
pub fn sun_occlusion_table_index(slot: usize, level: usize, page: [i32; 2]) -> usize {
    let w = SUN_OCCLUSION_WINDOW as i32;
    ((slot * SUN_OCCLUSION_LEVELS + level) * SUN_OCCLUSION_WINDOW as usize
        + page[1].rem_euclid(w) as usize)
        * SUN_OCCLUSION_WINDOW as usize
        + page[0].rem_euclid(w) as usize
}

/// A table entry: physical page `physical` holds `page` (the bits above the
/// window's tell windows apart).
pub fn sun_occlusion_entry(page: [i32; 2], physical: u32) -> u32 {
    let shift = SUN_OCCLUSION_WINDOW.trailing_zeros();
    let tag = |c: i32| ((c >> shift) as u32) & 511;
    (physical + 1) | (tag(page[0]) << 13) | (tag(page[1]) << 22)
}

/// The physical page an entry holds `page` in, if it does.
pub fn sun_occlusion_entry_physical(entry: u32, page: [i32; 2]) -> Option<u32> {
    let physical = entry & 0x1fff;
    (physical != 0 && sun_occlusion_entry(page, physical - 1) == entry).then(|| physical - 1)
}

/// The level whose texel is the largest no wider than `footprint` (m), of
/// `levels` from texel `finest` (`sun_occlusion_level`).
pub fn sun_occlusion_level(finest: f32, levels: usize, footprint: f32) -> usize {
    (footprint.max(1e-6) / finest)
        .log2()
        .floor()
        .clamp(0.0, levels as f32 - 1.0) as usize
}

/// The sun's occlusion as published, read back (diagnostics and tests):
/// the frame, the pages buffer's words and the atlas's texels (rows of
/// [`SUN_OCCLUSION_ATLAS_SIDE`]). [`Self::read`] reads it as the shaders do.
pub struct SunOcclusionSnapshot {
    pub frame: SunOcclusionFrame,
    pub pages: Vec<u32>,
    pub atlas: Vec<f32>,
}

/// What [`SunOcclusionSnapshot::read`] found.
#[derive(Clone, Copy, Debug)]
pub struct SunOcclusionRead {
    /// The sun's share (the 2x2 filter's).
    pub lit: f32,
    /// The level read, its texel (m), and the epoch.
    pub level: usize,
    pub texel: f32,
    pub epoch: usize,
    /// The receiver's distance from the near plane, and the nearest tap's
    /// stored occluder distance (0: none).
    pub depth: f32,
    pub stored: f32,
    /// Where in the nearest texel the point lies (0..1 across it).
    pub frac: [f32; 2],
    /// The epoch's direction to the sun.
    pub sun: [f32; 3],
}

impl SunOcclusionSnapshot {
    /// As `sun_occlusion_at`: the sun's share at camera-relative `p`
    /// (receiver normal `n`, footprint m) from the first resident level from
    /// the footprint's up; none where no page holds it.
    pub fn read(&self, p: [f32; 3], n: [f32; 3], footprint: f32) -> Option<SunOcclusionRead> {
        let finest = self.frame.info[0];
        let levels = self.frame.info[1] as usize;
        if finest <= 0.0 || levels == 0 {
            return None;
        }
        let want = sun_occlusion_level(finest, levels, footprint);
        (want..levels.min(SUN_OCCLUSION_LEVELS)).find_map(|level| {
            (0..SUN_OCCLUSION_EPOCHS)
                .filter(|&e| self.frame.epochs[e].right[3] != 0.0)
                .find_map(|e| self.read_level(p, n, e, level))
        })
    }

    /// `sun_occlusion_read_level`: level `level` of epoch `epoch`, if its
    /// pages there are resident.
    pub fn read_level(
        &self,
        p: [f32; 3],
        n: [f32; 3],
        epoch: usize,
        level: usize,
    ) -> Option<SunOcclusionRead> {
        let dot = |a: [f32; 4], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let ep = &self.frame.epochs[epoch];
        let l = ep.levels[level];
        let texel = self.frame.info[0] * (level as f32).exp2();
        let c = [
            dot(ep.right, p) / texel + l.frac[0],
            dot(ep.down, p) / texel + l.frac[1],
        ];
        let whole = c.map(f32::floor);
        let frac = [c[0] - whole[0], c[1] - whole[1]];
        let at = [l.texel[0] + whole[0] as i32, l.texel[1] + whole[1] as i32];
        let shift = frac.map(|f| if f < 0.5 { -1 } else { 0 });
        let base = [at[0] + shift[0], at[1] + shift[1]];
        let f = [
            frac[0] - 0.5 - shift[0] as f32,
            frac[1] - 0.5 - shift[1] as f32,
        ];
        let page_shift = SUN_OCCLUSION_PAGE.trailing_zeros();
        let eye = l.texel.map(|t| t >> page_shift);
        let toward = dot(ep.forward, n);
        let slope = if toward < -0.05 {
            [
                (-dot(ep.right, n) / toward).clamp(-4.0, 4.0),
                (-dot(ep.down, n) / toward).clamp(-4.0, 4.0),
            ]
        } else {
            [0.0; 2]
        };
        let bias = texel * (1.0 + 0.5 * (slope[0].abs() + slope[1].abs()));
        let depth = dot(ep.forward, p);
        let half = (SUN_OCCLUSION_WINDOW / 2) as i32;
        let mut lit = [0.0f32; 4];
        let (mut nearest, mut receiver_depth) = (0.0, 0.0);
        for (k, out) in lit.iter_mut().enumerate() {
            let tap = [(k & 1) as i32, (k >> 1) as i32];
            let t = [base[0] + tap[0], base[1] + tap[1]];
            let page = t.map(|t| t >> page_shift);
            if (page[0] - eye[0]).abs() >= half || (page[1] - eye[1]).abs() >= half {
                return None;
            }
            let slot = ep.right[3] as usize - 1;
            let entry = self.pages[sun_occlusion_table_index(slot, level, page)];
            let physical = sun_occlusion_entry_physical(entry, page)?;
            let side = SUN_OCCLUSION_ATLAS_PAGES;
            let x = (physical % side) * SUN_OCCLUSION_PAGE
                + (t[0] & (SUN_OCCLUSION_PAGE as i32 - 1)) as u32;
            let y = (physical / side) * SUN_OCCLUSION_PAGE
                + (t[1] & (SUN_OCCLUSION_PAGE as i32 - 1)) as u32;
            let stored = self.atlas[(y * SUN_OCCLUSION_ATLAS_SIDE + x) as usize];
            let near = f32::from_bits(self.pages[(self.frame.atlas[1] + physical) as usize]);
            let receiver = depth - near
                + ((tap[0] as f32 - f[0]) * slope[0] + (tap[1] as f32 - f[1]) * slope[1]) * texel;
            *out = if stored <= 0.0 || receiver <= stored + bias {
                1.0
            } else {
                0.0
            };
            if tap[0] as f32 == f[0].round() && tap[1] as f32 == f[1].round() {
                nearest = stored;
                receiver_depth = depth - near;
            }
        }
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Some(SunOcclusionRead {
            lit: mix(mix(lit[0], lit[1], f[0]), mix(lit[2], lit[3], f[0]), f[1]),
            level,
            texel,
            epoch,
            depth: receiver_depth,
            stored: nearest,
            frac,
            sun: [-ep.forward[0], -ep.forward[1], -ep.forward[2]],
        })
    }
}
