use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "world";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("World", "World partition, streaming, and environment settings")
        .setting("world_partition_enabled",
            SchemaEntry::new("Enable world partition for large open worlds", false)
                .label("World Partition").page("World")
                .field_type(FieldType::Checkbox))
        .setting("cell_size",
            SchemaEntry::new("World partition cell size in meters", 512_i64)
                .label("Cell Size (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(8192.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 8192)))
        .setting("streaming_distance",
            SchemaEntry::new("Radius around the camera to stream in level content (m)", 2000.0_f64)
                .label("Streaming Distance (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(50000.0), step: Some(100.0) })
                .validator(Validator::float_range(100.0, 50000.0)))
        .setting("hlod_enabled",
            SchemaEntry::new("Enable hierarchical LOD for distant world cells", false)
                .label("HLOD").page("World")
                .field_type(FieldType::Checkbox))
        .setting("origin_rebasing",
            SchemaEntry::new("Shift the world origin near the camera to prevent float precision issues", true)
                .label("Origin Rebasing").page("World")
                .field_type(FieldType::Checkbox))
        .setting("gravity",
            SchemaEntry::new("World gravity vector Y component (m/s²)", -9.81_f64)
                .label("Gravity (m/s²)").page("World")
                .field_type(FieldType::NumberInput { min: Some(-100.0), max: Some(0.0), step: Some(0.1) })
                .validator(Validator::float_range(-100.0, 0.0)))
        .setting("sky_type",
            SchemaEntry::new("Sky rendering mode", "procedural")
                .label("Sky Type").page("World")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Solid Color", "solid"),
                    DropdownOption::new("Gradient", "gradient"),
                    DropdownOption::new("Skybox", "skybox"),
                    DropdownOption::new("Procedural (Atmospheric)", "procedural"),
                ]}))
        .setting("sun_angle",
            SchemaEntry::new("Angle of the directional sun light (degrees from horizon)", 45.0_f64)
                .label("Sun Angle (°)").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 90.0, step: 0.5 })
                .validator(Validator::float_range(0.0, 90.0)))
        .setting("enable_weather",
            SchemaEntry::new("Enable procedural weather system", false)
                .label("Weather System").page("World")
                .field_type(FieldType::Checkbox))
        .setting("occlusion_culling",
            SchemaEntry::new("Enable GPU-based occlusion culling for hidden geometry", true)
                .label("Occlusion Culling").page("World")
                .field_type(FieldType::Checkbox))
        .setting("time_of_day",
            SchemaEntry::new("Current time of day in 24-hour decimal format (e.g. 14.5 = 14:30)", 12.0_f64)
                .label("Time of Day").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 24.0, step: 0.1 })
                .validator(Validator::float_range(0.0, 24.0)))
        .setting("day_length_minutes",
            SchemaEntry::new("Duration of a full in-game day cycle in real-time minutes (0 = frozen)", 0.0_f64)
                .label("Day Length (real minutes)").page("World")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(1440.0), step: Some(1.0) })
                .validator(Validator::float_range(0.0, 1440.0)))
        .setting("sun_azimuth",
            SchemaEntry::new("Horizontal angle of the sun (degrees, 0 = North)", 180.0_f64)
                .label("Sun Azimuth (°)").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 360.0, step: 1.0 })
                .validator(Validator::float_range(0.0, 360.0)))
        .setting("wind_speed",
            SchemaEntry::new("Wind speed for foliage and particle simulations (m/s)", 3.0_f64)
                .label("Wind Speed (m/s)").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 50.0, step: 0.5 })
                .validator(Validator::float_range(0.0, 50.0)))
        .setting("wind_direction",
            SchemaEntry::new("Wind direction in degrees (0 = North, clockwise)", 0.0_f64)
                .label("Wind Direction (°)").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 360.0, step: 1.0 })
                .validator(Validator::float_range(0.0, 360.0)))
        .setting("ambient_temperature",
            SchemaEntry::new("Ambient temperature in Celsius used by environmental simulation", 20.0_f64)
                .label("Temperature (°C)").page("World")
                .field_type(FieldType::NumberInput { min: Some(-80.0), max: Some(60.0), step: Some(0.5) })
                .validator(Validator::float_range(-80.0, 60.0)))
        .setting("fog_density",
            SchemaEntry::new("Atmospheric fog density coefficient", 0.0_f64)
                .label("Fog Density").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.001 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("fog_start_distance",
            SchemaEntry::new("Distance at which exponential fog begins (m)", 100.0_f64)
                .label("Fog Start (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(10000.0), step: Some(10.0) })
                .validator(Validator::float_range(0.0, 10000.0)))
        .setting("cloud_coverage",
            SchemaEntry::new("Cloud coverage fraction for procedural sky (0.0 = clear, 1.0 = overcast)", 0.3_f64)
                .label("Cloud Coverage").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("rain_intensity",
            SchemaEntry::new("Rainfall intensity for the weather system (0 = none, 1 = heavy)", 0.0_f64)
                .label("Rain Intensity").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("snow_intensity",
            SchemaEntry::new("Snowfall intensity (0 = none, 1 = blizzard)", 0.0_f64)
                .label("Snow Intensity").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("water_plane_enabled",
            SchemaEntry::new("Render a global infinite water plane at ocean level", false)
                .label("Global Water Plane").page("World")
                .field_type(FieldType::Checkbox))
        .setting("ocean_level",
            SchemaEntry::new("Height of the global ocean water plane in world units", 0.0_f64)
                .label("Ocean Level (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(-10000.0), max: Some(10000.0), step: Some(1.0) })
                .validator(Validator::float_range(-10000.0, 10000.0)))
        .setting("ambient_sound_enabled",
            SchemaEntry::new("Play procedural ambient soundscape based on environment zone", true)
                .label("Ambient Soundscape").page("World")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
