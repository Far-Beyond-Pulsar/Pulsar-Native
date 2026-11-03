//! Global wind state shared by every foliage shader.
//!
//! One [`GpuWind`] uniform drives grass blades, tree WPO and impostor cards. Sharing a
//! single uniform (rather than letting each pass derive its own time base) is what keeps
//! the three representations *in phase*: a blade at the L1/L2 cross-fade band is drawn
//! twice, once as geometry and once as a card, and if the two evaluated wind at even
//! slightly different times the stochastic cross-fade would show the silhouette shearing
//! against itself.
//!
//! The CPU-side mirror is [`Wind`], which owns the clock. Call [`Wind::advance`] exactly
//! once per simulated frame and upload [`Wind::to_gpu`]; the pair of timestamps that
//! produces is the whole reason this module exists (see [`GpuWind::time_prev_time`]).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// Global wind parameters uploaded as a uniform. Exactly 48 bytes.
///
/// Consumed by the shared `foliage_wind.wgsl` prelude, which every foliage vertex shader
/// includes so grass, tree G-buffer and impostor passes evaluate byte-identical wind.
///
/// # Layout (48 bytes, 16-byte aligned)
/// ```text
///  0..16   direction_speed: vec4<f32>  xyz = normalised direction, w = base speed (m/s)
/// 16..32   gust:            vec4<f32>  amplitude, frequency, phase, turbulence scale
/// 32..40   time_prev_time:  vec2<f32>  t, t - dt
/// 40..48   _pad:            vec2<f32>  reserved; keeps the struct a multiple of 16
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuWind {
    /// xyz = normalised wind direction in world space, w = base speed in m/s.
    ///
    /// The direction **must** already be normalised on the CPU ([`Wind::to_gpu`] does
    /// this). Shaders scale sway by `w` and never re-normalise, so an unnormalised
    /// direction silently doubles the amplitude instead of failing visibly.
    pub direction_speed: [f32; 4],

    /// Gust envelope: `[amplitude, frequency (Hz), phase (radians), turbulence scale (1/m)]`.
    ///
    /// `turbulence_scale` is a *spatial* frequency: it is what makes a gust front visibly
    /// travel across a field instead of every blade in the world pulsing in unison. Setting
    /// it to zero is a valid "uniform gust" mode, not a bug.
    pub gust: [f32; 4],

    /// `[t, t - dt]` — the current and previous frame's wind clock, in seconds.
    ///
    /// **Do not remove the second timestamp, and do not replace it with a `dt` scalar
    /// derived at draw time.** Every foliage vertex shader evaluates the entire wind model
    /// twice — once at `t` and once at `t - dt` — and emits both the current clip position
    /// and a true `prev_clip_position` into the G-buffer velocity target. That velocity is
    /// what TAA uses to reproject animated vegetation.
    ///
    /// What breaks without it: with no valid `prev_clip_position`, wind-displaced vertices
    /// report zero motion, TAA reprojects them to the wrong history texel, and every blade
    /// of grass in the frame smears — the exact artefact this renderer exists to avoid.
    /// It also takes the stochastic LOD cross-fade down with it, because that fade relies
    /// on TAA resolving a dithered pattern that only converges when the motion vectors
    /// under it are correct.
    ///
    /// The two timestamps are also why the wind clock must be advanced exactly once per
    /// frame, from one place: advancing it twice halves the apparent velocity, and not
    /// advancing it makes `t == t - dt`, which reports zero motion just as badly as
    /// omitting the field.
    pub time_prev_time: [f32; 2],

    /// Padding to the 16-byte uniform stride. Zeroed; never read by a shader.
    pub _pad: [f32; 2],
}

const _: () = {
    assert!(
        std::mem::size_of::<GpuWind>() == 48,
        "GpuWind must be exactly 48 bytes"
    );
    assert!(
        std::mem::align_of::<GpuWind>() <= 16,
        "GpuWind alignment must be 16 bytes or less for GPU compatibility"
    );
};

impl Default for GpuWind {
    fn default() -> Self {
        Wind::default().to_gpu()
    }
}

