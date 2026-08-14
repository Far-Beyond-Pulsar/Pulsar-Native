//! Water volume component (Phase D, Pulsar-Native#558) — heightfield-sim
//! water rendering (waves, reflections, caustics, underwater fog).
//!
//! Helio already has full native support for this
//! (`Scene::insert_water_volume`/`update_water_volume`/`remove_water_volume`,
//! `WaterVolumeDescriptor`) — same shape as `ReflectionCaptureComponent`,
//! the gap this closes is purely the author-facing `#[engine_class]`
//! wrapper.
//!
//! `WaterVolumeDescriptor` authors `bounds_min`/`bounds_max` as absolute
//! world-space AABB corners. That's redundant with the scene's own
//! transform system and would desync the moment someone moves the owning
//! object without also editing two raw coordinate fields by hand, so this
//! component instead exposes `size` (centered on the owning object's
//! position) and derives the AABB from `owner.position` every sync pass —
//! matching how every other component in this crate treats the owning
//! object's transform as authoritative, not a field to duplicate.
//!
//! Not covered here: `WaterHitboxDescriptor`. Read its own doc before
//! assuming it belongs alongside this component — it explicitly records an
//! object's *previous* AABB and *current* AABB from frame to frame (`old_min`/
//! `old_max`/`new_min`/`new_max`) to drive splash displacement. That's
//! per-frame runtime state computed from something else's movement, not
//! data a level designer authors once and places — it needs to be driven by
//! physics/movement code reacting to objects entering a water volume, which
//! is a different kind of integration than "one component, one purpose"
//! placement. Deferred, not overlooked.

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use helio::{Renderer, WaterVolumeDescriptor};
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner,
};
use serde::{Deserialize, Serialize};

use crate::subsystems::WaterVolumeCache;

pub const WATER_VOLUME_CLASS_NAME: &str = "WaterVolumeComponent";

/// Heightfield-simulation water volume, centered on the owning object.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Bounds", category_color = "#3AA0FF")]
#[category("Waves", category_color = "#3AA0FF")]
#[category("Wind", category_color = "#3AA0FF")]
#[category("Visual", category_color = "#2FA88A")]
#[category("Reflection", category_color = "#2FA88A")]
#[category("Caustics", category_color = "#C79A3E")]
#[category("Underwater", category_color = "#7C6FD1")]
#[category("Shadow", category_color = "#7C6FD1")]
#[category("Lighting", category_color = "#D18F6F")]
pub struct WaterVolumeComponent {
    #[property]
    pub enabled: bool,

    // ── Bounds ──────────────────────────────────────────────────────────
    /// Full AABB extent (not half-extent) in each axis, centered on the
    /// owning object's position.
    #[property(category = "Bounds")]
    pub size: [f32; 3],
    /// Water surface height, as a Y offset from the owning object's
    /// position (0 = surface sits exactly at the object's own Y).
    #[property(category = "Bounds")]
    pub surface_height_offset: f32,

