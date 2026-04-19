use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "graphics";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Graphics", "Real-time graphics feature toggles and quality")
        .setting("renderer",
            SchemaEntry::new("Graphics API backend", "auto")
                .label("Renderer").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto", "auto"),
                    DropdownOption::new("Vulkan", "vulkan"),
                    DropdownOption::new("DirectX 12", "dx12"),
                    DropdownOption::new("Metal", "metal"),
                    DropdownOption::new("OpenGL (legacy)", "opengl"),
                ]}))
        .setting("msaa_samples",
            SchemaEntry::new("Multi-sample anti-aliasing sample count (0 = off)", "4")
                .label("MSAA Samples").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "0"),
                    DropdownOption::new("2×", "2"),
                    DropdownOption::new("4×", "4"),
                    DropdownOption::new("8×", "8"),
                ]}))
        .setting("anti_aliasing",
            SchemaEntry::new("Anti-aliasing technique for final image", "taa")
                .label("Anti-Aliasing").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("FXAA", "fxaa"),
                    DropdownOption::new("SMAA", "smaa"),
                    DropdownOption::new("TAA", "taa"),
                    DropdownOption::new("DLAA", "dlaa"),
                ]}))
        .setting("max_fps",
            SchemaEntry::new("Maximum frames per second the game will render (0 = unlimited)", 0_i64)
                .label("Max FPS").page("Graphics")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(360.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 360)))
        .setting("shadow_quality",
            SchemaEntry::new("Shadow rendering quality preset", "high")
                .label("Shadow Quality").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "off"),
                    DropdownOption::new("Low", "low"),
                    DropdownOption::new("Medium", "medium"),
                    DropdownOption::new("High", "high"),
                    DropdownOption::new("Ultra", "ultra"),
                ]})
                .validator(Validator::string_one_of(["off", "low", "medium", "high", "ultra"])))
        .setting("shadow_distance",
            SchemaEntry::new("Maximum distance at which shadows are cast (m)", 500.0_f64)
                .label("Shadow Distance (m)").page("Graphics")
                .field_type(FieldType::NumberInput { min: Some(10.0), max: Some(10000.0), step: Some(10.0) })
                .validator(Validator::float_range(10.0, 10000.0)))
        .setting("shadow_cascades",
            SchemaEntry::new("Number of cascades for cascaded shadow maps (CSM)", 4_i64)
                .label("Shadow Cascades").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("1", "1"),
                    DropdownOption::new("2", "2"),
                    DropdownOption::new("4", "4"),
                    DropdownOption::new("8", "8"),
                ]}))
        .setting("texture_quality",
            SchemaEntry::new("Global texture resolution multiplier", "full")
                .label("Texture Quality").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Quarter", "quarter"),
                    DropdownOption::new("Half", "half"),
                    DropdownOption::new("Full", "full"),
                ]})
                .validator(Validator::string_one_of(["quarter", "half", "full"])))
        .setting("texture_filtering",
            SchemaEntry::new("Texture filtering mode", "anisotropic_16x")
                .label("Texture Filtering").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Nearest", "nearest"),
                    DropdownOption::new("Bilinear", "bilinear"),
                    DropdownOption::new("Trilinear", "trilinear"),
                    DropdownOption::new("Anisotropic 2×", "anisotropic_2x"),
                    DropdownOption::new("Anisotropic 4×", "anisotropic_4x"),
                    DropdownOption::new("Anisotropic 8×", "anisotropic_8x"),
                    DropdownOption::new("Anisotropic 16×", "anisotropic_16x"),
                ]}))
        .setting("ambient_occlusion",
            SchemaEntry::new("Screen-space ambient occlusion technique", "ssao")
                .label("Ambient Occlusion").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "off"),
                    DropdownOption::new("SSAO", "ssao"),
                    DropdownOption::new("HBAO", "hbao"),
                    DropdownOption::new("GTAO", "gtao"),
                ]}))
        .setting("bloom_enabled",
            SchemaEntry::new("Enable bloom glow effect on bright areas", true)
                .label("Bloom").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("bloom_intensity",
            SchemaEntry::new("Intensity of the bloom effect", 1.0_f64)
                .label("Bloom Intensity").page("Graphics")
                .field_type(FieldType::Slider { min: 0.0, max: 5.0, step: 0.1 })
                .validator(Validator::float_range(0.0, 5.0)))
        .setting("motion_blur",
            SchemaEntry::new("Enable per-object and camera motion blur", false)
                .label("Motion Blur").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("depth_of_field",
            SchemaEntry::new("Enable camera depth-of-field blur effect", false)
                .label("Depth of Field").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("lens_flare",
            SchemaEntry::new("Enable lens flare effects from bright light sources", false)
                .label("Lens Flare").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("chromatic_aberration",
            SchemaEntry::new("Enable chromatic aberration post-process effect", false)
                .label("Chromatic Aberration").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("vignette",
            SchemaEntry::new("Enable screen vignette darkening at edges", false)
                .label("Vignette").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("fog_enabled",
            SchemaEntry::new("Enable atmospheric fog", false)
                .label("Fog").page("Graphics")
                .field_type(FieldType::Checkbox))
        .setting("lod_bias",
            SchemaEntry::new("Bias applied to LOD selection distance (negative = higher quality)", 0.0_f64)
                .label("LOD Bias").page("Graphics")
                .field_type(FieldType::Slider { min: -2.0, max: 2.0, step: 0.1 })
                .validator(Validator::float_range(-2.0, 2.0)));

    let _ = cfg.register(NS, OWNER, schema);
}
