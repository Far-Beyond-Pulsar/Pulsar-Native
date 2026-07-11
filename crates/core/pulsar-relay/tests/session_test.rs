#[cfg(test)]
mod tests {
    use pulsar_multiplayer_core::session::Role;
    use pulsar_relay::auth::AuthService;
    use pulsar_relay::config::Config;
    use pulsar_relay::session::SessionStore;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_config_defaults() {
        let config = Config::default();
        assert_eq!(config.max_sessions, 10000);
        assert_eq!(config.log_level, "info");
    }

    #[tokio::test]
    async fn test_session_creation() {
        let config = Arc::new(Config::default());
        let store = SessionStore::new(config);
        let session = store
            .create_session("host-1".into(), serde_json::json!({"name": "test"}))
            .unwrap();
        assert_eq!(session.host_id, "host-1");
        assert!(session.participants.is_empty());
    }

    #[tokio::test]
    async fn test_session_join() {
        let config = Arc::new(Config::default());
        let store = SessionStore::new(config);
        let session = store
            .create_session("host-1".into(), serde_json::json!({}))
            .unwrap();
        let sid = session.id.clone();

        let joined = store
            .join_session(&sid, "peer-1".into(), Role::Editor)
            .unwrap();
        assert_eq!(joined.participants.len(), 1);
        assert_eq!(joined.participants[0].peer_id, "peer-1");

        let session = store.get_session(&sid).unwrap();
        assert_eq!(session.participants.len(), 1);
    }

    #[tokio::test]
    async fn test_session_close() {
        let config = Arc::new(Config::default());
        let store = SessionStore::new(config);
        let session = store
            .create_session("host-1".into(), serde_json::json!({}))
            .unwrap();
        let sid = session.id.clone();

        store.close_session(&sid, "user_requested").unwrap();
        assert!(store.get_session(&sid).is_none());
    }

    #[tokio::test]
    async fn test_auth_token_generation_and_verification() {
        let config = Config::default();
        let auth = AuthService::new(&config).unwrap();

        let token = auth
            .create_join_token("session-1".into(), Role::Editor, Duration::from_secs(3600))
            .unwrap();

        let (session_id, role) = auth.verify_join_token(&token).unwrap();
        assert_eq!(session_id, "session-1");
        assert_eq!(role, Role::Editor);
    }

    #[tokio::test]
    async fn test_health_checker() {
        let config = Arc::new(Config::default());
        let health = pulsar_relay::health::HealthChecker::new(config);
        let status = health.check_health().await;
        assert_eq!(status.status, "healthy");
    }
}
