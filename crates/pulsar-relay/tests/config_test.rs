#[cfg(test)]
mod tests {
    use pulsar_relay::config::{Config, Cli};

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.max_sessions, 10000);
        assert_eq!(config.relay_bandwidth_limit, 10 * 1024 * 1024);
        assert_eq!(config.prometheus_port, 9090);
        assert!(config.jwt_secret == "change-this-secret-in-production");
    }

    #[test]
    fn test_config_default_addresses() {
        let config = Config::default();
        assert_eq!(config.http_bind.to_string(), "0.0.0.0:8080");
        assert_eq!(config.quic_bind.to_string(), "0.0.0.0:8443");
        assert_eq!(config.udp_bind.to_string(), "0.0.0.0:7000");
    }

    #[test]
    fn test_config_session_ttl_default() {
        let config = Config::default();
        assert_eq!(config.session_ttl.as_secs(), 3600);
    }

    #[test]
    fn test_config_nat_timeouts() {
        let config = Config::default();
        assert_eq!(config.nat_probe_timeout.as_secs(), 5);
        assert_eq!(config.hole_punch_timeout.as_secs(), 10);
    }

    #[test]
    fn test_config_mtls_default() {
        let config = Config::default();
        assert!(!config.mtls_enabled);
        assert!(config.client_ca_path.is_none());
    }

    #[test]
    fn test_cli_parsing() {
        let args = vec![
            "pulsar-relay",
            "--http-bind", "127.0.0.1:9090",
            "--log-level", "debug",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.http_bind.unwrap().to_string(), "127.0.0.1:9090");
        assert_eq!(cli.log_level, "debug");
    }
}
