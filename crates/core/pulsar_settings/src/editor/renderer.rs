use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "renderer";

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
        "hardware_ray_queries",
        SchemaEntry::new(
            "Request experimental hardware ray queries when the GPU supports them. Global \
             illumination traces meshes with them: without, mesh scenes get no GI",
            false,
        )
        .label("Hardware Ray Queries")
        .page("Renderer / Device")
        .requires_restart()
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "max_frame_latency",
        SchemaEntry::new("Maximum frames queued for presentation", 2_i64)
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
        let settings = cfg.list_settings(NS, OWNER).expect("the renderer owner is registered");
        assert!(settings.iter().any(|s| s.key == "hardware_ray_queries"));
        for setting in settings {
            assert!(setting.requires_restart(), "{} must require a restart", setting.key);
        }
    }
}
