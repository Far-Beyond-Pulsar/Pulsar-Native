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
                ]}))
        .setting("default_player_class",
            SchemaEntry::new("Class name of the default player pawn", "DefaultPawn")
                .label("Default Player Class").page("Gameplay")
                .field_type(FieldType::TextInput { placeholder: Some("DefaultPawn".into()), multiline: false }))
        .setting("default_controller_class",
            SchemaEntry::new("Class name of the default player controller", "PlayerController")
                .label("Default Controller Class").page("Gameplay")
                .field_type(FieldType::TextInput { placeholder: Some("PlayerController".into()), multiline: false }))
        .setting("default_hud_class",
            SchemaEntry::new("Class name of the HUD to spawn for each player", "HUD")
                .label("Default HUD Class").page("Gameplay")
                .field_type(FieldType::TextInput { placeholder: Some("HUD".into()), multiline: false }))
        .setting("respawn_delay_seconds",
            SchemaEntry::new("Seconds between a player dying and being allowed to respawn", 3.0_f64)
                .label("Respawn Delay (s)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(300.0), step: Some(0.5) })
                .validator(Validator::float_range(0.0, 300.0)))
        .setting("respawn_at_checkpoint",
            SchemaEntry::new("Respawn players at the last activated checkpoint instead of world start", true)
                .label("Respawn at Checkpoint").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("max_spectators",
            SchemaEntry::new("Maximum number of spectators allowed in a session", 8_i64)
                .label("Max Spectators").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(128.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 128)))
        .setting("allow_spectating",
            SchemaEntry::new("Allow players to spectate after dying instead of waiting at a screen", true)
                .label("Allow Spectating").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("warmup_time_seconds",
            SchemaEntry::new("Pre-match warmup period before the game begins in seconds (0 = skip)", 0.0_f64)
                .label("Warmup Time (s)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(600.0), step: Some(5.0) })
                .validator(Validator::float_range(0.0, 600.0)))
        .setting("match_time_limit_seconds",
            SchemaEntry::new("Time limit for a match in seconds (0 = unlimited)", 0.0_f64)
                .label("Match Time Limit (s)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(7200.0), step: Some(30.0) })
                .validator(Validator::float_range(0.0, 7200.0)))
        .setting("friendly_fire",
            SchemaEntry::new("Allow players to damage teammates", false)
                .label("Friendly Fire").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("damage_multiplier",
            SchemaEntry::new("Global multiplier applied to all damage values", 1.0_f64)
                .label("Damage Multiplier").page("Gameplay")
                .field_type(FieldType::Slider { min: 0.0, max: 5.0, step: 0.05 })
                .validator(Validator::float_range(0.0, 5.0)))
        .setting("experience_multiplier",
            SchemaEntry::new("Global multiplier applied to all XP/score gained", 1.0_f64)
                .label("XP Multiplier").page("Gameplay")
                .field_type(FieldType::Slider { min: 0.0, max: 10.0, step: 0.1 })
                .validator(Validator::float_range(0.0, 10.0)))
        .setting("save_game_slots",
            SchemaEntry::new("Number of save game slots available to the player", 10_i64)
                .label("Save Game Slots").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(100.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 100)))
        .setting("auto_save_enabled",
            SchemaEntry::new("Automatically save the game at checkpoints and intervals", true)
                .label("Auto Save").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("auto_save_interval_seconds",
            SchemaEntry::new("How often the game auto-saves in seconds (0 = checkpoint-only)", 300_i64)
                .label("Auto Save Interval (s)").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(3600.0), step: Some(30.0) })
                .validator(Validator::int_range(0, 3600)))
        .setting("death_penalty",
            SchemaEntry::new("Consequences for the player upon death", "none")
                .label("Death Penalty").page("Gameplay")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Lose Gold", "gold"),
                    DropdownOption::new("Lose Items", "items"),
                    DropdownOption::new("Permadeath", "permadeath"),
                ]}))
        .setting("physics_interaction_enabled",
            SchemaEntry::new("Allow player characters to physically interact with the environment", true)
                .label("Physics Interaction").page("Gameplay")
                .field_type(FieldType::Checkbox))
        .setting("objective_markers_enabled",
            SchemaEntry::new("Show in-world objective markers and waypoints", true)
                .label("Objective Markers").page("Gameplay")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
