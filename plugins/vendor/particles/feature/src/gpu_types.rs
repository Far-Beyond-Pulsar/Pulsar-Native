//! GPU data types for the Corona GPU particle system.
//!
//! All types are `#[repr(C)]` with `bytemuck::Pod + Zeroable` for direct GPU upload.

use bytemuck::{Pod, Zeroable};

/// The shared particle pool's capacity: every emitter's range is taken from
/// it, and an allocation that does not fit is clamped (see the crate doc).
pub const CORONA_MAX_PARTICLES: u32 = 1_048_576; // 2^20

/// Emitter rows the pass reads.
pub const CORONA_MAX_EMITTERS: u32 = 64;

/// Most particles one emitter may request.
pub const CORONA_MAX_PARTICLES_PER_EMITTER: u32 = 262_144;

/// Every emitter's range starts on a multiple of this many particles (the
/// pass's scan block), so a block never holds two emitters' particles.
pub const CORONA_RANGE_ALIGNMENT: u32 = 256;

/// Per-particle GPU state (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuCoronaParticle {
    /// Position (xyz) + alive flag (w, 0 or 1)
    pub pos_and_alive: [f32; 4],
    /// Velocity (xyz) + pad
    pub velocity: [f32; 4],
    /// Color (rgba)
    pub color: [f32; 4],
    /// size.x, size.y, lifetime, age
    pub size_lifetime_age: [f32; 4],
}

/// Per-emitter GPU definition (240 bytes). `particle_offset` and
/// `particle_count` are its range of the shared pool (offset a multiple of
/// [`CORONA_RANGE_ALIGNMENT`]). The pass keeps the spawn cursor in its own
/// buffer and reads `spawn_cursor` as the emitter's identity: when it or the
/// range changes, the row's particles restart.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuCoronaEmitter {
    pub transform: [[f32; 4]; 4],
    // emission params: emit_rate, lifetime, lifetime_variation, gravity
    pub emit_params: [f32; 4],
    // start_size.xy, end_size.xy
    pub size_params: [f32; 4],
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
    pub velocity: [f32; 4],
    pub velocity_variation: [f32; 4],
    pub extras: [f32; 4], // emitter_type, spawn_radius, pad, active (1 or 0)
    pub texture_index: i32,
    pub particle_offset: u32,
    pub particle_count: u32,
    pub spawn_cursor: u32,
    pub _pad: [f32; 12],
}

/// Uniforms pushed to the Corona simulation every frame.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuCoronaUniforms {
    pub delta_time: f32,
    pub total_particles: u32,
    pub emitter_count: u32,
    pub frame_count: u32,
}

/// Indirect draw arguments written by the build-indirect compute pass.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuCoronaDrawIndirect {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

// ── Emitter shape types (CPU-side only) ───────────────────────────────────

/// Shape of the emitter volume for particle spawning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoronaEmitterShape {
    Point,
    Sphere { radius: f32 },
}

impl CoronaEmitterShape {
    pub fn as_type_index(&self) -> u32 {
        match self {
            Self::Point => 0,
            Self::Sphere { .. } => 1,
        }
    }

    pub fn radius(&self) -> f32 {
        match self {
            Self::Point => 0.0,
            Self::Sphere { radius } => *radius,
        }
    }
}

impl Default for CoronaEmitterShape {
    fn default() -> Self {
        Self::Point
    }
}

/// CPU-side descriptor for creating a Corona particle emitter.
#[derive(Debug, Clone)]
pub struct CoronaEmitterDescriptor {
    pub max_particles: u32,
    pub emit_rate: f32,
    pub lifetime: f32,
    pub lifetime_variation: f32,
    pub start_size: [f32; 2],
    pub end_size: [f32; 2],
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
    pub velocity: [f32; 3],
    pub velocity_variation: [f32; 3],
    pub gravity: f32,
    pub shape: CoronaEmitterShape,
    pub texture_index: i32,
    pub position: [f32; 3],
}

impl Default for CoronaEmitterDescriptor {
    fn default() -> Self {
        Self {
            max_particles: 16384,
            emit_rate: 100.0,
            lifetime: 2.0,
            lifetime_variation: 0.5,
            start_size: [0.5, 0.5],
            end_size: [0.1, 0.1],
            start_color: [1.0, 1.0, 1.0, 1.0],
            end_color: [1.0, 1.0, 1.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            velocity_variation: [0.0, 0.0, 0.0],
            gravity: 0.0,
            shape: CoronaEmitterShape::Point,
            texture_index: -1,
            position: [0.0, 0.0, 0.0],
        }
    }
}

impl CoronaEmitterDescriptor {
    pub fn to_gpu(&self) -> GpuCoronaEmitter {
        GpuCoronaEmitter {
            transform: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [self.position[0], self.position[1], self.position[2], 1.0],
            ],
            emit_params: [
                self.emit_rate,
                self.lifetime,
                self.lifetime_variation,
                self.gravity,
            ],
            size_params: [
                self.start_size[0],
                self.start_size[1],
                self.end_size[0],
                self.end_size[1],
            ],
            start_color: self.start_color,
            end_color: self.end_color,
            velocity: [self.velocity[0], self.velocity[1], self.velocity[2], 0.0],
            velocity_variation: [
                self.velocity_variation[0],
                self.velocity_variation[1],
                self.velocity_variation[2],
                0.0,
            ],
            extras: [
                self.shape.as_type_index() as f32,
                self.shape.radius(),
                0.0,
                1.0, // active
            ],
            texture_index: self.texture_index,
            particle_offset: 0,
            particle_count: self.max_particles,
            spawn_cursor: 0,
            _pad: [0.0; 12],
        }
    }
}

/// Frame data passed from the Renderer to the CoronaPass each frame.
#[derive(Clone, Copy)]
pub struct CoronaEmitterFrameData<'a> {
    pub emitters: &'a [u8],
    pub count: u32,
    pub generation: u64,
    pub max_particles: u32,
}
