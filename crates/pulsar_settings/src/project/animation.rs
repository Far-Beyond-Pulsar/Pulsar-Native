use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "animation";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Animation", "Skeletal animation and blend tree configuration")
        .setting("max_bones_per_mesh",
            SchemaEntry::new("Maximum number of bones influencing a single skinned mesh", 256_i64)
                .label("Max Bones per Mesh").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(32.0), max: Some(1024.0), step: Some(32.0) })
                .validator(Validator::int_range(32, 1024)))
        .setting("max_bone_weights",
            SchemaEntry::new("Maximum bone influences per vertex", "4")
                .label("Max Bone Weights").page("Animation")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("1 (rigid)", "1"),
                    DropdownOption::new("2", "2"),
                    DropdownOption::new("4 (standard)", "4"),
                    DropdownOption::new("8 (high quality)", "8"),
                ]}))
        .setting("blend_tree_depth",
            SchemaEntry::new("Maximum depth of animation blend trees", 8_i64)
                .label("Blend Tree Depth").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(2.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(2, 32)))
        .setting("root_motion",
            SchemaEntry::new("Enable root motion extraction from animation clips", true)
                .label("Root Motion").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("ik_solver",
            SchemaEntry::new("Inverse kinematics solver algorithm", "fabrik")
                .label("IK Solver").page("Animation")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("FABRIK", "fabrik"),
                    DropdownOption::new("CCD", "ccd"),
                    DropdownOption::new("Two-Bone", "two_bone"),
                ]}))
        .setting("ik_iterations",
            SchemaEntry::new("Maximum IK solver iterations per joint chain per frame", 10_i64)
                .label("IK Iterations").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 64)))
        .setting("compression_quality",
            SchemaEntry::new("Keyframe compression quality for stored animation clips", "medium")
                .label("Animation Compression").page("Animation")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None (uncompressed)", "none"),
                    DropdownOption::new("Low", "low"),
                    DropdownOption::new("Medium", "medium"),
                    DropdownOption::new("High", "high"),
                ]})
                .validator(Validator::string_one_of(["none", "low", "medium", "high"])))
        .setting("retargeting",
            SchemaEntry::new("Enable animation retargeting between different skeleton rigs", true)
                .label("Animation Retargeting").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("update_rate_hz",
            SchemaEntry::new("Animation graph evaluation rate in Hz (0 = every frame)", 0_i64)
                .label("Animation Update Rate (Hz)").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(120.0), step: Some(5.0) })
                .validator(Validator::int_range(0, 120)))
        .setting("gpu_skinning",
            SchemaEntry::new("Perform mesh skinning on the GPU", true)
                .label("GPU Skinning").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("animation_notify_budget_us",
            SchemaEntry::new("CPU time budget per frame for animation notifies / events in microseconds", 2000_i64)
                .label("Notify Budget (\u00b5s)").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(16000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 16_000)))
        .setting("physics_animation_blend",
            SchemaEntry::new("Blend between kinematic animation and physics simulation on ragdoll", 1.0_f64)
                .label("Physics Animation Blend").page("Animation")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("procedural_animation",
            SchemaEntry::new("Enable procedural secondary motion for hair, clothing, and accessories", true)
                .label("Procedural Animation").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("foot_ik",
            SchemaEntry::new("Enable foot inverse kinematics to plant feet on uneven terrain", true)
                .label("Foot IK").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("aim_offset",
            SchemaEntry::new("Enable aim offset blend space for upper-body aiming", true)
                .label("Aim Offset").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("look_at",
            SchemaEntry::new("Enable head/eye look-at constraint driven by target position", true)
                .label("Look-At Constraint").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("transition_blend_time",
            SchemaEntry::new("Default blend time for state machine transitions in seconds", 0.2_f64)
                .label("Default Blend Time (s)").page("Animation")
                .field_type(FieldType::Slider { min: 0.0, max: 2.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 2.0)))
        .setting("additive_animation_enabled",
            SchemaEntry::new("Support additive animation layers that modify a base pose", true)
                .label("Additive Layers").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("motion_matching",
            SchemaEntry::new("Use motion matching instead of traditional state machines for locomotion", false)
                .label("Motion Matching").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("motion_matching_candidates",
            SchemaEntry::new("Number of candidate poses evaluated per motion matching query", 8_i64)
                .label("MM Candidates").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 64)))
        .setting("cloth_simulation",
            SchemaEntry::new("Enable GPU cloth simulation for capes, robes, and flags", false)
                .label("Cloth Simulation").page("Animation")
                .field_type(FieldType::Checkbox))
        .setting("cloth_substeps",
            SchemaEntry::new("Physics substeps per frame for cloth simulation accuracy", 2_i64)
                .label("Cloth Substeps").page("Animation")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(8.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 8)));

    let _ = cfg.register(NS, OWNER, schema);
}
