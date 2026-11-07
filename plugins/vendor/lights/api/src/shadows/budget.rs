//! Bounded shadow residency and atlas configuration. CPU loops are bounded by
//! the resident pool, never by SceneDB's light count.
use bytemuck::{Pod, Zeroable};

pub const MAX_SHADOW_CASTERS: usize = 256;
pub const MAX_SHADOW_FACES: usize = MAX_SHADOW_CASTERS * 6;
pub const MIN_TILE_SIZE: u32 = 128;

#[derive(Debug, Clone, Copy)]
pub struct ShadowBudget {
    /// Both Depth32Float atlases and half-resolution RGBA16F transmission.
    pub memory_bytes: u64,
    /// Maximum atlas face updates in one frame.
    pub updates_per_frame: u32,
    /// Sum of updated tile areas. Static and dynamic updates each count.
    pub update_texels_per_frame: u32,
    pub max_resolution: u32,
    pub fade_frames: u32,
    pub max_distance: f32,
    pub hysteresis: f32,
    /// Page-level invalidation (a quality tier). A moving shadow caster dirties
    /// only the 8x8 pages of a face's dynamic layer that its old and new
    /// bounds touch; the dynamic layer clears and redraws just those pages,
    /// and casters that touch none of them are skipped. Off, a moved caster
    /// redraws every page of each face it reaches. Cheap to track, so on by
    /// default; turn it off for targets where the extra vertex work per
    /// caster is not worth the saved fill.
    pub page_invalidation: bool,
}
impl Default for ShadowBudget {
    fn default() -> Self {
        Self {
            memory_bytes: 160 * 1024 * 1024,
            updates_per_frame: 32,
            update_texels_per_frame: 4 * 1024 * 1024,
            max_resolution: 2048,
            fade_frames: 12,
            max_distance: 500.0,
            hysteresis: 0.2,
            page_invalidation: true,
        }
    }
}
impl ShadowBudget {
    pub fn atlas_size(self, device_limit: u32) -> u32 {
        // Eight depth bytes plus two amortized transmission bytes per texel.
        let mut size = MIN_TILE_SIZE;
        while size * 2 <= device_limit.min(8192)
            && u64::from(size * 2).pow(2) * 10 <= self.memory_bytes
        {
            size *= 2;
        }
        size.min(device_limit).max(1)
    }
    pub fn validate(self) -> std::result::Result<Self, &'static str> {
        if self.memory_bytes < 10 * u64::from(MIN_TILE_SIZE).pow(2) {
            return Err("shadow memory budget must hold a 128px tile in each atlas");
        }
        if self.updates_per_frame < 3
            || self.update_texels_per_frame < 3 * MIN_TILE_SIZE * MIN_TILE_SIZE
        {
            return Err("shadow update budget must allow at least one 128px tile across depth and transmission");
        }
        if !self.max_resolution.is_power_of_two() || !(128..=2048).contains(&self.max_resolution) {
            return Err("shadow maximum resolution must be a power of two from 128 through 2048");
        }
        if !self.max_distance.is_finite() || self.max_distance <= 0.0 {
            return Err("shadow distance must be finite and positive");
        }
        if !self.hysteresis.is_finite() || !(0.0..=1.0).contains(&self.hysteresis) {
            return Err("shadow hysteresis must be between zero and one");
        }
        Ok(self)
    }
}

/// Coordinates in the shared depth atlas. A zero size means no residency.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct ShadowTile {
    pub x: u32,
    pub y: u32,
    pub size: u32,
    pub valid: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct ShadowResident {
    /// SceneDB row plus one; zero is vacant.
    pub owner: u32,
    pub light_type: u32,
    pub flags: u32,
    pub target: u32,
    pub score: f32,
    pub strength: f32,
    pub resolution: u32,
    pub hash: u32,
    pub tiles: [ShadowTile; 6],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ResidencyTable {
    pub header: [u32; 4],
    pub residents: [ShadowResident; MAX_SHADOW_CASTERS],
}
impl Default for ResidencyTable {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// The lights' shadow settings (a renderer config's `features`): the atlas
/// budget the shadow views are fitted into and the filtering quality.
#[derive(Clone, Copy, Debug)]
pub struct ShadowSettings {
    pub budget: ShadowBudget,
    pub quality: crate::ShadowQuality,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            budget: ShadowBudget::default(),
            quality: crate::ShadowQuality::Medium,
        }
    }
}

impl ShadowSettings {
    /// The validated budget and the atlas side it gives on a device whose
    /// largest 2D texture is `device_limit` texels.
    pub fn atlas(&self, device_limit: u32) -> Result<(ShadowBudget, u32), String> {
        let budget = self
            .budget
            .validate()
            .map_err(|_| "invalid shadow budget".to_string())?;
        Ok((budget, budget.atlas_size(device_limit)))
    }
}
