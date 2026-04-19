use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "network";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Network", "Multiplayer networking configuration")
        .setting("enable_multiplayer",
            SchemaEntry::new("Enable multiplayer networking in this project", false)
                .label("Enable Multiplayer").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("transport",
            SchemaEntry::new("Network transport protocol", "udp")
                .label("Transport").page("Network")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("UDP (unreliable)", "udp"),
                    DropdownOption::new("WebTransport / QUIC", "quic"),
                    DropdownOption::new("TCP (reliable)", "tcp"),
                    DropdownOption::new("Steam Networking", "steam"),
                ]}))
        .setting("server_port",
            SchemaEntry::new("Port the dedicated server listens on", 7777_i64)
                .label("Server Port").page("Network")
                .field_type(FieldType::NumberInput { min: Some(1024.0), max: Some(65535.0), step: Some(1.0) })
                .validator(Validator::int_range(1024, 65535)))
        .setting("max_players",
            SchemaEntry::new("Maximum number of connected players", 64_i64)
                .label("Max Players").page("Network")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(4096.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 4096)))
        .setting("replication_rate",
            SchemaEntry::new("How many times per second actor state is replicated to clients", 30_i64)
                .label("Replication Rate (Hz)").page("Network")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(120.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 120)))
        .setting("client_prediction",
            SchemaEntry::new("Enable client-side prediction and server reconciliation", true)
                .label("Client Prediction").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("lag_compensation",
            SchemaEntry::new("Enable server-side lag compensation for hit detection", true)
                .label("Lag Compensation").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("lag_compensation_ms",
            SchemaEntry::new("Maximum lag compensation window in milliseconds", 200_i64)
                .label("Lag Compensation Window (ms)").page("Network")
                .field_type(FieldType::NumberInput { min: Some(50.0), max: Some(1000.0), step: Some(10.0) })
                .validator(Validator::int_range(50, 1000)))
        .setting("nat_punchthrough",
            SchemaEntry::new("Attempt NAT punchthrough for peer-to-peer connections", true)
                .label("NAT Punchthrough").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("bandwidth_limit_kbps",
            SchemaEntry::new("Per-connection outgoing bandwidth limit in Kbit/s (0 = unlimited)", 0_i64)
                .label("Bandwidth Limit (Kbps)").page("Network")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(100_000.0), step: Some(100.0) })
                .validator(Validator::int_range(0, 100_000)))
        .setting("compression",
            SchemaEntry::new("Compress network packets before sending", true)
                .label("Packet Compression").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("encryption",
            SchemaEntry::new("Encrypt all network traffic with DTLS", false)
                .label("Encryption (DTLS)").page("Network")
                .field_type(FieldType::Checkbox))
        .setting("rpc_timeout_ms",
            SchemaEntry::new("Timeout for reliable RPC calls in milliseconds", 5000_i64)
                .label("RPC Timeout (ms)").page("Network")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(30000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 30_000)));

    let _ = cfg.register(NS, OWNER, schema);
}