    // ── Waves ───────────────────────────────────────────────────────────
    /// Peak wave displacement in metres, above and below the rest height.
    /// Clamped by the shader to the headroom within `size`, so waves can
    /// never leave the volume.
    #[property(min = 0.0, max = 20.0, step = 0.05, category = "Waves")]
    pub wave_amplitude: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Waves")]
    pub wave_frequency: f32,
    #[property(min = 0.0, max = 20.0, step = 0.05, category = "Waves")]
    pub wave_speed: f32,
    // `[f32; 2]` doesn't implement `Reflectable` (only `[f32; 3]`/`[f32; 4]`
    // do, per pulsar_reflection's built-in prims) -- split into two plain
    // f32 fields rather than reaching for a 3-element array with an unused
    // component, which would be a wrong shape for what this actually is.
    /// Primary wave direction X (XZ plane). Need not be normalized together
    /// with `wave_direction_z`.
    #[property(category = "Waves")]
    pub wave_direction_x: f32,
    /// Primary wave direction Z (XZ plane).
    #[property(category = "Waves")]
    pub wave_direction_z: f32,
    /// 0.0 = sine wave, 1.0 = sharp peaks.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Waves")]
    pub wave_steepness: f32,
    /// Restoring-force spring constant toward the mean height. ~1.0 feels
    /// fluid; ~2.0 feels jelly-like.
    #[property(min = 0.5, max = 2.0, step = 0.01, category = "Waves")]
    pub wave_spring: f32,
    /// Per-step energy damping (0..1). Closer to 1.0 = waves linger; closer
    /// to 0.9 = waves die quickly.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Waves")]
    pub wave_damping: f32,
    /// Spatial scale of gust impulses on the heightfield. 1.0 = default;
    /// 0.25 = fine ripples; 2.0 = large swells.
    #[property(min = 0.05, max = 10.0, step = 0.05, category = "Waves")]
    pub wave_scale: f32,

    // ── Wind ────────────────────────────────────────────────────────────
    /// Wind direction X, world XZ space. `[0, 0]` (both X and Z) = calm
    /// water. Same `[f32; 2]`-isn't-`Reflectable` reasoning as
    /// `wave_direction_x`/`_z` above.
    #[property(category = "Wind")]
    pub wind_direction_x: f32,
    /// Wind direction Z, world XZ space.
    #[property(category = "Wind")]
    pub wind_direction_z: f32,
    /// 0 = calm, ~1 = gentle ripples, ~5 = choppy.
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Wind")]
    pub wind_strength: f32,

    // ── Visual ──────────────────────────────────────────────────────────
    /// Base water color (deep water).
    #[property(category = "Visual")]
    pub water_color: [f32; 3],
    /// RGB absorption per meter depth (Beer-Lambert).
    #[property(category = "Visual")]
    pub extinction: [f32; 3],
    /// Wave steepness threshold to spawn foam.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Visual")]
    pub foam_threshold: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Visual")]
    pub foam_amount: f32,

    // ── Reflection / refraction ─────────────────────────────────────────
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Reflection")]
    pub reflection_strength: f32,
    /// 1.0 is physically plausible; 0.0 disables distortion.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Reflection")]
    pub refraction_strength: f32,
    /// Higher = sharper Fresnel falloff.
    #[property(min = 0.1, max = 20.0, step = 0.1, category = "Reflection")]
    pub fresnel_power: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Reflection")]
    pub fresnel_min: f32,
    /// Index of refraction. 1.333 for water.
    #[property(min = 1.0, max = 2.5, step = 0.001, category = "Reflection")]
    pub ior: f32,
    #[property(category = "Reflection")]
    pub ssr_enabled: bool,
    // `u32` doesn't implement `Reflectable` (only `i32`/`i64`/`u64` do,
    // per pulsar_reflection's built-in prims) -- `i32` here, clamped to
    // non-negative when building the descriptor.
    #[property(min = 1, max = 256, category = "Reflection")]
    pub ssr_steps: i32,
    #[property(min = 0.001, max = 1.0, step = 0.001, category = "Reflection")]
    pub ssr_step_size: f32,
    #[property(min = 0.001, max = 1.0, step = 0.001, category = "Reflection")]
    pub ssr_thickness: f32,

    // ── Caustics ────────────────────────────────────────────────────────
    #[property(category = "Caustics")]
    pub caustics_enabled: bool,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Caustics")]
    pub caustics_intensity: f32,
    #[property(min = 0.1, max = 50.0, step = 0.1, category = "Caustics")]
    pub caustics_scale: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Caustics")]
    pub caustics_speed: f32,

    // ── Underwater ──────────────────────────────────────────────────────
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Underwater")]
    pub fog_density: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Underwater")]
    pub god_rays_intensity: f32,
    /// Effective water density for fog.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Underwater")]
    pub density: f32,

