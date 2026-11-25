use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "simulation";

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
    );
    let _ = cfg.register(NS, OWNER, schema);
}
