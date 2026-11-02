//! The level's global wind (Pulsar-Native #1123): one wind every foliage
//! component sways in, each foliage type by its own per-band response
//! (`FoliageComponent`'s trunk sway, branch flutter and leaf jitter).
//!
//! A level has at most one (`engine_backend::scene::level_rules`), edited
//! in World Settings like the sky. The wind is a world-space direction and
//! speed with a travelling gust; the owner object's transform does not
//! move it. A foliage component may still blow in its own wind instead
//! (`use_global_wind` off), and a level without a global wind uses the
//! components' own wind.
//!
//! SceneDB derives a `GlobalWindSourceRow` (`environment_rows`) from the
//! authored value; the environment join places it as the foliage passes'
//! wind row (`"foliage_wind"`). How the wind moves over time is the
//! renderer's frame clock, not part of the row.

use engine_class_derive::{engine_class, register_world_component};

/// The class name scripts and level files use.
pub const WIND_CLASS_NAME: &str = "WindComponent";

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Wind", category_color = "#7EE787")]
#[serde(default)]
pub struct WindComponent {
    #[property(category = "Wind")]
    pub enabled: bool,
    /// World-space direction the wind blows toward (need not be
    /// normalised).
    #[property(category = "Wind")]
    pub direction: [f32; 3],
    /// Base wind speed in m/s; 0 is calm.
    #[property(min = 0.0, max = 60.0, step = 0.1, category = "Wind")]
    pub speed: f32,
    /// Peak additional sway during a gust, as a multiple of the base sway.
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Wind")]
    pub gust_amplitude: f32,
    /// Gusts per second.
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Wind")]
    pub gust_frequency: f32,
    /// Spatial frequency of the gust fronts in 1/m: larger makes them
    /// smaller and more chaotic, 0 makes every plant gust together.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Wind")]
    pub turbulence_scale: f32,
}

impl Default for WindComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            direction: [1.0, 0.0, 0.35],
            speed: 2.0,
            gust_amplitude: 0.6,
            gust_frequency: 0.25,
            turbulence_scale: 0.05,
        }
    }
}

impl WindComponent {
    /// The foliage passes' wind row: direction (normalised), speed and
    /// gusts. Its wind clock is zero (the passes read the frame clock); its
    /// `_pad[0]` is 1, the join's mark of a global wind row, and the whole
    /// row is zero while disabled.
    pub fn to_row(&self) -> helio_pass_foliage_place::GpuWind {
        if !self.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let mut row = helio_pass_foliage_place::Wind {
            direction: glam::Vec3::from_array(self.direction),
            speed: self.speed.max(0.0),
            gust_amplitude: self.gust_amplitude.max(0.0),
            gust_frequency: self.gust_frequency.max(0.0),
            turbulence_scale: self.turbulence_scale.max(0.0),
            ..Default::default()
        }
        .to_gpu();
        row._pad[0] = 1.0;
        row
    }
}

#[register_world_component]
impl WindComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_is_the_foliage_wind_row_marked_global() {
        let wind = WindComponent {
            direction: [0.0, 0.0, 2.0],
            speed: 5.0,
            ..Default::default()
        };
        let row = wind.to_row();
        assert_eq!(row.direction_speed, [0.0, 0.0, 1.0, 5.0]);
        assert_eq!(row.time_prev_time, [0.0; 2]);
        assert_eq!(row._pad[0], 1.0);
        let calm = WindComponent {
            speed: 0.0,
            ..Default::default()
        };
        assert_eq!(
            calm.to_row()._pad[0],
            1.0,
            "a calm wind is still the level's wind"
        );
        let off = WindComponent {
            enabled: false,
            ..Default::default()
        };
        assert!(bytemuck::bytes_of(&off.to_row()).iter().all(|b| *b == 0));
    }
}