/// CPU-side authoring state and clock for the global wind.
///
/// Held by `Scene` (`scene.set_wind(...)`) and converted to [`GpuWind`] once per frame.
/// This type owns the *only* wind clock in the renderer: passes must not keep their own
/// accumulated time, because two clocks drift and drifting clocks put the blade geometry
/// and the impostor cards out of phase at the cross-fade band.
///
/// ```
/// # use helio_pass_foliage_place::wind::Wind;
/// # use glam::Vec3;
/// let mut wind = Wind { direction: Vec3::X, speed: 4.0, gust_amplitude: 0.6, ..Default::default() };
/// wind.advance(1.0 / 60.0);
/// let gpu = wind.to_gpu();
/// assert!(gpu.time_prev_time[0] > gpu.time_prev_time[1]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wind {
    /// World-space wind direction. Need not be normalised; [`Wind::to_gpu`] normalises it.
    pub direction: Vec3,
    /// Base wind speed in m/s. Drives the steady component of trunk/stem sway.
    pub speed: f32,
    /// Peak additional sway during a gust, as a multiple of the base amplitude.
    pub gust_amplitude: f32,
    /// Gust rate in Hz. Low by design — a gust is a weather event, not a vibration.
    pub gust_frequency: f32,
    /// Gust phase offset in radians. Exposed so two scenes (or a replay) can be put in
    /// the same gust cycle without rewinding the clock.
    pub gust_phase: f32,
    /// Spatial frequency of the gust noise, in 1/m. Larger values make gust fronts
    /// smaller and more chaotic; zero makes the gust globally uniform.
    pub turbulence_scale: f32,
    /// Current wind time in seconds.
    ///
    /// Public so `Wind { .. , ..Default::default() }` construction works, but it should be
    /// moved only by [`advance`](Self::advance) or [`reset_time`](Self::reset_time).
    /// Assigning it directly leaves `prev_time` behind, and the resulting frame delta is
    /// whatever the gap happens to be — which is published straight into foliage motion
    /// vectors.
    pub time: f32,
    /// Wind time at the previous frame, in seconds.
    ///
    /// See [`GpuWind::time_prev_time`] — this is the input for foliage motion vectors, not
    /// a debugging convenience. Never set it independently of `time`.
    pub prev_time: f32,
}

impl Default for Wind {
    /// A light, believable breeze: 2 m/s along +X with a slow, shallow gust.
    ///
    /// Deliberately not calm. A zero-speed default reads as "wind is broken" during
    /// bring-up, whereas a visible breeze makes a mis-wired uniform obvious immediately.
    fn default() -> Self {
        Self {
            direction: Vec3::X,
            speed: 2.0,
            gust_amplitude: 0.35,
            gust_frequency: 0.15,
            gust_phase: 0.0,
            turbulence_scale: 0.05,
            time: 0.0,
            prev_time: 0.0,
        }
    }
}

impl Wind {
    /// The delta actually represented by the published timestamp pair.
    ///
    /// This is the interval foliage motion vectors are computed over, which is not
    /// necessarily the `dt` last passed to [`advance`](Self::advance) — see the clamping
    /// rules there.
    #[inline]
    pub fn delta_time(&self) -> f32 {
        self.time - self.prev_time
    }

    /// Rolls `time` into `prev_time` and advances `time` by `dt` seconds.
    ///
    /// Call this exactly once per simulated frame, before [`to_gpu`](Self::to_gpu).
    /// Calling it twice in a frame publishes a stale `prev_time` and halves the apparent
    /// foliage velocity; not calling it leaves `time == prev_time`, which makes every
    /// foliage motion vector zero and hands TAA the smearing it is supposed to prevent.
    ///
    /// `dt` is clamped to a finite, non-negative value. A paused or rewound clock (a
    /// frame with `dt <= 0`, or a `NaN` from a degenerate timestep) would otherwise
    /// publish `prev_time >= time`, producing motion vectors that point backwards or are
    /// `NaN`; a `NaN` velocity poisons the TAA history for the whole screen, not just the
    /// affected pixels. Clamping degrades to "vegetation is frozen this frame", which is
    /// both correct and recoverable.
    #[inline]
    pub fn advance(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.prev_time = self.time;
        self.time += dt;
    }

    /// Resets both timestamps to `t`, collapsing the frame delta to zero.
    ///
    /// Use on a teleport, level load or timeline scrub — anywhere a continuous `advance`
    /// would publish a huge `dt` and throw one frame of enormous, wrong foliage velocity
    /// into the velocity target. One frozen frame is invisible; one frame of 100 m/s
    /// grass velocity is a full-screen TAA smear.
    #[inline]
    pub fn reset_time(&mut self, t: f32) {
        let t = if t.is_finite() { t } else { 0.0 };
        self.time = t;
        self.prev_time = t;
    }

