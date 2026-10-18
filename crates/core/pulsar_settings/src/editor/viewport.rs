use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "viewport";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Viewport", "Level editor camera and transform snapping")
        .setting(
            "camera_move_speed",
            SchemaEntry::new("Fly camera movement speed", 10.0_f64)
                .label("Camera Speed")
                .page("Viewport")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: None,
                    step: Some(1.0),
                })
                .validator(Validator::custom(|value| {
                    let speed = value.as_float().map_err(|error| error.to_string())?;
                    if speed.is_finite() && (1.0..=f32::MAX as f64).contains(&speed) {
                        Ok(())
                    } else {
                        Err(
                            "camera speed must be finite and between 1 and the maximum supported value"
                                .to_owned(),
                        )
                    }
                })),
        )
        .setting(
            "location_snap",
            SchemaEntry::new("Translation snap increment", 1.0_f64)
                .label("Location Snap")
                .page("Viewport")
                .field_type(FieldType::NumberInput {
                    min: Some(0.01),
                    max: Some(1000.0),
                    step: Some(0.1),
                })
                .validator(Validator::float_range(0.01, 1000.0)),
        )
        .setting(
            "rotation_snap",
            SchemaEntry::new("Rotation snap increment in degrees", 15.0_f64)
                .label("Rotation Snap")
                .page("Viewport")
                .field_type(FieldType::NumberInput {
                    min: Some(0.1),
                    max: Some(360.0),
                    step: Some(1.0),
                })
                .validator(Validator::float_range(0.1, 360.0)),
        )
        .setting(
            "scale_snap",
            SchemaEntry::new("Scale snap increment", 0.1_f64)
                .label("Scale Snap")
                .page("Viewport")
                .field_type(FieldType::NumberInput {
                    min: Some(0.01),
                    max: Some(10.0),
                    step: Some(0.01),
                })
                .validator(Validator::float_range(0.01, 10.0)),
        );
    let _ = cfg.register(NS, OWNER, schema);
}