    // ── Shadow ──────────────────────────────────────────────────────────
    /// Rim light intensity for pool walls.
    #[property(min = 0.0, max = 5.0, step = 0.05, category = "Shadow")]
    pub shadow_rim: f32,
    /// 0.0 = no hitbox shadow, 1.0 = full shadow under a hitbox.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Shadow")]
    pub shadow_hitbox: f32,
    #[property(min = 0.0, max = 5.0, step = 0.05, category = "Shadow")]
    pub shadow_ao: f32,

    // ── Lighting ────────────────────────────────────────────────────────
    /// Sun / dominant directional light direction, world space. Need not be
    /// normalized.
    #[property(category = "Lighting")]
    pub sun_direction: [f32; 3],
}

impl Default for WaterVolumeComponent {
    /// Mirrors `WaterVolumeDescriptor::ocean()`'s values.
    fn default() -> Self {
        Self {
            enabled: true,
            size: [200.0, 60.0, 200.0],
            surface_height_offset: 0.0,
            wave_amplitude: 0.5,
            wave_frequency: 0.3,
            wave_speed: 1.5,
            wave_direction_x: 1.0,
            wave_direction_z: 0.0,
            wave_steepness: 0.5,
            wave_spring: 1.2,
            wave_damping: 0.985,
            wave_scale: 1.0,
            wind_direction_x: 0.0,
            wind_direction_z: 0.0,
            wind_strength: 0.0,
            water_color: [0.0, 0.2, 0.4],
            extinction: [0.1, 0.05, 0.02],
            foam_threshold: 0.8,
            foam_amount: 0.6,
            reflection_strength: 0.8,
            refraction_strength: 1.0,
            fresnel_power: 5.0,
            fresnel_min: 0.1,
            ior: 1.333,
            ssr_enabled: true,
            ssr_steps: 32,
            ssr_step_size: 0.05,
            ssr_thickness: 0.02,
            caustics_enabled: true,
            caustics_intensity: 1.5,
            caustics_scale: 5.0,
            caustics_speed: 0.5,
            fog_density: 0.03,
            god_rays_intensity: 1.0,
            density: 0.03,
            shadow_rim: 1.0,
            shadow_hitbox: 0.0,
            shadow_ao: 1.0,
            sun_direction: [0.5, 1.0, 0.5],
        }
    }
}

impl WaterVolumeComponent {
    fn to_descriptor(&self, owner: &RuntimeComponentOwner) -> WaterVolumeDescriptor {
        let [cx, cy, cz] = owner.position;
        let [sx, sy, sz] = self.size;
        WaterVolumeDescriptor {
            bounds_min: [cx - sx * 0.5, cy - sy * 0.5, cz - sz * 0.5],
            bounds_max: [cx + sx * 0.5, cy + sy * 0.5, cz + sz * 0.5],
            surface_height: cy + self.surface_height_offset,
            wave_amplitude: self.wave_amplitude,
            wave_frequency: self.wave_frequency,
            wave_speed: self.wave_speed,
            wave_direction: [self.wave_direction_x, self.wave_direction_z],
            wave_steepness: self.wave_steepness,
            water_color: self.water_color,
            extinction: self.extinction,
            foam_threshold: self.foam_threshold,
            foam_amount: self.foam_amount,
            reflection_strength: self.reflection_strength,
            refraction_strength: self.refraction_strength,
            fresnel_power: self.fresnel_power,
            caustics_enabled: self.caustics_enabled,
            caustics_intensity: self.caustics_intensity,
            caustics_scale: self.caustics_scale,
            caustics_speed: self.caustics_speed,
            fog_density: self.fog_density,
            god_rays_intensity: self.god_rays_intensity,
            ssr_enabled: self.ssr_enabled,
            ssr_steps: self.ssr_steps.max(0) as u32,
            ssr_step_size: self.ssr_step_size,
            ssr_thickness: self.ssr_thickness,
            ior: self.ior,
            fresnel_min: self.fresnel_min,
            density: self.density,
            shadow_rim: self.shadow_rim,
            shadow_hitbox: self.shadow_hitbox,
            shadow_ao: self.shadow_ao,
            sun_direction: self.sun_direction,
            wave_spring: self.wave_spring,
            wave_damping: self.wave_damping,
            wind_direction: [self.wind_direction_x, self.wind_direction_z],
            wind_strength: self.wind_strength,
            wave_scale: self.wave_scale,
        }
    }
}

