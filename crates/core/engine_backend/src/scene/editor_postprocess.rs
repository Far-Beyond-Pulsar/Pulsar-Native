//! The editor viewport's post-process baseline.
//!
//! Helio resolves post-processing on the GPU from its defaults, the
//! camera's `camera_postprocess` rows (authored `CameraPostProcessComponent`s)
//! and post-process volumes. The editor viewport's toggles are a renderer
//! setting, not scene data: bloom comes from the project's graphics settings
//! (`bloom_enabled`, `bloom_intensity`) and the toolbar's Bloom toggle, and
//! the renderer sets them as the resolver's defaults
//! (`PostProcessVolumeBlendPass::set_defaults`). Every other setting stays at
//! Helio's default. Authored camera rows and volumes in the level still
//! override it (Pulsar-Native#1035, Phase 4: the editor no longer writes a
//! row of its own into the scene world).

use helio_pass_postprocess::PostProcessSettings;

/// The post-process values the editor controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorPostProcess {
    pub bloom_enabled: bool,
    /// Helio units: the fraction of extracted bright light that is scattered.
    pub bloom_intensity: f32,
}

impl EditorPostProcess {
    /// Resolve from the project graphics settings and the toolbar toggle.
    ///
    /// Bloom shows only when both the project and the viewport toggle enable
    /// it. The project's intensity is a multiplier on Helio's default, so the
    /// schema default of 1.0 renders Helio's calibrated look.
    pub fn resolve(
        project_bloom_enabled: bool,
        project_bloom_intensity: f64,
        viewport_bloom: bool,
    ) -> Self {
        let intensity = if project_bloom_intensity.is_finite() {
            project_bloom_intensity.clamp(0.0, 5.0) as f32
        } else {
            1.0
        };
        Self {
            bloom_enabled: project_bloom_enabled && viewport_bloom,
            bloom_intensity: PostProcessSettings::default().bloom_intensity * intensity,
        }
    }

    /// Read the project graphics settings; missing values use the schema
    /// defaults (bloom on, intensity 1.0).
    pub fn from_project_settings(viewport_bloom: bool) -> Self {
        use engine_state::settings::{global_config, ConfigValue, NS_PROJECT};
        let setting = |key: &str| global_config().get(NS_PROJECT, "graphics", key).ok();
        let enabled = match setting("bloom_enabled") {
            Some(ConfigValue::Bool(value)) => value,
            _ => true,
        };
        let intensity = match setting("bloom_intensity") {
            Some(ConfigValue::Float(value)) => value,
            Some(ConfigValue::Int(value)) => value as f64,
            _ => 1.0,
        };
        Self::resolve(enabled, intensity, viewport_bloom)
    }

    /// The resolver defaults this baseline stands for.
    pub fn settings(self) -> PostProcessSettings {
        let mut settings = PostProcessSettings::default();
        settings.bloom_enabled = self.bloom_enabled;
        settings.bloom_intensity = self.bloom_intensity;
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_project_setting_both_gate_bloom() {
        assert!(EditorPostProcess::resolve(true, 1.0, true).bloom_enabled);
        assert!(
            !EditorPostProcess::resolve(true, 1.0, false).bloom_enabled,
            "toolbar off"
        );
        assert!(
            !EditorPostProcess::resolve(false, 1.0, true).bloom_enabled,
            "project off"
        );
    }

    #[test]
    fn project_intensity_scales_helio_default() {
        let helio_default = PostProcessSettings::default().bloom_intensity;
        assert_eq!(
            EditorPostProcess::resolve(true, 1.0, true).bloom_intensity,
            helio_default
        );
        assert_eq!(
            EditorPostProcess::resolve(true, 2.0, true).bloom_intensity,
            helio_default * 2.0
        );
        assert_eq!(
            EditorPostProcess::resolve(true, 9.0, true).bloom_intensity,
            helio_default * 5.0,
            "clamped to the schema range"
        );
        assert_eq!(
            EditorPostProcess::resolve(true, f64::NAN, true).bloom_intensity,
            helio_default
        );
    }

    #[test]
    fn the_baseline_changes_only_bloom() {
        let off = EditorPostProcess::resolve(true, 1.0, false).settings();
        let mut expected = PostProcessSettings::default();
        expected.bloom_enabled = false;
        assert_eq!(
            bytemuck::bytes_of(&off.to_gpu()),
            bytemuck::bytes_of(&expected.to_gpu())
        );
    }
}
