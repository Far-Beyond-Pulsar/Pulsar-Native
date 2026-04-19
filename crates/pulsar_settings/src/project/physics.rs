use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "physics";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Physics", "Physics engine configuration")
        .setting("physics_backend",
            SchemaEntry::new("Physics simulation library to use", "rapier")
                .label("Physics Backend").page("Physics")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Rapier (default)", "rapier"),
                    DropdownOption::new("Jolt", "jolt"),
                    DropdownOption::new("Bullet", "bullet"),
                    DropdownOption::new("PhysX", "physx"),
                ]}))
        .setting("gravity_scale",
            SchemaEntry::new("Global multiplier applied to all gravitational forces", 1.0_f64)
                .label("Gravity Scale").page("Physics")
                .field_type(FieldType::Slider { min: 0.0, max: 5.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 5.0)))
        .setting("solver_iterations",
            SchemaEntry::new("Constraint solver velocity iterations per substep", 8_i64)
                .label("Solver Iterations").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 64)))
        .setting("solver_position_iterations",
            SchemaEntry::new("Constraint solver position iterations per substep", 4_i64)
                .label("Position Iterations").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 32)))
        .setting("substep_enabled",
            SchemaEntry::new("Enable physics sub-stepping for higher accuracy", false)
                .label("Substepping").page("Physics")
                .field_type(FieldType::Checkbox))
        .setting("max_substeps",
            SchemaEntry::new("Maximum physics sub-steps per frame", 4_i64)
                .label("Max Substeps").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(16.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 16)))
        .setting("continuous_collision",
            SchemaEntry::new("Enable continuous collision detection (CCD) for fast objects", true)
                .label("CCD").page("Physics")
                .field_type(FieldType::Checkbox))
        .setting("sleeping_enabled",
            SchemaEntry::new("Allow physics bodies to sleep when they come to rest", true)
                .label("Body Sleeping").page("Physics")
                .field_type(FieldType::Checkbox))
        .setting("sleep_threshold",
            SchemaEntry::new("Linear velocity below which a body may go to sleep (m/s)", 0.05_f64)
                .label("Sleep Threshold (m/s)").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(0.001), max: Some(1.0), step: Some(0.001) })
                .validator(Validator::float_range(0.001, 1.0)))
        .setting("default_friction",
            SchemaEntry::new("Default surface friction coefficient for new colliders", 0.5_f64)
                .label("Default Friction").page("Physics")
                .field_type(FieldType::Slider { min: 0.0, max: 2.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 2.0)))
        .setting("default_restitution",
            SchemaEntry::new("Default bounciness coefficient for new colliders", 0.0_f64)
                .label("Default Restitution").page("Physics")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("collision_matrix_size",
            SchemaEntry::new("Number of physics collision layers", 32_i64)
                .label("Collision Layers").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(2.0), max: Some(64.0), step: Some(2.0) })
                .validator(Validator::int_range(2, 64)));

    let _ = cfg.register(NS, OWNER, schema);
}
