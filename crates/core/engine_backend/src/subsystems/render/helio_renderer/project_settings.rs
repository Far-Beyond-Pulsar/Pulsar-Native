//! The project's renderer settings, read through Helio's description of
//! them (`helio_settings`; `pulsar_settings::project::rendering` registers
//! the same description). The editor viewport and the game build their
//! renderers from these.

use helio::helio_settings::{self, Group, Resolved};

/// `group`'s values in the project settings (defaults where unset or
/// invalid).
pub fn resolved(group: &'static Group) -> Resolved {
    resolved_from(engine_state::settings::global_config(), group)
}

fn resolved_from(
    config: &engine_state::settings::ConfigManager,
    group: &'static Group,
) -> Resolved {
    group.resolve(|key| {
        config
            .get(engine_state::settings::NS_PROJECT, group.key, key)
            .ok()
            .and_then(pulsar_settings::project::rendering::helio_value)
    })
}

/// The renderer's settings.
pub fn renderer() -> Resolved {
    resolved(&helio_settings::RENDERER)
}

/// Voxel worlds' residency settings.
pub fn voxels() -> Resolved {
    resolved(&helio_settings::VOXEL_RESIDENCY)
}

/// A renderer config with the project's settings.
pub fn renderer_config(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    settings: &Resolved,
) -> helio::RendererConfig {
    helio::RendererConfig::new(width, height, format).with_settings(settings)
}

/// The TSR preset to render with: the project's, or with "Automatic" the
/// scene's preference (`scene`).
pub fn tsr_quality(
    settings: &Resolved,
    scene: Option<helio::TsrQuality>,
) -> Option<helio::TsrQuality> {
    helio::TsrSetting::from_settings(settings).resolve(scene)
}

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// The project settings the renderer's graph is built from: a change to
/// any of them rebuilds it (live ones apply per frame).
#[derive(Clone)]
pub(super) struct ProjectGraphSettings {
    pub renderer: helio::helio_settings::Resolved,
    pub voxels: helio::helio_settings::Resolved,
}

/// Resolve settings only when a relevant project value changes. Subscribe
/// before the first load, and clear the dirty bit before reading values so a
/// concurrent edit requests another refresh on the next frame.
pub(super) struct ProjectGraphSettingsCache {
    config: engine_state::settings::ConfigManager,
    dirty: Arc<AtomicBool>,
    _listener: engine_state::settings::ListenerId,
    current: Option<Arc<ProjectGraphSettings>>,
}

impl ProjectGraphSettingsCache {
    pub fn new() -> Self {
        Self::with_config(engine_state::settings::global_config().clone())
    }

    fn with_config(config: engine_state::settings::ConfigManager) -> Self {
        let dirty = Arc::new(AtomicBool::new(true));
        let changed = Arc::clone(&dirty);
        let listener = config.on_any_change(move |event| {
            if event.namespace == engine_state::settings::NS_PROJECT
                && event.owner.len() == 1
                && (event.owner[0] == helio::helio_settings::RENDERER.key
                    || event.owner[0] == helio::helio_settings::VOXEL_RESIDENCY.key)
            {
                changed.store(true, Ordering::Release);
            }
        });
        Self {
            config,
            dirty,
            _listener: listener,
            current: None,
        }
    }

    pub fn load(&mut self) -> Arc<ProjectGraphSettings> {
        if self.dirty.load(Ordering::Acquire) || self.current.is_none() {
            self.dirty.store(false, Ordering::Release);
            self.current = Some(Arc::new(ProjectGraphSettings {
                renderer: resolved_from(&self.config, &helio_settings::RENDERER),
                voxels: resolved_from(&self.config, &helio_settings::VOXEL_RESIDENCY),
            }));
        }
        Arc::clone(self.current.as_ref().expect("settings loaded"))
    }
}

impl ProjectGraphSettings {
    pub fn same_graph(&self, other: &Self) -> bool {
        self.renderer.same_graph(&other.renderer) && self.voxels.same_graph(&other.voxels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::settings::{ConfigManager, NamespaceSchema, SchemaEntry};

    #[test]
    fn settings_are_reused_until_a_relevant_value_changes() {
        let config = ConfigManager::new();
        let rendering = config
            .register(
                "project",
                "rendering",
                NamespaceSchema::new("Rendering", "test")
                    .setting("render_scale", SchemaEntry::new("Scale", 1.0)),
            )
            .unwrap();
        let unrelated = config
            .register(
                "editor",
                "rendering",
                NamespaceSchema::new("Editor", "test")
                    .setting("enabled", SchemaEntry::new("Enabled", true)),
            )
            .unwrap();
        let mut cache = ProjectGraphSettingsCache::with_config(config);
        let first = cache.load();
        assert!(Arc::ptr_eq(&first, &cache.load()));
        unrelated.set("enabled", false).unwrap();
        assert!(Arc::ptr_eq(&first, &cache.load()));
        rendering.set("render_scale", 0.5).unwrap();
        let changed = cache.load();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(changed.renderer.number("render_scale"), 0.5);
        assert!(Arc::ptr_eq(&changed, &cache.load()));
    }
}
