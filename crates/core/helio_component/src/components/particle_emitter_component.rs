//! A particle emitter: GPU particles (the Corona pass) spawned at the owner
//! (Pulsar-Native #1059).
//!
//! Particles spawn at the owner's position (a point, or anywhere inside a
//! sphere around it), fly with the authored velocity in world space, fall by
//! the authored gravity, and fade from the start colour and size to the end
//! ones over their lifetime. Each particle is a camera-facing sprite: one
//! cell of the Corona pass's built-in 4×4 atlas (`sprite`: 0-3 soft blobs,
//! 4-7 rings, 8-11 stars, 12-15 sparkles).
//!
//! `max_particles` is how many particles the emitter may have alive at
//! once: its share of the pass's shared particle pool. The pool, the
//! emitter's place in it and its spawn cursor are runtime state, never
//! authored: SceneDB derives a `CoronaEmitterSourceRow` (`environment_rows`)
//! from the authored value, and the environment join gates it on the
//! instance and its owner's visibility, places it at the owner and allocates
//! its range of the pool on the GPU, clamped when the pool is full.

use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

/// The class name scripts and level files use.
pub const PARTICLE_EMITTER_CLASS_NAME: &str = "ParticleEmitterComponent";

/// Where particles spawn, around the owner's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum ParticleEmitterShape {
    /// At the owner's position.
    Point,
    /// Anywhere inside a sphere of `spawn_radius` around it.
    Sphere,
}

impl Default for ParticleEmitterShape {
    fn default() -> Self {
        Self::Point
    }
}

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Particles", category_color = "#D9643A")]
#[category("Motion", category_color = "#D9643A")]
#[category("Appearance", category_color = "#C7853B")]
#[serde(default)]
pub struct ParticleEmitterComponent {
    #[property(category = "Particles")]
    pub enabled: bool,
    /// Most particles alive at once: the emitter's share of the shared
    /// particle pool.
    #[property(min = 1.0, max = 262144.0, step = 1.0, category = "Particles")]
    pub max_particles: u32,
    /// Particles spawned per second.
    #[property(min = 0.0, max = 100000.0, step = 1.0, category = "Particles")]
    pub emit_rate: f32,
    /// Seconds a particle lives.
    #[property(min = 0.01, max = 600.0, step = 0.1, category = "Particles")]
    pub lifetime: f32,
    /// Random spread of the lifetime (plus or minus, seconds).
    #[property(min = 0.0, max = 600.0, step = 0.1, category = "Particles")]
    pub lifetime_variation: f32,
    #[property(category = "Particles")]
    pub shape: ParticleEmitterShape,
    /// The sphere's radius (`Sphere` shape).
    #[property(min = 0.0, max = 100000.0, step = 0.1, category = "Particles")]
    pub spawn_radius: f32,
    /// Velocity at spawn, in world space (m/s).
    #[property(category = "Motion")]
    pub velocity: [f32; 3],
    /// Random spread of each velocity component (plus or minus).
    #[property(category = "Motion")]
    pub velocity_variation: [f32; 3],
    /// Vertical acceleration (m/s²; negative falls).
    #[property(min = -1000.0, max = 1000.0, step = 0.1, category = "Motion")]
    pub gravity: f32,
    /// Sprite size (world units) at spawn and at the end of its life.
    #[property(min = 0.0, max = 1000.0, step = 0.01, category = "Appearance")]
    pub start_size: f32,
    #[property(min = 0.0, max = 1000.0, step = 0.01, category = "Appearance")]
    pub end_size: f32,
    /// Colour and opacity at spawn and at the end of its life.
    #[property(category = "Appearance")]
    pub start_color: [f32; 4],
    #[property(category = "Appearance")]
    pub end_color: [f32; 4],
    /// The built-in atlas cell (0-15).
    #[property(min = 0.0, max = 15.0, step = 1.0, category = "Appearance")]
    pub sprite: u32,
}

impl Default for ParticleEmitterComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            max_particles: 1024,
            emit_rate: 100.0,
            lifetime: 2.0,
            lifetime_variation: 0.5,
            shape: ParticleEmitterShape::Point,
            spawn_radius: 0.5,
            velocity: [0.0, 1.0, 0.0],
            velocity_variation: [0.5, 0.2, 0.5],
            gravity: 0.0,
            start_size: 0.2,
            end_size: 0.05,
            start_color: [1.0, 1.0, 1.0, 1.0],
            end_color: [1.0, 1.0, 1.0, 0.0],
            sprite: 0,
        }
    }
}

impl ParticleEmitterComponent {
    /// The Corona pass row in the owner's space: every authored field, with
    /// the transform and particle offset left zero for the environment join
    /// and `particle_count` the requested range (clamped to
    /// `CORONA_MAX_PARTICLES_PER_EMITTER`). A disabled emitter's row is
    /// zero, which the join skips.
    pub fn to_row(&self) -> helio_pass_corona::GpuCoronaEmitter {
        if !self.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let shape = match self.shape {
            ParticleEmitterShape::Point => helio_pass_corona::CoronaEmitterShape::Point,
            ParticleEmitterShape::Sphere => helio_pass_corona::CoronaEmitterShape::Sphere {
                radius: self.spawn_radius.max(0.0),
            },
        };
        let mut row = helio_pass_corona::CoronaEmitterDescriptor {
            max_particles: self
                .max_particles
                .clamp(1, helio_pass_corona::CORONA_MAX_PARTICLES_PER_EMITTER),
            emit_rate: self.emit_rate.max(0.0),
            lifetime: self.lifetime.max(0.01),
            lifetime_variation: self.lifetime_variation.max(0.0),
            start_size: [self.start_size.max(0.0); 2],
            end_size: [self.end_size.max(0.0); 2],
            start_color: self.start_color,
            end_color: self.end_color,
            velocity: self.velocity,
            velocity_variation: self.velocity_variation.map(f32::abs),
            gravity: self.gravity,
            shape,
            texture_index: self.sprite.min(15) as i32,
            position: [0.0; 3],
        }
        .to_gpu();
        row.transform = [[0.0; 4]; 4];
        row
    }
}

#[register_world_component]
impl ParticleEmitterComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_requests_its_pool_range_and_leaves_placement_to_the_join() {
        let emitter = ParticleEmitterComponent {
            max_particles: 500,
            shape: ParticleEmitterShape::Sphere,
            spawn_radius: 2.0,
            sprite: 9,
            ..Default::default()
        };
        let row = emitter.to_row();
        assert_eq!(row.particle_count, 500);
        assert_eq!(row.particle_offset, 0);
        assert_eq!(row.transform, [[0.0; 4]; 4]);
        assert_eq!(row.extras, [1.0, 2.0, 0.0, 1.0]);
        assert_eq!(row.texture_index, 9);

        let huge = ParticleEmitterComponent {
            max_particles: u32::MAX,
            ..Default::default()
        };
        assert_eq!(
            huge.to_row().particle_count,
            helio_pass_corona::CORONA_MAX_PARTICLES_PER_EMITTER
        );
        let disabled = ParticleEmitterComponent {
            enabled: false,
            ..Default::default()
        };
        assert!(
            bytemuck::bytes_of(&disabled.to_row())
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}