#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for WaterVolumeComponent {
    const CLASS_NAME: &'static str = WATER_VOLUME_CLASS_NAME;

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let cached_id = get_subsystem!(context, WaterVolumeCache).get(owner.scene_object_id);

        if !component.enabled {
            if let Some(id) = cached_id {
                let removed = get_subsystem!(context, Renderer).scene_mut().remove_water_volume(id);
                if removed.is_ok() {
                    get_subsystem!(context, WaterVolumeCache).remove(owner.scene_object_id);
                }
            }
            return;
        }

        let descriptor = component.to_descriptor(owner);

        match cached_id {
            Some(id) => {
                let _ = get_subsystem!(context, Renderer)
                    .scene_mut()
                    .update_water_volume(id, descriptor);
            }
            None => {
                let inserted = get_subsystem!(context, Renderer).scene_mut().insert_water_volume(descriptor);
                if let Ok(id) = inserted {
                    get_subsystem!(context, WaterVolumeCache)
                        .insert(owner.scene_object_id.to_string(), id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_subsystems::{Subsystem, SubsystemContext};
    use pulsar_reflection::{apply_runtime_behavior_for_class, Subsystems};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    struct TestRuntimeContext {
        project_root: PathBuf,
        subsystems: Subsystems,
        errors: Vec<String>,
    }

    impl ComponentRuntimeContext for TestRuntimeContext {
        fn subsystems_mut(&mut self) -> &mut Subsystems {
            &mut self.subsystems
        }
        fn project_root(&self) -> &Path {
            &self.project_root
        }
        fn report_error(&mut self, message: String) {
            self.errors.push(message);
        }
    }

    fn owner<'a>(props: &'a HashMap<String, serde_json::Value>) -> RuntimeComponentOwner<'a> {
        RuntimeComponentOwner {
            scene_object_id: "lake",
            position: [0.0, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props,
        }
    }

    #[test]
    fn runtime_behavior_has_the_reflected_component_name() {
        assert_eq!(
            <WaterVolumeComponent as ComponentRuntimeBehavior>::CLASS_NAME,
            WATER_VOLUME_CLASS_NAME
        );
    }

    #[test]
    fn to_descriptor_centers_bounds_on_owner_position() {
        let component = WaterVolumeComponent { size: [10.0, 4.0, 10.0], ..Default::default() };
        let props = HashMap::new();
        let mut o = owner(&props);
        o.position = [5.0, 1.0, -5.0];

        let descriptor = component.to_descriptor(&o);

        assert_eq!(descriptor.bounds_min, [0.0, -1.0, -10.0]);
        assert_eq!(descriptor.bounds_max, [10.0, 3.0, 0.0]);
        assert_eq!(descriptor.surface_height, 1.0);
    }

    #[test]
    fn disabling_a_never_inserted_volume_is_a_quiet_no_op() {
        let mut subsystems = Subsystems::new();
        subsystems.register(WaterVolumeCache::new());
        let mut context = TestRuntimeContext {
            project_root: PathBuf::from("."),
            subsystems,
            errors: Vec::new(),
        };
        let props = HashMap::new();
        let disabled = WaterVolumeComponent { enabled: false, ..Default::default() };

        assert!(apply_runtime_behavior_for_class(
            WATER_VOLUME_CLASS_NAME,
            &owner(&props),
            0,
            &serde_json::to_value(disabled).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
    }
}
