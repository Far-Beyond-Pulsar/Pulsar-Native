//! GPU shadow-matrix and shadow-quality-configuration types.
//!
//! Shadow matrices are this pass's own per-frame output; shadow quality/CSM
//! configuration is consumed by every pass that shades against shadows
//! (gbuffer, deferred-light, hlfs, volumetric-fog) but this pass is the one
//! that actually computes the cascades, so it owns the shape.

use bytemuck::{Pod, Zeroable};

/// Per-light shadow matrix for the shadow map atlas.
/// Layout: a 64-byte matrix plus 32 bytes of atlas metadata, matching WGSL.
/// 6 consecutive entries per light (indices light_idx*6 .. light_idx*6+5):
///   - Point lights: 6 cube-face view-projection matrices (+X/-X/+Y/-Y/+Z/-Z)
///   - Spot lights:  face 0 = perspective view-proj, faces 1-5 = identity (unused)
///   - Directional:  faces 0-3 = cascades, faces 4-5 = identity (unused)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuShadowMatrix {
    /// Light-space view-projection matrix (64 bytes, matches `LightMatrix { mat: mat4x4<f32> }`)
    pub light_view_proj: [f32; 16],
    /// Atlas offset, scale and faded visibility strength.
    pub atlas: [f32; 4],
    /// Physical layer, author flags, tile resolution, mode (0 legacy, 1 invalid, 2 resident).
    pub policy: [u32; 4],
}

/// Cascade far-plane distances (metres) shared by all passes that read or
/// write CSM data (GBuffer, DeferredLight, HLFS).
///
/// **Must stay in sync** with the WGSL constant `CSM_SPLITS` in
/// `helio-pass-shadow-matrix/shaders/shadow_matrices.wgsl`.
pub const CSM_SPLITS: [f32; 4] = [16.0, 80.0, 300.0, 1400.0];

/// Shadow quality presets for runtime configuration.
///
/// The Vogel disk uses a stable per-pixel hash so noise is static across
/// frames and TAA can accumulate it effectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowQuality {
    /// 8-sample PCF, no PCSS — low-end / mobile
    Low,
    /// 12-sample PCF, no PCSS — mid-tier default
    Medium,
    /// 12-sample PCF + PCSS 8 blocker / 16 filter — high-end PC
    High,
    /// 16-sample PCF + PCSS 16 blocker / 32 filter — cinematic
    Ultra,
}

/// Per-cascade configuration (16 bytes, GPU-aligned).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CascadeConfig {
    /// Far plane distance for this cascade (meters)
    pub split_distance: f32,
    /// Base depth bias for this cascade
    pub depth_bias: f32,
    /// PCF filter radius (texels in shadow atlas)
    pub filter_radius: f32,
    /// PCSS light size (meters, 0.0 = disable PCSS for this cascade)
    pub pcss_light_size: f32,
}

/// Global shadow configuration (96 bytes, GPU uniform).
///
/// Uploaded to GPU as a uniform buffer for runtime shadow quality control.
/// Changes to this config require a single buffer write (delta upload).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ShadowConfig {
    /// Per-cascade settings (4 cascades × 16 bytes = 64 bytes)
    pub cascades: [CascadeConfig; 4],
    /// Global PCSS toggle (0 = PCF only, 1 = PCSS enabled)
    pub enable_pcss: u32,
    /// Blocker search sample count (8-16 recommended)
    pub pcss_blocker_samples: u32,
    /// PCSS filter sample count (16-32 recommended)
    pub pcss_filter_samples: u32,
    /// Standard PCF sample count (non-PCSS path). Quality-driven.
    /// Low=4, Medium=8, High=12, Ultra=16
    pub pcf_sample_count: u32,
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self::from_quality(ShadowQuality::Medium)
    }
}

