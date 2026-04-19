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
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
