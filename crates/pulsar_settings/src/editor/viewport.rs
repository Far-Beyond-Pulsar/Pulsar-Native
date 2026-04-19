use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "viewport";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Viewport", "3D viewport camera and rendering settings")
        // ── Camera ─────────────────────────────────────────────────────────
        .setting("camera_speed",
            SchemaEntry::new("Base speed for editor camera movement (m/s)", 4.0_f64)
                .label("Camera Speed").page("Viewport")
                .field_type(FieldType::Slider { min: 0.1, max: 50.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 50.0)))
        .setting("camera_speed_boost",
            SchemaEntry::new("Speed multiplier applied when the sprint key is held", 5.0_f64)
                .label("Camera Speed Boost").page("Viewport")
                .field_type(FieldType::Slider { min: 1.0, max: 20.0, step: 0.5 })
                .validator(Validator::float_range(1.0, 20.0)))
        .setting("fov_degrees",
            SchemaEntry::new("Perspective field of view in degrees", 70_i64)
                .label("Camera FOV").page("Viewport")
                .field_type(FieldType::NumberInput { min: Some(20.0), max: Some(150.0), step: Some(1.0) })
                .validator(Validator::int_range(20, 150)))
        .setting("near_clip",
            SchemaEntry::new("Near clipping plane distance (m)", 0.1_f64)
                .label("Near Clip").page("Viewport")
                .field_type(FieldType::Slider { min: 0.001, max: 1.0, step: 0.001 })
                .validator(Validator::float_range(0.001, 1.0)))
        .setting("far_clip",
            SchemaEntry::new("Far clipping plane distance (m)", 100000.0_f64)
                .label("Far Clip").page("Viewport")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(1_000_000.0), step: Some(100.0) })
                .validator(Validator::float_range(100.0, 1_000_000.0)))
        .setting("invert_y",
            SchemaEntry::new("Invert the camera Y axis in viewport navigation", false)
                .label("Invert Y").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("mouse_sensitivity",
            SchemaEntry::new("Mouse sensitivity for viewport camera rotation", 1.0_f64)
                .label("Mouse Sensitivity").page("Viewport")
                .field_type(FieldType::Slider { min: 0.1, max: 10.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 10.0)))
        .setting("scroll_sensitivity",
            SchemaEntry::new("Scroll wheel sensitivity for dolly zoom", 1.0_f64)
                .label("Scroll Sensitivity").page("Viewport")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("camera_type",
            SchemaEntry::new("Default camera projection type", "perspective")
                .label("Camera Type").page("Viewport")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Perspective", "perspective"),
                    DropdownOption::new("Orthographic", "orthographic"),
                ]})
                .validator(Validator::string_one_of(["perspective", "orthographic"])))
        // ── Rendering ──────────────────────────────────────────────────────
        .setting("realtime_rendering",
            SchemaEntry::new("Render the viewport continuously (high GPU usage)", true)
                .label("Realtime Rendering").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("post_fx_quality",
            SchemaEntry::new("Post-processing quality in the editor viewport", "medium")
                .label("Post FX Quality").page("Viewport")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "off"),
                    DropdownOption::new("Low", "low"),
                    DropdownOption::new("Medium", "medium"),
                    DropdownOption::new("High", "high"),
                    DropdownOption::new("Cinematic", "cinematic"),
                ]})
                .validator(Validator::string_one_of(["off", "low", "medium", "high", "cinematic"])))
        .setting("wireframe_overlay",
            SchemaEntry::new("Show wireframe overlay on selected meshes", false)
                .label("Wireframe Overlay").page("Viewport")
                .field_type(FieldType::Checkbox))
        // ── Overlays & Helpers ─────────────────────────────────────────────
        .setting("show_grid",
            SchemaEntry::new("Show the world grid in the viewport", true)
                .label("Show Grid").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("grid_size",
            SchemaEntry::new("Primary grid cell size in meters", 1.0_f64)
                .label("Grid Size").page("Viewport")
                .field_type(FieldType::Slider { min: 0.01, max: 100.0, step: 0.01 })
                .validator(Validator::float_range(0.01, 100.0)))
        .setting("snap_to_grid",
            SchemaEntry::new("Snap object transforms to the grid by default", false)
                .label("Snap to Grid").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("show_gizmos",
            SchemaEntry::new("Show transform gizmos on selected objects", true)
                .label("Show Gizmos").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("gizmo_size",
            SchemaEntry::new("Screen-space size of transform gizmos", 1.0_f64)
                .label("Gizmo Size").page("Viewport")
                .field_type(FieldType::Slider { min: 0.3, max: 3.0, step: 0.05 })
                .validator(Validator::float_range(0.3, 3.0)))
        .setting("show_icons",
            SchemaEntry::new("Show billboard icons for lights, cameras, and empties", true)
                .label("Show Icons").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("show_stats",
            SchemaEntry::new("Display realtime rendering statistics overlay", false)
                .label("Show Stats").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("show_collision",
            SchemaEntry::new("Visualize collision shapes in the viewport", false)
                .label("Show Collision").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("show_navmesh",
            SchemaEntry::new("Visualize navigation meshes", false)
                .label("Show Navmesh").page("Viewport")
                .field_type(FieldType::Checkbox))
        .setting("lighting_mode",
            SchemaEntry::new("Viewport shading/lighting mode", "lit")
                .label("Lighting Mode").page("Viewport")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Lit", "lit"),
                    DropdownOption::new("Unlit", "unlit"),
                    DropdownOption::new("Wireframe", "wireframe"),
                    DropdownOption::new("Detail Lighting", "detail"),
                    DropdownOption::new("Reflections", "reflections"),
                    DropdownOption::new("Light Complexity", "light_complexity"),
                ]})
                .validator(Validator::string_one_of(["lit", "unlit", "wireframe", "detail", "reflections", "light_complexity"])));

    let _ = cfg.register(NS, OWNER, schema);
}
