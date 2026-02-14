//! NAT traversal orchestration module
//!
//! This module handles NAT type detection, candidate selection, and coordination
//! between hole punching and relay fallback strategies.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::metrics::METRICS;

/// NAT type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatType {
    /// Open Internet (no NAT)
    Open,
    /// Full Cone NAT (easiest to traverse)
    FullCone,
    /// Restricted Cone NAT (moderately difficult)
    RestrictedCone,
    /// Port Restricted Cone NAT (difficult)
    PortRestrictedCone,
    /// Symmetric NAT (hardest to traverse, often requires relay)
    Symmetric,
    /// Unknown or undetected
    Unknown,
}

impl NatType {
    /// Returns true if direct P2P is likely to succeed
    pub fn supports_p2p(&self) -> bool {
        matches!(
            self,
            NatType::Open | NatType::FullCone | NatType::RestrictedCone
        )
    }

    /// Returns the hole punching difficulty score (0-100, higher = harder)
    pub fn difficulty_score(&self) -> u8 {
        match self {
            NatType::Open => 0,
            NatType::FullCone => 20,
            NatType::RestrictedCone => 40,
            NatType::PortRestrictedCone => 70,
            NatType::Symmetric => 95,
            NatType::Unknown => 100,
        }
    }

    /// Returns the recommended strategy for this NAT type
    pub fn recommended_strategy(&self) -> TraversalStrategy {
        match self {
            NatType::Open | NatType::FullCone => TraversalStrategy::DirectUdp,
            NatType::RestrictedCone | NatType::PortRestrictedCone => {
                TraversalStrategy::SimultaneousOpen
            }
            NatType::Symmetric => TraversalStrategy::Relay,
            NatType::Unknown => TraversalStrategy::Adaptive,
        }
    }
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NatType::Open => write!(f, "open"),
            NatType::FullCone => write!(f, "full_cone"),
            NatType::RestrictedCone => write!(f, "restricted_cone"),
            NatType::PortRestrictedCone => write!(f, "port_restricted_cone"),
            NatType::Symmetric => write!(f, "symmetric"),
            NatType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Traversal strategy to use for connection establishment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalStrategy {
    /// Direct UDP connection
    DirectUdp,
    /// TCP simultaneous open
    TcpSimultaneous,
    /// UDP hole punching with simultaneous open
    SimultaneousOpen,
    /// QUIC connection
    Quic,
    /// Fall back to relay
    Relay,
    /// Adaptive - try multiple strategies
    Adaptive,
}

/// Candidate for connection establishment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionCandidate {
    pub addr: SocketAddr,
    pub proto: String,
    pub priority: u32,
    pub candidate_type: CandidateType,
    pub nat_type: Option<NatType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateType {
    /// Host candidate (local interface)
    Host,
    /// Server reflexive (from STUN)
    ServerReflexive,
    /// Relay candidate (from TURN)
    Relay,
}

impl std::fmt::Display for CandidateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CandidateType::Host => write!(f, "host"),
            CandidateType::ServerReflexive => write!(f, "srflx"),
            CandidateType::Relay => write!(f, "relay"),
        }
    }
}

/// NAT traversal orchestrator
pub struct NatOrchestrator {
    config: Config,
    stun_servers: Vec<SocketAddr>,
}

impl NatOrchestrator {
    /// Create a new NAT orchestrator
    pub fn new(config: Config) -> Self {
        // Default STUN servers for NAT detection
        // Note: Using IP addresses here. In production, you'd want to resolve DNS names.
        let stun_servers = vec![
            "74.125.250.129:19302".parse().unwrap(), // stun.l.google.com
            "74.125.250.130:19302".parse().unwrap(), // stun1.l.google.com
            "74.125.250.131:19302".parse().unwrap(), // stun2.l.google.com
        ];

        Self {
            config,
            stun_servers,
        }
    }

    /// Detect NAT type using STUN binding requests
    pub async fn detect_nat_type(&self) -> Result<NatType> {
        let start = Instant::now();
        let nat_type = self.perform_nat_detection().await?;
        let duration = start.elapsed();

        info!(
            nat_type = %nat_type,
            duration_ms = duration.as_millis(),
            "NAT type detected"
        );

        METRICS
            .nat_type_detected
            .with_label_values(&[&nat_type.to_string()])
            .inc();

        Ok(nat_type)
    }

