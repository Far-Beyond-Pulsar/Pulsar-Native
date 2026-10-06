use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "rendering";

fn choices(values: &[(&str, &str)]) -> Vec<DropdownOption> {
    values
        .iter()
        .map(|(label, value)| DropdownOption::new(*label, *value))
        .collect()
}

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Rendering", "Project rendering quality and effects")
        .setting(
            "render_scale",
            SchemaEntry::new("Internal render resolution scale", 0.75_f64)
                .label("Render Scale")
                .page("Rendering / Quality")
                .field_type(FieldType::Slider {
                    min: 0.25,
                    max: 1.0,
                    step: 0.05,
                })
                .validator(Validator::float_range(0.25, 1.0)),
        )
        .setting(
            "tsr_quality",
            SchemaEntry::new("Temporal upscaling quality preset", "off")
                .label("Temporal Super Resolution")
                .page("Rendering / Quality")
                .field_type(FieldType::Dropdown {
                    options: choices(&[
                        ("Off", "off"),
                        ("Performance", "performance"),
                        ("Balanced", "balanced"),
                        ("Quality", "quality"),
                        ("Native", "native"),
                    ]),
                }),
        )
        .setting(
            "shadow_quality",
            SchemaEntry::new("Shadow filtering quality", "medium")
                .label("Shadow Quality")
                .page("Rendering / Quality")
                .field_type(FieldType::Dropdown {
                    options: choices(&[
                        ("Low", "low"),
                        ("Medium", "medium"),
                        ("High", "high"),
                        ("Ultra", "ultra"),
                    ]),
                }),
        )
        .setting(
            "shadow_atlas_size",
            SchemaEntry::new(
                "Shadow atlas resolution; larger values use substantially more GPU memory",
                1024_i64,
            )
            .label("Shadow Atlas Size")
            .page("Rendering / Quality")
            .field_type(FieldType::Dropdown {
                options: choices(&[
                    ("512", "512"),
                    ("1024", "1024"),
                    ("2048", "2048"),
                    ("4096", "4096"),
                ]),
            }),
        )
        .setting(
            "screen_space_reflections",
            SchemaEntry::new("Enable screen-space reflections", false)
                .label("Screen Space Reflections")
                .page("Rendering / Effects")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "planar_reflections",
            SchemaEntry::new("Render authored planar reflection surfaces", false)
                .label("Planar Reflections")
                .page("Rendering / Effects")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "render_mode",
            SchemaEntry::new(
                "Renderer path; changing it rebuilds the render graph",
                "deferred",
            )
            .label("Render Mode")
            .page("Rendering / Advanced")
            .field_type(FieldType::Dropdown {
                options: choices(&[
                    ("Deferred", "deferred"),
                    ("Forward Opaque", "forward_opaque"),
                    ("Forward Only", "forward_only"),
                ]),
            }),
        );
    let _ = cfg.register(NS, OWNER, schema);
}