    /// Packs into the GPU uniform, normalising the direction.
    ///
    /// A zero-length (or non-finite) direction falls back to `+X` rather than producing
    /// `NaN`: a `NaN` direction propagates through the vertex position of every foliage
    /// instance in the scene, and the resulting primitives are discarded, so the failure
    /// mode is "all vegetation vanished" with no shader error to point at.
    pub fn to_gpu(&self) -> GpuWind {
        let dir = self.direction.normalize_or_zero();
        let dir = if dir.is_finite() && dir.length_squared() > 0.0 {
            dir
        } else {
            Vec3::X
        };
        GpuWind {
            direction_speed: [dir.x, dir.y, dir.z, self.speed],
            gust: [
                self.gust_amplitude,
                self.gust_frequency,
                self.gust_phase,
                self.turbulence_scale,
            ],
            time_prev_time: [self.time, self.prev_time],
            _pad: [0.0; 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuWind, Wind};
    use glam::Vec3;

    #[test]
    fn gpu_wind_layout_is_stable() {
        assert_eq!(std::mem::size_of::<GpuWind>(), 48);
        assert_eq!(std::mem::align_of::<GpuWind>(), 4);
        // Offsets the WGSL `foliage_wind.wgsl` prelude relies on.
        let w = GpuWind::default();
        let bytes = bytemuck::bytes_of(&w);
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    fn advance_rolls_time_into_prev_time() {
        let mut wind = Wind::default();
        assert_eq!(wind.time, 0.0);
        assert_eq!(wind.prev_time, 0.0);

        wind.advance(0.5);
        assert_eq!(wind.prev_time, 0.0);
        assert_eq!(wind.time, 0.5);
        assert_eq!(wind.delta_time(), 0.5);

        wind.advance(0.25);
        assert_eq!(wind.prev_time, 0.5);
        assert_eq!(wind.time, 0.75);
        assert_eq!(wind.delta_time(), 0.25);
    }

    #[test]
    fn advance_publishes_both_timestamps_to_the_gpu_struct() {
        let mut wind = Wind::default();
        wind.advance(1.0 / 60.0);
        wind.advance(1.0 / 60.0);
        let gpu = wind.to_gpu();
        assert_eq!(gpu.time_prev_time[0], wind.time);
        assert_eq!(gpu.time_prev_time[1], wind.prev_time);
        // Non-zero delta is what makes foliage motion vectors non-degenerate.
        assert!(gpu.time_prev_time[0] > gpu.time_prev_time[1]);
    }

    #[test]
    fn advance_clamps_degenerate_deltas() {
        let mut wind = Wind::default();
        wind.advance(1.0);

        wind.advance(-5.0);
        assert_eq!(wind.time, 1.0);
        assert_eq!(wind.prev_time, 1.0);
        assert_eq!(wind.delta_time(), 0.0);

        wind.advance(f32::NAN);
        assert!(wind.time.is_finite());
        assert_eq!(wind.delta_time(), 0.0);

        wind.advance(f32::INFINITY);
        assert!(wind.time.is_finite());
        assert_eq!(wind.time, 1.0);
    }

    #[test]
    fn reset_time_collapses_the_frame_delta() {
        let mut wind = Wind::default();
        wind.advance(0.016);
        wind.reset_time(120.0);
        assert_eq!(wind.time, 120.0);
        assert_eq!(wind.prev_time, 120.0);
        assert_eq!(wind.delta_time(), 0.0);

        wind.reset_time(f32::NAN);
        assert_eq!(wind.time, 0.0);
    }

    #[test]
    fn to_gpu_normalises_direction_and_keeps_speed_separate() {
        let wind = Wind {
            direction: Vec3::new(0.0, 0.0, 3.0),
            speed: 7.5,
            ..Default::default()
        };
        let gpu = wind.to_gpu();
        let dir = Vec3::from_slice(&gpu.direction_speed[..3]);
        assert!((dir.length() - 1.0).abs() < 1e-6);
        assert_eq!(dir, Vec3::Z);
        assert_eq!(gpu.direction_speed[3], 7.5);
    }

    #[test]
    fn to_gpu_falls_back_to_plus_x_for_a_degenerate_direction() {
        for bad in [
            Vec3::ZERO,
            Vec3::splat(f32::NAN),
            Vec3::splat(f32::INFINITY),
        ] {
            let gpu = Wind {
                direction: bad,
                ..Default::default()
            }
            .to_gpu();
            let dir = Vec3::from_slice(&gpu.direction_speed[..3]);
            assert!(
                dir.is_finite(),
                "degenerate direction {bad:?} produced {dir:?}"
            );
            assert_eq!(dir, Vec3::X);
        }
    }

    #[test]
    fn gust_packing_order_matches_the_shader_contract() {
        let wind = Wind {
            gust_amplitude: 0.6,
            gust_frequency: 0.2,
            gust_phase: 1.25,
            turbulence_scale: 0.08,
            ..Default::default()
        };
        assert_eq!(wind.to_gpu().gust, [0.6, 0.2, 1.25, 0.08]);
    }
}

impl From<GpuWind> for crate::FoliageWindRow {
    fn from(v: GpuWind) -> Self {
        bytemuck::cast(v)
    }
}
impl From<crate::FoliageWindRow> for GpuWind {
    fn from(v: crate::FoliageWindRow) -> Self {
        bytemuck::cast(v)
    }
}

// The host row of the wind (`host_rows`, the foliage api's) converts to and
// from this crate's wind record.
#[cfg(feature = "host-rows")]
mod host_rows {
    use super::GpuWind;

    impl From<GpuWind> for helio_foliage_api::host_rows::FoliageWindComponent {
        fn from(value: GpuWind) -> Self {
            crate::FoliageWindRow::from(value).into()
        }
    }

    impl From<helio_foliage_api::host_rows::FoliageWindComponent> for GpuWind {
        fn from(value: helio_foliage_api::host_rows::FoliageWindComponent) -> Self {
            crate::FoliageWindRow::from(value).into()
        }
    }
}
