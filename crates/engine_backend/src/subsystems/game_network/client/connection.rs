//! The game client networking subsystem.
//! 
//! This module handles all client-side networking functionalities,
//! including establishing connections to game servers, managing
//! connection states, and handling data transmission.


/// The PulsarIntConnection struct represents a connection to a game server.
/// It holds information about the server's IP address, port, connection
/// status, last ping time, latency, and a reference to the underlying
/// connection object.
pub struct PulsarIntConnection {
    pub ip: Option<[u8; 4]>,    
    pub port: Option<u16>,
    pub last_ping: std::time::Instant,
    pub last_latency_ms: u32,
    pub connection_state: PulsarIntConnectionState,
    pub conn_ref: Option<()>, // TODO: Replace with actual connection reference type
}

impl PulsarIntConnection {
    /// Creates a new PulsarIntConnection instance with the specified
    /// IP and port.
    /// 
    /// Note: The connection is not established upon creation. Call the
    /// `connect` method to establish the connection.
    pub fn new(ip: Option<[u8; 4]>, port: Option<u16>) -> Self {
        Self {
            ip,
            port,
            connection_state: PulsarIntConnectionState::Disconnected,
            last_ping: std::time::Instant::now(),
            last_latency_ms: 0,
            conn_ref: None,
        }
    }

    pub fn set_connection_info(&mut self, ip: Option<[u8; 4]>, port: Option<u16>) {
        self.ip = ip;
        self.port = port;
    }

    pub fn connect(&mut self) {
        if let (Some(_ip), Some(_port)) = (self.ip, self.port) {
            self.connection_state = PulsarIntConnectionState::Connecting;
            // Connection logic will be implemented when actual transport layer is added
            tracing::debug!("Connection initiated for {:?}:{:?}", self.ip, self.port);
        } else {
            tracing::error!("Cannot connect: IP or port not set");
        }
    }

    pub fn disconnect(&mut self) {
        match self.connection_state {
            PulsarIntConnectionState::Connected | PulsarIntConnectionState::Connecting => {
                self.connection_state = PulsarIntConnectionState::Disconnecting;
                // Disconnection logic will be implemented when actual transport layer is added
                tracing::debug!("Disconnection initiated");
                self.connection_state = PulsarIntConnectionState::Disconnected;
            }
            _ => {
                tracing::warn!("Disconnect called but not in connected state");
            }
        }
    }
}

/// The PulsarIntConnectionState enum represents the various states
/// of a game network connection.
pub enum PulsarIntConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

impl Default for PulsarIntConnectionState {
    fn default() -> Self {
        PulsarIntConnectionState::Disconnected
    }
}