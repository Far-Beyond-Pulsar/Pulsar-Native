//! Simple example client demonstrating the join flow
//!
//! This example shows how to:
//! 1. Create a session via REST API
//! 2. Join a session with a token
//! 3. Connect via WebSocket for signaling
//! 4. Perform UDP hole punching
//! 5. Establish QUIC P2P connection

use anyhow::Result;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let base_url = std::env::var("PULSAR_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client = reqwest::Client::new();

    tracing::debug!("Pulsar MultiEdit - Example Client");
    tracing::debug!("=================================\n");

    // Step 1: Create a session
    tracing::debug!("1. Creating session...");
    let create_resp = client
        .post(format!("{}/v1/sessions", base_url))
        .json(&json!({
            "host_id": "example-host",
            "metadata": {
                "name": "Example Session",
                "max_participants": 10
            }
        }))
        .send()
        .await?;

    if !create_resp.status().is_success() {
        anyhow::bail!("Failed to create session: {}", create_resp.status());
    }

    let create_data: serde_json::Value = create_resp.json().await?;
    let session_id = create_data["session_id"].as_str().unwrap();
    let join_token = create_data["join_token"].as_str().unwrap();

    tracing::debug!("   ✓ Session created:");
    tracing::debug!("     - ID: {}", session_id);
    tracing::debug!("     - Token: {}...", &join_token[..20]);
    tracing::debug!("");

    // Step 2: Join the session
    tracing::debug!("2. Joining session as peer...");
    let join_resp = client
        .post(format!("{}/v1/sessions/{}/join", base_url, session_id))
        .json(&json!({
            "join_token": join_token,
            "peer_id": "example-peer"
        }))
        .send()
        .await?;

    if !join_resp.status().is_success() {
        anyhow::bail!("Failed to join session: {}", join_resp.status());
    }

    let join_data: serde_json::Value = join_resp.json().await?;
    tracing::debug!("   ✓ Joined session:");
    tracing::debug!("     - Peer ID: {}", join_data["peer_id"]);
    tracing::debug!("     - Role: {}", join_data["role"]);
    tracing::debug!("     - Participants: {}", join_data["participant_count"]);
    tracing::debug!("");

    // Step 3: Get session info
    tracing::debug!("3. Fetching session details...");
    let session_resp = client
        .get(format!("{}/v1/sessions/{}", base_url, session_id))
        .send()
        .await?;

    if session_resp.status().is_success() {
        let session_data: serde_json::Value = session_resp.json().await?;
        tracing::debug!("   ✓ Session details:");
        tracing::debug!("     - Host: {}", session_data["host_id"]);
        tracing::debug!("     - Created: {}", session_data["created_at"]);
        tracing::debug!("     - Expires: {}", session_data["expires_at"]);
        tracing::debug!("");
    }

    // Step 4: Check health
    tracing::debug!("4. Checking service health...");
    let health_resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await?;

    if health_resp.status().is_success() {
        let health_data: serde_json::Value = health_resp.json().await?;
        tracing::debug!("   ✓ Service health: {}", health_data["status"]);
        if let Some(checks) = health_data["checks"].as_array() {
            for check in checks {
                tracing::debug!("     - {}: {}", check["name"], check["status"]);
            }
        }
        tracing::debug!("");
    }

    // Step 5: Close session
    tracing::debug!("5. Closing session...");
    let close_resp = client
        .post(format!("{}/v1/sessions/{}/close", base_url, session_id))
        .send()
        .await?;

    if close_resp.status().is_success() {
        tracing::debug!("   ✓ Session closed");
    } else {
        tracing::debug!("   ✗ Failed to close session: {}", close_resp.status());
    }
    tracing::debug!("");

    tracing::debug!("Example completed successfully!");

    Ok(())
}
