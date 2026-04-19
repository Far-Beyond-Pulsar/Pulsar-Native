use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "gameplay";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Gameplay", "Core gameplay simulation parameters")
        .setting("tick_rate",
            SchemaEntry::new("Logic updates per second for the main game tick", 60_i64)
                .label("Tick Rate (Hz)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(120.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 120)))
        .setting("fixed_timestep",
            SchemaEntry::new("Fixed physics timestep in seconds", 0.016666_f64)
                .label("Fixed Timestep (s)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.001), max: Some(0.1), step: Some(0.001) })
                .validator(Validator::float_range(0.001, 0.1)))
        .setting("pause_when_unfocused",
            SchemaEntry::new("Pause game simulation when the window loses focus", true)
                .label("Pause When Unfocused").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("default_game_mode",
            SchemaEntry::new("Class name of the default game mode to spawn", "DefaultGameMode")
                .label("Default Game Mode").page("Gameplay")
                .field_type(FieldType::TextInput { placeholder: Some("DefaultGameMode".into()), multiline: false }))
        .setting("max_entities",
            SchemaEntry::new("Maximum entity count the ECS world will pre-allocate", 65536_i64)
                .label("Max Entities").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(256.0), max: Some(16_777_216.0), step: Some(256.0) })
                .validator(Validator::int_range(256, 16_777_216)))
        .setting("random_seed",
            SchemaEntry::new("RNG seed used for reproducible simulations (0 = random each launch)", 0_i64)
                .label("Random Seed").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(2_147_483_647.0), step: Some(1.0) }))
        .setting("cheat_enabled_in_release",
            SchemaEntry::new("Allow cheat commands to run in release builds", false)
                .label("Cheats in Release").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("difficulty",
            SchemaEntry::new("Starting difficulty level for new game sessions", "normal")
                .label("Default Difficulty").page("Gameplay")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Easy", "easy"),
                    DropdownOption::new("Normal", "normal"),
                    DropdownOption::new("Hard", "hard"),
                    DropdownOption::new("Custom", "custom"),
                ]}));

    let _ = cfg.register(NS, OWNER, schema);
}