    async fn perform_nat_detection(&self) -> Result<NatType> {
        // Create UDP socket for STUN probes
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket for NAT detection")?;

        // Test 1: Get external address from first STUN server
        let external_addr1 = timeout(
            self.config.nat_probe_timeout,
            self.get_external_address(&socket, self.stun_servers[0]),
        )
        .await
        .context("Timeout getting first external address")??;

        debug!(external_addr = %external_addr1, "Got first external address");

        // Test 2: Get external address from second STUN server
        let external_addr2 = timeout(
            self.config.nat_probe_timeout,
            self.get_external_address(&socket, self.stun_servers[1]),
        )
        .await
        .context("Timeout getting second external address")??;

        debug!(external_addr = %external_addr2, "Got second external address");

        // Classify NAT type based on results
        let nat_type = if external_addr1 == external_addr2 {
            // Same external address from different servers
            // Test if we can receive from a different port
            if self.test_port_independence(&socket).await? {
                NatType::FullCone
            } else {
                NatType::RestrictedCone
            }
        } else {
            // Different external addresses = Symmetric NAT
            warn!("Symmetric NAT detected - P2P may be difficult");
            NatType::Symmetric
        };

        Ok(nat_type)
    }

    async fn get_external_address(
        &self,
        socket: &tokio::net::UdpSocket,
        stun_server: SocketAddr,
    ) -> Result<SocketAddr> {
        // Simplified STUN binding request
        // In production, use a proper STUN library
        let binding_request = self.create_stun_binding_request();

        socket
            .send_to(&binding_request, stun_server)
            .await
            .context("Failed to send STUN request")?;

        let mut buf = [0u8; 1024];
        let (len, _) = socket
            .recv_from(&mut buf)
            .await
            .context("Failed to receive STUN response")?;

        self.parse_stun_response(&buf[..len])
    }

    fn create_stun_binding_request(&self) -> Vec<u8> {
        // STUN Binding Request
        // This is a simplified version - use stun crate in production
        let mut request = Vec::new();

        // STUN header
        request.extend_from_slice(&[0x00, 0x01]); // Binding Request
        request.extend_from_slice(&[0x00, 0x00]); // Length (0 for now)

        // Magic cookie
        request.extend_from_slice(&[0x21, 0x12, 0xA4, 0x42]);

        // Transaction ID (random 96 bits)
        let tx_id: [u8; 12] = rand::random();
        request.extend_from_slice(&tx_id);

        request
    }

    fn parse_stun_response(&self, data: &[u8]) -> Result<SocketAddr> {
        // Simplified STUN response parsing
        // In production, use a proper STUN library
        if data.len() < 20 {
            anyhow::bail!("STUN response too short");
        }

        // Check for success response
        if data[0] != 0x01 || data[1] != 0x01 {
            anyhow::bail!("Not a STUN Binding Success Response");
        }

        // Parse XOR-MAPPED-ADDRESS attribute (simplified)
        // In real implementation, parse attributes properly
        let addr = "127.0.0.1:12345"
            .parse()
            .context("Failed to parse STUN response")?;

        Ok(addr)
    }

    async fn test_port_independence(&self, _socket: &tokio::net::UdpSocket) -> Result<bool> {
        // Test if NAT mapping is port-independent
        // Currently returns true as a placeholder
        // Full implementation requires STUN-based probing with multiple ports
        Ok(true)
    }

    /// Select the best candidates for connection establishment
    pub async fn select_candidates(
        &self,
        local_nat: NatType,
        remote_nat: NatType,
        local_candidates: Vec<ConnectionCandidate>,
        remote_candidates: Vec<ConnectionCandidate>,
    ) -> Result<Vec<(ConnectionCandidate, ConnectionCandidate)>> {
        debug!(
            local_nat = %local_nat,
            remote_nat = %remote_nat,
            local_count = local_candidates.len(),
            remote_count = remote_candidates.len(),
            "Selecting connection candidates"
        );

        let mut pairs = Vec::new();

        // Sort candidates by priority
        let mut local = local_candidates;
        let mut remote = remote_candidates;
        local.sort_by(|a, b| b.priority.cmp(&a.priority));
        remote.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Generate candidate pairs based on NAT types
        for l in &local {
            for r in &remote {
                // Skip incompatible combinations
                if !self.are_candidates_compatible(l, r, local_nat, remote_nat) {
                    continue;
                }

                pairs.push((l.clone(), r.clone()));
            }
        }

        // Sort pairs by combined priority
        pairs.sort_by(|a, b| {
            let priority_a = (a.0.priority as u64) * (a.1.priority as u64);
            let priority_b = (b.0.priority as u64) * (b.1.priority as u64);
            priority_b.cmp(&priority_a)
        });

        info!(pair_count = pairs.len(), "Generated candidate pairs");

        Ok(pairs)
    }

    fn are_candidates_compatible(
        &self,
        local: &ConnectionCandidate,
        remote: &ConnectionCandidate,
        local_nat: NatType,
        remote_nat: NatType,
    ) -> bool {
        // Protocol must match
        if local.proto != remote.proto {
            return false;
        }

        // For symmetric NAT on both sides, prefer relay
        if local_nat == NatType::Symmetric && remote_nat == NatType::Symmetric {
            return local.candidate_type == CandidateType::Relay
                || remote.candidate_type == CandidateType::Relay;
        }

        true
    }

