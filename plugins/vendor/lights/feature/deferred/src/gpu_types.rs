//! GPU reflection capture types.

use bytemuck::{Pod, Zeroable};

/// Influence volume shape for a reflection capture.
///
/// Both shapes are parallax-corrected: the reflection ray is intersected
/// against the volume so the cubemap appears anchored to the room rather
/// than infinitely far away.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionCaptureShape {
    /// Radial influence, faded over the outer 10% of `influence_radius`.
    Sphere = 0,
    /// Oriented box influence, faded over `transition_distance` from each face.
    Box = 1,
}

/// How a capture's cubemap pixels are produced.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionCaptureMobility {
    /// Pre-filtered offline by the probe baker. Never re-rendered at runtime.
    Static = 0,
    /// Re-rendered at runtime rather than baked.
    ///
    /// TODO: not implemented — dynamic captures are inert. They are never
    /// assigned a cubemap layer, so `cubemap_index` stays -1 and the shader
    /// skips them. Do not hand one a layer without also adding the runtime
    /// array and a `mobility` branch to the sampling code: layer indices are
    /// per-array, so a dynamic capture holding an index today would read the
    /// *baked* array and silently show the wrong probe.
    ///
    /// Intended semantics when implemented: a dynamic capture scopes realtime
    /// lighting — it bounds where whatever realtime lighting is currently
    /// active takes effect, and which objects that lighting considers. It is
    /// deliberately not tied to one lighting system. A scene with no dynamic
    /// captures applies realtime lighting everywhere. Spatial partitioning
    /// applies either way; the capture set bounds the work rather than
    /// replacing the partition.
    Dynamic = 1,
}

/// Per-capture GPU data. 112 bytes.
///
/// Captures are uploaded sorted by influence volume, smallest first, so the
/// shader can blend them front-to-back and let a small capture override the
/// larger one it sits inside.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuReflectionCapture {
///     position_radius:    vec4<f32>,   // xyz = world position, w = influence radius
///     extents_transition: vec4<f32>,   // xyz = local half-extents (box), w = transition distance
///     world_to_local:     mat4x4<f32>, // box parallax; identity for spheres
///     cubemap_index:      i32,         // cube array layer, -1 = no cubemap
///     shape:              u32,         // ReflectionCaptureShape
///     mobility:           u32,         // ReflectionCaptureMobility
///     brightness:         f32,
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuReflectionCapture {
    /// xyz = world position, w = influence radius.
    pub position_radius: [f32; 4],
    /// xyz = local half-extents (box only), w = transition distance.
    pub extents_transition: [f32; 4],
    /// World → capture-local transform, used for oriented-box parallax.
    /// Identity for spheres, which parallax-correct in world space.
    pub world_to_local: [[f32; 4]; 4],
    /// Layer in the reflection cube array, or -1 when no cubemap is resident.
    pub cubemap_index: i32,
    /// [`ReflectionCaptureShape`] as u32.
    pub shape: u32,
    /// [`ReflectionCaptureMobility`] as u32.
    pub mobility: u32,
    /// Linear multiplier applied to the sampled radiance.
    pub brightness: f32,
}

impl GpuReflectionCapture {
    /// A capture that contributes nothing, used to pad unused slots.
    pub fn disabled() -> Self {
        Self {
            position_radius: [0.0; 4],
            extents_transition: [0.0; 4],
            world_to_local: [[0.0; 4]; 4],
            cubemap_index: -1,
            shape: ReflectionCaptureShape::Sphere as u32,
            mobility: ReflectionCaptureMobility::Static as u32,
            brightness: 0.0,
        }
    }

    /// Relative size of the influence volume, used to order captures so the
    /// smallest (most specific) capture is blended last and therefore wins.
    pub fn influence_size(&self) -> f32 {
        match self.shape {
            x if x == ReflectionCaptureShape::Box as u32 => {
                let e = self.extents_transition;
                // Sphere-equivalent radius of the box, so both shapes sort on
                // a comparable scale.
                (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt()
            }
            _ => self.position_radius[3],
        }
    }
}
