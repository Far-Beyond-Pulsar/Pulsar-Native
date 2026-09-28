//! The editor viewport's post-process baseline.
//!
//! Helio resolves post-processing on the GPU from its defaults, the camera's
//! `camera_postprocess` row and post-process volumes. The editor drives the
//! camera row for its viewport: bloom comes from the project's graphics
//! settings (`bloom_enabled`, `bloom_intensity`) and the toolbar's Bloom
//! toggle. Every other setting stays at Helio's default, which is what the
//! viewport rendered before this row existed. Volumes in the level still
//! override it.
//!
//! The row lives on its own entity without a `StableId`, so it never shows
//! up in the outliner or in saved levels.

use helio_pass_postprocess::{CameraPostProcessComponent, PostProcessSettings};

/// Camera view the editor viewport renders (Helio's default camera view).
pub const EDITOR_VIEW_ID: u32 = 0;

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

    fn settings(self) -> PostProcessSettings {
        let mut settings = PostProcessSettings::default();
        settings.bloom_enabled = self.bloom_enabled;
        settings.bloom_intensity = self.bloom_intensity;
        settings
    }
}

fn editor_row(
    world: &pulsar_scenedb::World,
) -> Option<(pulsar_scenedb::Entity, CameraPostProcessComponent)> {
    world
        .query::<&CameraPostProcessComponent>()
        .find(|(_, row)| row.view_id == EDITOR_VIEW_ID)
        .map(|(entity, row)| (entity, *row))
}

/// Whether the world's editor camera row already holds `desired`.
pub fn editor_postprocess_is_current(
    world: &pulsar_scenedb::World,
    desired: EditorPostProcess,
) -> bool {
    editor_row(world).is_some_and(|(_, row)| {
        row == CameraPostProcessComponent::new(EDITOR_VIEW_ID, &desired.settings())
    })
}

/// Write the editor camera row, creating its entity on first use (or after
/// the world was replaced, e.g. by loading a level).
pub fn apply_editor_postprocess(world: &mut pulsar_scenedb::World, desired: EditorPostProcess) {
    let entity = editor_row(world)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| world.spawn());
    world.insert(
        entity,
        CameraPostProcessComponent::new(EDITOR_VIEW_ID, &desired.settings()),
    );
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
    fn row_is_written_once_and_updated_in_place() {
        let mut world = pulsar_scenedb::World::new();
        let on = EditorPostProcess::resolve(true, 1.0, true);
        let off = EditorPostProcess::resolve(true, 1.0, false);
        assert!(!editor_postprocess_is_current(&world, on));

        apply_editor_postprocess(&mut world, on);
        assert!(editor_postprocess_is_current(&world, on));
        assert!(!editor_postprocess_is_current(&world, off));
        let row = editor_row(&world).unwrap().1.settings();
        assert_eq!(row.bloom_enabled, 1);

        apply_editor_postprocess(&mut world, off);
        assert!(editor_postprocess_is_current(&world, off));
        assert_eq!(
            world.query::<&CameraPostProcessComponent>().count(),
            1,
            "one row, updated in place"
        );
        assert_eq!(editor_row(&world).unwrap().1.settings().bloom_enabled, 0);
    }
}