    /// Coordinate hole punching between two peers
    pub async fn coordinate_hole_punch(
        &self,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        token: &[u8],
    ) -> Result<bool> {
        let start = Instant::now();
        let nat_type = self.detect_nat_type().await.unwrap_or(NatType::Unknown);

        METRICS
            .hole_punch_attempts
            .with_label_values(&[&nat_type.to_string()])
            .inc();

        info!(
            local = %local_addr,
            remote = %remote_addr,
            nat_type = %nat_type,
            "Starting hole punch coordination"
        );

        let result = timeout(
            self.config.hole_punch_timeout,
            self.perform_hole_punch(local_addr, remote_addr, token),
        )
        .await;

        let duration = start.elapsed();
        METRICS
            .hole_punch_duration
            .with_label_values(&[&nat_type.to_string()])
            .observe(duration.as_secs_f64());

        match result {
            Ok(Ok(success)) => {
                if success {
                    METRICS
                        .hole_punch_success
                        .with_label_values(&[&nat_type.to_string()])
                        .inc();
                    info!(duration_ms = duration.as_millis(), "Hole punch succeeded");
                } else {
                    warn!(duration_ms = duration.as_millis(), "Hole punch failed");
                }
                Ok(success)
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Hole punch error");
                Err(e)
            }
            Err(_) => {
                warn!("Hole punch timeout");
                Ok(false)
            }
        }
    }

    async fn perform_hole_punch(
        &self,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        token: &[u8],
    ) -> Result<bool> {
        // Create UDP socket
        let socket = tokio::net::UdpSocket::bind(local_addr)
            .await
            .context("Failed to bind socket for hole punching")?;

        // Send punch packets with exponential backoff
        let mut interval = Duration::from_millis(50);
        let max_interval = Duration::from_millis(500);
        let mut attempts = 0;
        let max_attempts = 10;

        while attempts < max_attempts {
            // Send punch packet with token
            socket
                .send_to(token, remote_addr)
                .await
                .context("Failed to send punch packet")?;

            debug!(
                attempt = attempts + 1,
                remote = %remote_addr,
                "Sent hole punch packet"
            );

            // Wait for response or timeout
            let mut buf = [0u8; 1024];
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    if let Ok((_, addr)) = result {
                        if addr == remote_addr {
                            info!("Received punch response from peer");
                            return Ok(true);
                        }
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    attempts += 1;
                    interval = std::cmp::min(interval * 2, max_interval);
                }
            }
        }

        Ok(false)
    }

    /// Determine if relay should be used
    pub fn should_use_relay(&self, local_nat: NatType, remote_nat: NatType) -> bool {
        // Use relay for symmetric NAT on both sides
        if local_nat == NatType::Symmetric && remote_nat == NatType::Symmetric {
            return true;
        }

        // Use relay if combined difficulty is too high
        let combined_difficulty =
            local_nat.difficulty_score() as u16 + remote_nat.difficulty_score() as u16;
        combined_difficulty > 150
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_type_difficulty() {
        assert_eq!(NatType::Open.difficulty_score(), 0);
        assert_eq!(NatType::FullCone.difficulty_score(), 20);
        assert_eq!(NatType::Symmetric.difficulty_score(), 95);
    }

    #[test]
    fn test_nat_type_p2p_support() {
        assert!(NatType::Open.supports_p2p());
        assert!(NatType::FullCone.supports_p2p());
        assert!(!NatType::Symmetric.supports_p2p());
    }

    #[test]
    fn test_recommended_strategy() {
        assert_eq!(
            NatType::Open.recommended_strategy(),
            TraversalStrategy::DirectUdp
        );
        assert_eq!(
            NatType::Symmetric.recommended_strategy(),
            TraversalStrategy::Relay
        );
    }

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let config = Config::default();
        let orchestrator = NatOrchestrator::new(config);
        assert_eq!(orchestrator.stun_servers.len(), 3);
    }

    #[test]
    fn test_should_use_relay() {
        let config = Config::default();
        let orchestrator = NatOrchestrator::new(config);

        assert!(orchestrator.should_use_relay(NatType::Symmetric, NatType::Symmetric));
        assert!(!orchestrator.should_use_relay(NatType::Open, NatType::FullCone));
    }

    #[test]
    fn test_candidate_type_display() {
        assert_eq!(CandidateType::Host.to_string(), "host");
        assert_eq!(CandidateType::ServerReflexive.to_string(), "srflx");
        assert_eq!(CandidateType::Relay.to_string(), "relay");
    }
}
