use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "rendering";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Rendering", "Rendering pipeline and lighting configuration")
        .setting("render_pipeline",
            SchemaEntry::new("Rendering pipeline to use", "deferred")
                .label("Render Pipeline").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Deferred", "deferred"),
                    DropdownOption::new("Forward+", "forward_plus"),
                    DropdownOption::new("Forward (Mobile)", "forward"),
                ]}))
        .setting("hdr_enabled",
            SchemaEntry::new("Enable high dynamic range rendering", true)
                .label("HDR").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("tonemapper",
            SchemaEntry::new("Tonemapping operator applied to the HDR buffer", "aces")
                .label("Tonemapper").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Linear", "linear"),
                    DropdownOption::new("Reinhard", "reinhard"),
                    DropdownOption::new("ACES", "aces"),
                    DropdownOption::new("AgX", "agx"),
                    DropdownOption::new("GT Tonemap", "gt"),
                ]}))
        .setting("exposure_mode",
            SchemaEntry::new("Camera exposure control mode", "auto")
                .label("Exposure Mode").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto", "auto"),
                    DropdownOption::new("Manual", "manual"),
                ]})
                .validator(Validator::string_one_of(["auto", "manual"])))
        .setting("manual_exposure",
            SchemaEntry::new("Manual exposure value in EV100 (used when mode = manual)", 10.0_f64)
                .label("Manual Exposure (EV100)").page("Rendering")
                .field_type(FieldType::Slider { min: -10.0, max: 20.0, step: 0.1 })
                .validator(Validator::float_range(-10.0, 20.0)))
        .setting("global_illumination",
            SchemaEntry::new("Global illumination technique", "lumen")
                .label("Global Illumination").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Baked (Lightmaps)", "lightmaps"),
                    DropdownOption::new("SSGI", "ssgi"),
                    DropdownOption::new("Lumen (Dynamic)", "lumen"),
                    DropdownOption::new("Voxel GI", "voxel"),
                ]}))
        .setting("reflections",
            SchemaEntry::new("Real-time reflection technique", "ssr")
                .label("Reflections").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Reflection Captures", "captures"),
                    DropdownOption::new("SSR", "ssr"),
                    DropdownOption::new("Raytraced", "raytraced"),
                ]}))
        .setting("ray_tracing",
            SchemaEntry::new("Enable hardware ray tracing (requires RTX / RDNA3)", false)
                .label("Ray Tracing").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("ray_tracing_shadows",
            SchemaEntry::new("Use ray-traced shadows (requires ray tracing)", false)
                .label("RT Shadows").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("ray_tracing_ao",
            SchemaEntry::new("Use ray-traced ambient occlusion (requires ray tracing)", false)
                .label("RT Ambient Occlusion").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("upscaling",
            SchemaEntry::new("Temporal upscaling method for performance", "none")
                .label("Upscaling").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Bilinear", "bilinear"),
                    DropdownOption::new("DLSS", "dlss"),
                    DropdownOption::new("FSR 3", "fsr3"),
                    DropdownOption::new("XeSS", "xess"),
                ]}))
        .setting("upscaling_quality",
            SchemaEntry::new("Upscaling quality preset", "quality")
                .label("Upscaling Quality").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Ultra Performance", "ultra_performance"),
                    DropdownOption::new("Performance", "performance"),
                    DropdownOption::new("Balanced", "balanced"),
                    DropdownOption::new("Quality", "quality"),
                    DropdownOption::new("Ultra Quality", "ultra_quality"),
                ]}))
        .setting("frame_interpolation",
            SchemaEntry::new("Generate intermediate frames to boost perceived frame rate", false)
                .label("Frame Generation").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("color_grading_lut",
            SchemaEntry::new("Path to a 3D LUT for color grading (leave blank to disable)", "")
                .label("Color Grading LUT").page("Rendering")
                .field_type(FieldType::TextInput { placeholder: Some("assets/luts/cinematic.cube".into()), multiline: false }))
        .setting("color_grading_lut_intensity",
            SchemaEntry::new("Blend intensity of the color grading LUT", 1.0_f64)
                .label("LUT Intensity").page("Rendering")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("gi_method",
            SchemaEntry::new("Global illumination technique for indirect light", "none")
                .label("Global Illumination").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Baked Lightmaps", "lightmap"),
                    DropdownOption::new("DDGI (Dynamic)", "ddgi"),
                    DropdownOption::new("Lumen (Software)", "lumen_sw"),
                    DropdownOption::new("Lumen (Hardware RT)", "lumen_hw"),
                    DropdownOption::new("SSGI", "ssgi"),
                ]})
                .validator(Validator::string_one_of(["none", "lightmap", "ddgi", "lumen_sw", "lumen_hw", "ssgi"])))
        .setting("lightmap_resolution",
            SchemaEntry::new("Default lightmap texture resolution in texels", "512")
                .label("Default Lightmap Resolution").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("64", "64"),
                    DropdownOption::new("128", "128"),
                    DropdownOption::new("256", "256"),
                    DropdownOption::new("512", "512"),
                    DropdownOption::new("1024", "1024"),
                    DropdownOption::new("2048", "2048"),
                ]}))
        .setting("ray_tracing_enabled",
            SchemaEntry::new("Enable hardware ray tracing for shadows, reflections, and GI", false)
                .label("Ray Tracing").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("rt_reflections",
            SchemaEntry::new("Use ray-traced reflections (requires ray tracing to be enabled)", false)
                .label("RT Reflections").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("rt_shadows",
            SchemaEntry::new("Use ray-traced soft shadows (requires ray tracing to be enabled)", false)
                .label("RT Shadows").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("rt_ambient_occlusion",
            SchemaEntry::new("Use ray-traced ambient occlusion for accurate contact shadows", false)
                .label("RT Ambient Occlusion").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("upscaler",
            SchemaEntry::new("Temporal upscaling technique to reconstruct native resolution from lower input", "none")
                .label("Upscaler").page("Rendering")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("DLSS (NVIDIA)", "dlss"),
                    DropdownOption::new("FSR 3 (AMD)", "fsr3"),
                    DropdownOption::new("XeSS (Intel)", "xess"),
                    DropdownOption::new("TAA (built-in)", "taa"),
                ]})
                .validator(Validator::string_one_of(["none", "dlss", "fsr3", "xess", "taa"])))
        .setting("sky_atmosphere",
            SchemaEntry::new("Enable physically-based sky atmosphere scattering", false)
                .label("Sky Atmosphere").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("volumetric_clouds",
            SchemaEntry::new("Render volumetric clouds using raymarching", false)
                .label("Volumetric Clouds").page("Rendering")
                .field_type(FieldType::Checkbox))
        .setting("max_draw_calls",
            SchemaEntry::new("Maximum draw calls per frame before an overdraw warning is surfaced", 10_000_i64)
                .label("Max Draw Calls").page("Rendering")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(1_000_000.0), step: Some(1000.0) })
                .validator(Validator::int_range(100, 1_000_000)))
        .setting("triangle_budget",
            SchemaEntry::new("Target triangle count per frame in thousands", 5_000_i64)
                .label("Triangle Budget (K)").page("Rendering")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(100_000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 100_000)));

    let _ = cfg.register(NS, OWNER, schema);
}
