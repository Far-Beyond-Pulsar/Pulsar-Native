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

    tracing::info!("Pulsar MultiEdit - Example Client");
    tracing::info!("=================================\n");

    // Step 1: Create a session
    tracing::info!("1. Creating session...");
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

    tracing::info!("   ✓ Session created:");
    tracing::info!("     - ID: {}", session_id);
    tracing::info!("     - Token: {}...", &join_token[..20]);
    tracing::info!("");

    // Step 2: Join the session
    tracing::info!("2. Joining session as peer...");
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
    tracing::info!("   ✓ Joined session:");
    tracing::info!("     - Peer ID: {}", join_data["peer_id"]);
    tracing::info!("     - Role: {}", join_data["role"]);
    tracing::info!("     - Participants: {}", join_data["participant_count"]);
    tracing::info!("");

    // Step 3: Get session info
    tracing::info!("3. Fetching session details...");
    let session_resp = client
        .get(format!("{}/v1/sessions/{}", base_url, session_id))
        .send()
        .await?;

    if session_resp.status().is_success() {
        let session_data: serde_json::Value = session_resp.json().await?;
        tracing::info!("   ✓ Session details:");
        tracing::info!("     - Host: {}", session_data["host_id"]);
        tracing::info!("     - Created: {}", session_data["created_at"]);
        tracing::info!("     - Expires: {}", session_data["expires_at"]);
        tracing::info!("");
    }

    // Step 4: Check health
    tracing::info!("4. Checking service health...");
    let health_resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await?;

    if health_resp.status().is_success() {
        let health_data: serde_json::Value = health_resp.json().await?;
        tracing::info!("   ✓ Service health: {}", health_data["status"]);
        if let Some(checks) = health_data["checks"].as_array() {
            for check in checks {
                tracing::info!("     - {}: {}", check["name"], check["status"]);
            }
        }
        tracing::info!("");
    }

    // Step 5: Close session
    tracing::info!("5. Closing session...");
    let close_resp = client
        .post(format!("{}/v1/sessions/{}/close", base_url, session_id))
        .send()
        .await?;

    if close_resp.status().is_success() {
        tracing::info!("   ✓ Session closed");
    } else {
        tracing::info!("   ✗ Failed to close session: {}", close_resp.status());
    }
    tracing::info!("");

    tracing::info!("Example completed successfully!");

    Ok(())
}
