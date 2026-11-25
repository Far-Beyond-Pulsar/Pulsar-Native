use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "renderer";

/// Whether the engine's device asks for hardware ray queries where the
/// adapter runs them (`engine_state::settings::hardware_ray_queries` reads
/// it). On unless the user turns it off: ray tracing is the renderer's
/// default tier, and the tier without it is what an adapter that has none
/// (or this setting, off) gets.
pub const HARDWARE_RAY_QUERIES: &str = "hardware_ray_queries";
pub const HARDWARE_RAY_QUERIES_DEFAULT: bool = true;

/// How a game window's frames reach the display: `fifo` (vsync, the default),
/// `fifo_relaxed`, `mailbox`, `immediate`, `auto_vsync` or `auto_no_vsync`. A mode
/// the surface does not offer falls back to `fifo`, which every surface has.
pub const PRESENT_MODE: &str = "present_mode";
pub const PRESENT_MODE_DEFAULT: &str = "fifo";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Renderer Device",
        "GPU device and surface options; changes require restarting the editor",
    )
    .setting(
        "backend_preference",
        SchemaEntry::new("Graphics API backend (Automatic prefers Vulkan)", "auto")
            .label("Graphics Backend")
            .page("Renderer / Device")
            .requires_restart()
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("Automatic", "auto"),
                    DropdownOption::new("Vulkan", "vulkan"),
                    DropdownOption::new("DX12", "dx12"),
                    DropdownOption::new("Metal", "metal"),
                    DropdownOption::new("OpenGL", "gl"),
                ],
            }),
    )
    .setting(
        "gpu_preference",
        SchemaEntry::new("GPU adapter selection policy", "high_performance")
            .label("GPU Preference")
            .page("Renderer / Device")
            .requires_restart()
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("High Performance", "high_performance"),
                    DropdownOption::new("Low Power", "low_power"),
                    DropdownOption::new("Automatic", "auto"),
                ],
            }),
    )
    .setting(
        HARDWARE_RAY_QUERIES,
        SchemaEntry::new(
            "Trace rays with the GPU's hardware ray queries when it has them. Meshes are traced \
             with them (global illumination, traced reflections and shadows); without, the \
             renderer uses its tier that needs none",
            HARDWARE_RAY_QUERIES_DEFAULT,
        )
        .label("Hardware Ray Queries")
        .page("Renderer / Device")
        .requires_restart()
        .field_type(FieldType::Checkbox),
    )
    .setting(
        PRESENT_MODE,
        SchemaEntry::new(
            "How a game window's frames reach the display. Fifo waits for the display's refresh (no tearing); Mailbox replaces the queued frame with the newest (no tearing, the lowest latency a compositor allows, where offered); Immediate presents at once and may tear. A mode the display does not offer uses Fifo",
            PRESENT_MODE_DEFAULT,
        )
        .label("Present Mode")
        .page("Renderer / Device")
        .requires_restart()
        .field_type(FieldType::Dropdown {
            options: vec![
                DropdownOption::new("Fifo (vsync)", "fifo"),
                DropdownOption::new("Fifo Relaxed", "fifo_relaxed"),
                DropdownOption::new("Mailbox", "mailbox"),
                DropdownOption::new("Immediate (may tear)", "immediate"),
                DropdownOption::new("Automatic, vsync", "auto_vsync"),
                DropdownOption::new("Automatic, no vsync", "auto_no_vsync"),
            ],
        }),
    )
    .setting(
        "max_frame_latency",
        SchemaEntry::new("Maximum frames queued for presentation", 1_i64)
            .label("Maximum Frame Latency")
            .page("Renderer / Device")
            .requires_restart()
            .field_type(FieldType::NumberInput {
                min: Some(1.0),
                max: Some(4.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(1, 4)),
    );
    let _ = cfg.register(NS, OWNER, schema);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every renderer device setting is read when the GPU device is created,
    /// so each one tells the settings screen it applies after a restart.
    #[test]
    fn every_device_setting_requires_a_restart() {
        let cfg: &'static ConfigManager = Box::leak(Box::new(ConfigManager::new()));
        register(cfg);
        let settings = cfg
            .list_settings(NS, OWNER)
            .expect("the renderer owner is registered");
        assert!(settings.iter().any(|s| s.key == HARDWARE_RAY_QUERIES));
        // Ray tracing is the default tier: a fresh install asks for ray queries.
        let ray_queries = cfg.get(NS, OWNER, HARDWARE_RAY_QUERIES).ok();
        assert_eq!(
            ray_queries.and_then(|value| value.as_bool().ok()),
            Some(true)
        );
        for setting in settings {
            assert!(
                setting.requires_restart(),
                "{} must require a restart",
                setting.key
            );
        }
    }
}