impl ShadowConfig {
    /// Create shadow configuration from a quality preset.
    pub fn from_quality(quality: ShadowQuality) -> Self {
        match quality {
            ShadowQuality::Low => Self {
                cascades: [
                    CascadeConfig {
                        split_distance: 16.0,
                        depth_bias: 0.00010,
                        filter_radius: 1.0,
                        pcss_light_size: 0.0, // No PCSS
                    },
                    CascadeConfig {
                        split_distance: 80.0,
                        depth_bias: 0.00015,
                        filter_radius: 1.5,
                        pcss_light_size: 0.0,
                    },
                    CascadeConfig {
                        split_distance: 300.0,
                        depth_bias: 0.00020,
                        filter_radius: 2.0,
                        pcss_light_size: 0.0,
                    },
                    CascadeConfig {
                        split_distance: 1400.0,
                        depth_bias: 0.00030,
                        filter_radius: 3.0,
                        pcss_light_size: 0.0,
                    },
                ],
                enable_pcss: 0,
                pcss_blocker_samples: 4,
                pcss_filter_samples: 4,
                pcf_sample_count: 8,
            },
            ShadowQuality::Medium => Self {
                cascades: [
                    CascadeConfig {
                        split_distance: 16.0,
                        depth_bias: 0.00010,
                        filter_radius: 1.5,
                        pcss_light_size: 0.0, // No PCSS
                    },
                    CascadeConfig {
                        split_distance: 80.0,
                        depth_bias: 0.00015,
                        filter_radius: 2.0,
                        pcss_light_size: 0.0,
                    },
                    CascadeConfig {
                        split_distance: 300.0,
                        depth_bias: 0.00020,
                        filter_radius: 2.5,
                        pcss_light_size: 0.0,
                    },
                    CascadeConfig {
                        split_distance: 1400.0,
                        depth_bias: 0.00030,
                        filter_radius: 3.5,
                        pcss_light_size: 0.0,
                    },
                ],
                enable_pcss: 0,
                pcss_blocker_samples: 8,
                pcss_filter_samples: 8,
                pcf_sample_count: 12,
            },
            ShadowQuality::High => Self {
                cascades: [
                    CascadeConfig {
                        split_distance: 16.0,
                        depth_bias: 0.00008,
                        filter_radius: 2.0,
                        pcss_light_size: 2.0, // PCSS enabled
                    },
                    CascadeConfig {
                        split_distance: 80.0,
                        depth_bias: 0.00012,
                        filter_radius: 2.5,
                        pcss_light_size: 4.0,
                    },
                    CascadeConfig {
                        split_distance: 300.0,
                        depth_bias: 0.00018,
                        filter_radius: 3.0,
                        pcss_light_size: 8.0,
                    },
                    CascadeConfig {
                        split_distance: 1400.0,
                        depth_bias: 0.00025,
                        filter_radius: 4.0,
                        pcss_light_size: 16.0,
                    },
                ],
                enable_pcss: 1,
                pcss_blocker_samples: 8,
                pcss_filter_samples: 16,
                pcf_sample_count: 12,
            },
            ShadowQuality::Ultra => Self {
                cascades: [
                    CascadeConfig {
                        split_distance: 16.0,
                        depth_bias: 0.00008,
                        filter_radius: 2.5,
                        pcss_light_size: 2.0, // PCSS enabled
                    },
                    CascadeConfig {
                        split_distance: 80.0,
                        depth_bias: 0.00012,
                        filter_radius: 3.0,
                        pcss_light_size: 4.0,
                    },
                    CascadeConfig {
                        split_distance: 300.0,
                        depth_bias: 0.00018,
                        filter_radius: 3.5,
                        pcss_light_size: 8.0,
                    },
                    CascadeConfig {
                        split_distance: 1400.0,
                        depth_bias: 0.00025,
                        filter_radius: 4.5,
                        pcss_light_size: 16.0,
                    },
                ],
                enable_pcss: 1,
                pcss_blocker_samples: 16,
                pcss_filter_samples: 32,
                pcf_sample_count: 16,
            },
        }
    }

    /// PSSM (Practical Split Scheme) cascade distribution.
    ///
    /// Computes cascade split distances using a blend of uniform and logarithmic
    /// distribution. `lambda = 0.5` is a good balance for most scenes.
    ///
    /// # Arguments
    /// - `near`: Camera near plane (meters)
    /// - `far`: Camera far plane (meters)
    /// - `lambda`: Blend factor (0.0 = uniform, 1.0 = logarithmic)
    pub fn pssm_splits(near: f32, far: f32, lambda: f32) -> [f32; 4] {
        let mut splits = [0.0; 4];
        for i in 0..4 {
            let ratio = (i as f32 + 1.0) / 4.0;
            let c_uniform = near + (far - near) * ratio;
            let c_log = near * (far / near).powf(ratio);
            splits[i] = lambda * c_log + (1.0 - lambda) * c_uniform;
        }
        splits
    }
}

/// Published by ShadowMatrixPass. Desired matrices are used for rendering;
/// committed matrices and metadata are sampled by lighting after tile updates.
#[derive(Clone, Copy)]
pub struct ShadowMatricesFrameData<'a> {
    pub shadow_matrices: &'a wgpu::Buffer,
    pub desired_matrices: Option<&'a wgpu::Buffer>,
    pub residency: Option<&'a crate::ResidencyTable>,
    pub budget: crate::ShadowBudget,
    /// Live shadow-face count this frame.
    pub shadow_count: u32,
    /// Per-resident (256 max) dirty generation counters -- `ShadowPass`
    /// compares against its own last-rendered gen to decide which faces to
    /// re-render.
    pub per_caster_dirty_gen: [u64; crate::MAX_SHADOW_CASTERS],
    /// Increments whenever any movable object moves -- the O(1) CPU gate
    /// `ShadowPass` checks before doing any per-face work.
    pub movable_objects_generation: u64,
    /// Which atlas faces are actually in use, read back from the GPU caster
    /// allocation. `None` until the readback for the current allocation
    /// lands (at startup, and briefly after a light edit); consumers then
    /// treat every face as in use.
    pub caster_layout: Option<CasterLayout>,
}

/// The GPU caster allocation as the CPU sees it: how many casters hold
/// atlas slots and each one's light type. Every caster reserves six faces
/// (`shadow_index = 6 * slot`) but only uses some of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CasterLayout {
    pub caster_count: u32,
    /// `light_type` per slot (0 directional, 1 point, 2 spot); only the
    /// first `caster_count` entries are meaningful.
    pub light_types: [u32; crate::MAX_SHADOW_CASTERS],
}

impl CasterLayout {
    /// Faces of its six a caster renders and the lighting shaders sample:
    /// four cascades for a directional light, one for a spot light, six
    /// cube faces for a point light. Unknown types keep all six.
    pub fn faces_used(&self, slot: usize) -> usize {
        match self.light_types.get(slot).copied() {
            Some(0) => 4,
            Some(2) => 1,
            _ => 6,
        }
    }

    /// Whether the caster's matrices follow the camera (directional
    /// cascades are fitted to the view; point and spot matrices are not).
    pub fn follows_camera(&self, slot: usize) -> bool {
        self.light_types.get(slot).copied() == Some(0)
    }

    /// Atlas face indices in use, in ascending order.
    pub fn active_faces(&self, face_count: usize) -> impl Iterator<Item = usize> + '_ {
        (0..(self.caster_count as usize).min(crate::MAX_SHADOW_CASTERS)).flat_map(move |slot| {
            (0..self.faces_used(slot))
                .map(move |face| slot * 6 + face)
                .filter(move |&face| face < face_count)
        })
    }
}
