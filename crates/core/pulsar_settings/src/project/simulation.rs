use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "simulation";

/// The late-latched look (`pulsar_game::late_look`).
pub const LATE_LOOK: &str = "late_look";
pub const LATE_LOOK_DEFAULT: bool = true;

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Simulation", "How often the game simulates").setting(
        "tick_rate_hz",
        SchemaEntry::new(
            "Simulation ticks per second in a windowed game: input, characters, scripts and physics advance once per tick. 0 matches the display's refresh rate (60 to 240). A game feels an input at the next tick, so a 60 Hz tick adds up to 16.7 ms (8.3 ms on average) of delay on a faster display",
            0_i64,
        )
        .label("Tick Rate (Hz)")
        .page("Simulation")
        .field_type(FieldType::NumberInput {
            min: Some(0.0),
            max: Some(1000.0),
            step: Some(1.0),
        })
        .validator(Validator::int_range(0, 1000)),
    )
    .setting(
        LATE_LOOK,
        SchemaEntry::new(
            "Turn the player's view by the mouse motion that arrived after the last tick, when the frame reads the camera, and let the next tick apply the same motion to the simulation (late-latched look). The view then shows the mouse as of the frame instead of as of the tick, a few milliseconds fresher. A pawn whose look cannot be told ahead (a smoothed or bent look mapping, a stick) shows the tick's view either way",
            LATE_LOOK_DEFAULT,
        )
        .label("Late Look")
        .page("Simulation")
        .field_type(FieldType::Checkbox),
    );
    let _ = cfg.register(NS, OWNER, schema);
}
