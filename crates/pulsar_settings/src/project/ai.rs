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
                .field_type(FieldType::Checkbox))
        .setting("crowd_simulation",
            SchemaEntry::new("Enable RVO crowd simulation for groups of agents", false)
                .label("Crowd Simulation").page("AI")
                .field_type(FieldType::Checkbox))
        .setting("max_crowd_agents",
            SchemaEntry::new("Maximum number of agents that participate in crowd simulation simultaneously", 128_i64)
                .label("Max Crowd Agents").page("AI")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(4096.0), step: Some(8.0) })
                .validator(Validator::int_range(8, 4096)))
        .setting("avoidance_algorithm",
            SchemaEntry::new("Local avoidance algorithm for agents avoiding each other", "rvo2")
                .label("Avoidance Algorithm").page("AI")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("RVO2", "rvo2"),
                    DropdownOption::new("ORCA", "orca"),
                    DropdownOption::new("Reciprocal Velocity Obstacles", "rvo"),
                ]})
                .validator(Validator::string_one_of(["none", "rvo2", "orca", "rvo"])))
        .setting("pathfinding_algorithm",
            SchemaEntry::new("Graph search algorithm used for pathfinding", "astar")
                .label("Pathfinding Algorithm").page("AI")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("A*", "astar"),
                    DropdownOption::new("Dijkstra", "dijkstra"),
                    DropdownOption::new("Theta* (smoother paths)", "theta_star"),
                    DropdownOption::new("Jump Point Search", "jps"),
                ]})
                .validator(Validator::string_one_of(["astar", "dijkstra", "theta_star", "jps"])))
        .setting("heuristic",
            SchemaEntry::new("Heuristic function used by A* pathfinding", "manhattan")
                .label("A* Heuristic").page("AI")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Manhattan", "manhattan"),
                    DropdownOption::new("Euclidean", "euclidean"),
                    DropdownOption::new("Chebyshev", "chebyshev"),
                    DropdownOption::new("Octile", "octile"),
                ]})
                .validator(Validator::string_one_of(["manhattan", "euclidean", "chebyshev", "octile"])))
        .setting("navmesh_auto_rebuild",
            SchemaEntry::new("Rebuild the navmesh automatically when level geometry changes", true)
                .label("Auto-Rebuild Navmesh").page("AI")
                .field_type(FieldType::Checkbox))
        .setting("navmesh_build_threads",
            SchemaEntry::new("Number of threads used for background navmesh building (0 = auto)", 0_i64)
                .label("Navmesh Build Threads").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 32)))
        .setting("navmesh_tile_size",
            SchemaEntry::new("NavMesh tile size in world units (larger = fewer tiles, less granularity)", 128_i64)
                .label("Navmesh Tile Size").page("AI")
                .field_type(FieldType::NumberInput { min: Some(16.0), max: Some(2048.0), step: Some(16.0) })
                .validator(Validator::int_range(16, 2048)))
        .setting("steering_force",
            SchemaEntry::new("Magnitude of the steering force applied per frame to guide agents", 1.0_f64)
                .label("Steering Force").page("AI")
                .field_type(FieldType::Slider { min: 0.1, max: 10.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 10.0)))
        .setting("max_angular_speed_deg",
            SchemaEntry::new("Maximum turning speed for agents in degrees per second", 360.0_f64)
                .label("Max Turn Speed (°/s)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(10.0), max: Some(1080.0), step: Some(10.0) })
                .validator(Validator::float_range(10.0, 1080.0)))
        .setting("los_check_interval_ms",
            SchemaEntry::new("Interval between line-of-sight checks for perception in milliseconds", 100_i64)
                .label("LoS Check Interval (ms)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(16.0), max: Some(1000.0), step: Some(16.0) })
                .validator(Validator::int_range(16, 1000)))
        .setting("hearing_range",
            SchemaEntry::new("Default AI hearing range in meters", 10.0_f64)
                .label("Hearing Range (m)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(500.0), step: Some(1.0) })
                .validator(Validator::float_range(0.0, 500.0)))
        .setting("forget_time_seconds",
            SchemaEntry::new("Time in seconds before an AI loses track of a stimulus it can no longer detect", 5.0_f64)
                .label("Forget Time (s)").page("AI")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(120.0), step: Some(0.5) })
                .validator(Validator::float_range(0.0, 120.0)));

    let _ = cfg.register(NS, OWNER, schema);
}
