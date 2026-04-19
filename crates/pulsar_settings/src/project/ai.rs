use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "ai";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("AI", "Artificial intelligence and navigation settings")
        .setting("navmesh_agent_radius",
            SchemaEntry::new("Default navigation agent radius (m)", 0.35_f64)
                .label("Agent Radius (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.01), max: Some(10.0), step: Some(0.01) })
                .validator(Validator::float_range(0.01, 10.0)))
        .setting("navmesh_agent_height",
            SchemaEntry::new("Default navigation agent height (m)", 1.8_f64)
                .label("Agent Height (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.1), max: Some(20.0), step: Some(0.1) })
                .validator(Validator::float_range(0.1, 20.0)))
        .setting("navmesh_cell_size",
            SchemaEntry::new("Navmesh voxel cell size (m) — smaller = more detail, more memory", 0.3_f64)
                .label("Cell Size (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.05), max: Some(2.0), step: Some(0.05) })
                .validator(Validator::float_range(0.05, 2.0)))
        .setting("navmesh_step_height",
            SchemaEntry::new("Maximum step height an agent can climb (m)", 0.25_f64)
                .label("Max Step Height (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(2.0), step: Some(0.05) })
                .validator(Validator::float_range(0.0, 2.0)))
        .setting("navmesh_max_slope",
            SchemaEntry::new("Maximum walkable slope angle in degrees", 45.0_f64)
                .label("Max Slope (°)").page("AI")
                .field_type(FieldType::Slider { min: 0.0, max: 90.0, step: 1.0 })
                .validator(Validator::float_range(0.0, 90.0)))
        .setting("pathfinding_budget_us",
            SchemaEntry::new("CPU budget for pathfinding per frame in microseconds", 1000_i64)
                .label("Pathfinding Budget (µs)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(16000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 16_000)))
        .setting("behavior_tree_tick_hz",
            SchemaEntry::new("How many times per second behavior trees are evaluated", 10_i64)
                .label("Behavior Tree Tick (Hz)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(60.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 60)))
        .setting("perception_range",
            SchemaEntry::new("Default AI perception/sight range in meters", 20.0_f64)
                .label("Perception Range (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(500.0), step: Some(1.0) })
                .validator(Validator::float_range(1.0, 500.0)))
        .setting("debug_navmesh",
            SchemaEntry::new("Overlay navmesh visualization in the editor viewport", false)
                .label("Debug Navmesh").page("AI")
                .field_type(FieldType::Checkbox))
        .setting("debug_paths",
            SchemaEntry::new("Draw AI pathfinding results in the editor viewport", false)
                .label("Debug Paths").page("AI")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
