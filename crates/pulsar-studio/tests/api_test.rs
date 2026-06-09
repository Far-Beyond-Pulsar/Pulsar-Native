#[cfg(test)]
mod tests {
    use clap::Parser;
    use pulsar_studio::config::{Cli, Config};
    use pulsar_studio::projects::types::{ProjectRecord, ProjectStatus};
    use pulsar_studio::sessions::types::{ConnectedUser, SessionHandle, WsMessage};
    use std::path::PathBuf;

    #[test]
    fn test_cli_defaults() {
        // Verify CLI struct has correct defaults via clap
        let cli = Cli::try_parse_from(["pulsar-studio", "--port", "7700"]).unwrap();
        assert_eq!(cli.port, 7700);
        assert_eq!(cli.bind, "0.0.0.0");
        assert_eq!(cli.server_name, "Pulsar Studio Server");
        assert_eq!(cli.max_projects, 100);
        assert!(cli.auth_token.is_empty());
    }

    #[test]
    fn test_config_from_cli() {
        let cli = Cli::try_parse_from(["pulsar-studio"]).unwrap();
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.port, 7700);
        assert_eq!(config.max_projects, 100);
        assert!(config.auth_required() == false);
    }

    #[test]
    fn test_auth_token_validation() {
        // When auth token is empty, all tokens pass
        let cli = Cli::try_parse_from(["pulsar-studio"]).unwrap();
        let config = Config::from_cli(cli).unwrap();
        assert!(config.verify_token("anything"));
        assert!(!config.auth_required());

        // When auth token is set, only matching hash passes
        let cli = Cli::try_parse_from([
            "pulsar-studio",
            "--auth-token",
            "mysecret",
        ]).unwrap();
        let config = Config::from_cli(cli).unwrap();
        assert!(config.auth_required());
        assert!(config.verify_token("mysecret"));
        assert!(!config.verify_token("wrong"));
    }

    #[test]
    fn test_project_status_display() {
        assert_eq!(ProjectStatus::Idle.as_str(), "idle");
        assert_eq!(ProjectStatus::Preparing.as_str(), "preparing");
        assert_eq!(ProjectStatus::Running.as_str(), "running");
        assert_eq!(
            ProjectStatus::Error("test".into()).as_str(),
            "error"
        );
    }

    #[test]
    fn test_project_record_defaults() {
        let record = ProjectRecord {
            id: "test-id".into(),
            name: "Test Project".into(),
            description: "A test".into(),
            owner: "tester".into(),
            created_at: chrono::Utc::now(),
            last_modified: chrono::Utc::now(),
            size_bytes: 1024,
            status: ProjectStatus::Idle,
            error_msg: String::new(),
        };
        assert_eq!(record.id, "test-id");
        assert_eq!(record.status, ProjectStatus::Idle);
    }

    #[test]
    fn test_ws_message_serde() {
        let msg = WsMessage::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);

        let msg = WsMessage::UserJoined { user: "alice".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"user_joined","user":"alice"}"#);

        let deserialized: WsMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            WsMessage::UserJoined { user } => assert_eq!(user, "alice"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_session_handle_creation() {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        let handle = SessionHandle {
            project_id: "proj-1".into(),
            tx,
        };
        assert_eq!(handle.project_id, "proj-1");
    }

    #[test]
    fn test_connected_user() {
        let user = ConnectedUser {
            username: "bob".into(),
            project_id: "proj-1".into(),
        };
        assert_eq!(user.username, "bob");
        assert_eq!(user.project_id, "proj-1");
    }
}
