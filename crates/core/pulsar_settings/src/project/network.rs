use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "network";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Network", "Multiplayer networking configuration")
        .setting(
            "enable_multiplayer",
            SchemaEntry::new("Enable multiplayer networking in this project", false)
                .label("Enable Multiplayer")
                .page("Network")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "transport",
            SchemaEntry::new("Network transport protocol", "udp")
                .label("Transport")
                .page("Network")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("UDP (unreliable)", "udp"),
                        DropdownOption::new("WebTransport / QUIC", "quic"),
                        DropdownOption::new("TCP (reliable)", "tcp"),
                        DropdownOption::new("Steam Networking", "steam"),
                    ],
                }),
        )
        .setting(
            "server_port",
            SchemaEntry::new("Port the dedicated server listens on", 7777_i64)
                .label("Server Port")
                .page("Network")
                .field_type(FieldType::NumberInput {
                    min: Some(1024.0),
                    max: Some(65535.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1024, 65535)),
        )
        .setting(
            "max_players",
            SchemaEntry::new("Maximum number of connected players", 64_i64)
                .label("Max Players")
                .page("Network")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(4096.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 4096)),
        )
        .setting(
            "replication_rate",
            SchemaEntry::new(
                "How many times per second actor state is replicated to clients",
                30_i64,
            )
            .label("Replication Rate (Hz)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(1.0),
                max: Some(120.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(1, 120)),
        )
        .setting(
            "client_prediction",
            SchemaEntry::new(
                "Enable client-side prediction and server reconciliation",
                true,
            )
            .label("Client Prediction")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "lag_compensation",
            SchemaEntry::new(
                "Enable server-side lag compensation for hit detection",
                true,
            )
            .label("Lag Compensation")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "lag_compensation_ms",
            SchemaEntry::new("Maximum lag compensation window in milliseconds", 200_i64)
                .label("Lag Compensation Window (ms)")
                .page("Network")
                .field_type(FieldType::NumberInput {
                    min: Some(50.0),
                    max: Some(1000.0),
                    step: Some(10.0),
                })
                .validator(Validator::int_range(50, 1000)),
        )
        .setting(
            "nat_punchthrough",
            SchemaEntry::new(
                "Attempt NAT punchthrough for peer-to-peer connections",
                true,
            )
            .label("NAT Punchthrough")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "bandwidth_limit_kbps",
            SchemaEntry::new(
                "Per-connection outgoing bandwidth limit in Kbit/s (0 = unlimited)",
                0_i64,
            )
            .label("Bandwidth Limit (Kbps)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(100_000.0),
                step: Some(100.0),
            })
            .validator(Validator::int_range(0, 100_000)),
        )
        .setting(
            "compression",
            SchemaEntry::new("Compress network packets before sending", true)
                .label("Packet Compression")
                .page("Network")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "encryption",
            SchemaEntry::new("Encrypt all network traffic with DTLS", false)
                .label("Encryption (DTLS)")
                .page("Network")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "rpc_timeout_ms",
            SchemaEntry::new("Timeout for reliable RPC calls in milliseconds", 5000_i64)
                .label("RPC Timeout (ms)")
                .page("Network")
                .field_type(FieldType::NumberInput {
                    min: Some(100.0),
                    max: Some(30000.0),
                    step: Some(100.0),
                })
                .validator(Validator::int_range(100, 30_000)),
        )
        .setting(
            "bandwidth_limit_kbps",
            SchemaEntry::new(
                "Outbound bandwidth cap per client connection in Kbps (0 = unlimited)",
                0_i64,
            )
            .label("Bandwidth Limit (Kbps)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(100_000.0),
                step: Some(100.0),
            })
            .validator(Validator::int_range(0, 100_000)),
        )
        .setting(
            "compression_enabled",
            SchemaEntry::new(
                "Compress network packets to reduce bandwidth at the cost of CPU",
                true,
            )
            .label("Packet Compression")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "encryption_enabled",
            SchemaEntry::new("Encrypt network traffic using DTLS/TLS", false)
                .label("Encryption")
                .page("Network")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "nat_traversal",
            SchemaEntry::new(
                "Enable NAT punch-through for peer-to-peer connections",
                true,
            )
            .label("NAT Traversal")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "relay_server_url",
            SchemaEntry::new(
                "WebSocket relay server URL used when direct NAT traversal fails",
                "",
            )
            .label("Relay Server URL")
            .page("Network")
            .field_type(FieldType::TextInput {
                placeholder: Some("wss://relay.example.com".into()),
                multiline: false,
            }),
        )
        .setting(
            "stun_server_url",
            SchemaEntry::new(
                "STUN server URL used for NAT traversal and IP discovery",
                "stun:stun.l.google.com:19302",
            )
            .label("STUN Server URL")
            .page("Network")
            .field_type(FieldType::TextInput {
                placeholder: Some("stun:stun.l.google.com:19302".into()),
                multiline: false,
            }),
        )
        .setting(
            "prediction_enabled",
            SchemaEntry::new(
                "Enable client-side movement prediction for responsive feel",
                true,
            )
            .label("Client Prediction")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "reconciliation_enabled",
            SchemaEntry::new(
                "Reconcile predicted state with authoritative server corrections",
                true,
            )
            .label("Server Reconciliation")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "interpolation_delay_ms",
            SchemaEntry::new(
                "Entity interpolation delay in milliseconds to smooth remote player movement",
                100_i64,
            )
            .label("Interpolation Delay (ms)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(500.0),
                step: Some(10.0),
            })
            .validator(Validator::int_range(0, 500)),
        )
        .setting(
            "max_queued_snapshots",
            SchemaEntry::new(
                "Maximum number of world state snapshots buffered for reconciliation",
                60_i64,
            )
            .label("Max Queued Snapshots")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(10.0),
                max: Some(256.0),
                step: Some(10.0),
            })
            .validator(Validator::int_range(10, 256)),
        )
        .setting(
            "debug_network_stats",
            SchemaEntry::new(
                "Show real-time network stats overlay (ping, packet loss, bandwidth)",
                false,
            )
            .label("Network Stats Overlay")
            .page("Network")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "simulate_latency_ms",
            SchemaEntry::new(
                "Artificial latency added to all network packets for testing (0 = off)",
                0_i64,
            )
            .label("Simulated Latency (ms)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(2000.0),
                step: Some(10.0),
            })
            .validator(Validator::int_range(0, 2000)),
        )
        .setting(
            "simulate_packet_loss_pct",
            SchemaEntry::new(
                "Percentage of packets artificially dropped for network testing (0 = off)",
                0_i64,
            )
            .label("Simulated Packet Loss (%)")
            .page("Network")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(50.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 50)),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
