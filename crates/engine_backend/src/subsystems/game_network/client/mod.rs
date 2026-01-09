//! The game client networking subsystem.

mod connection;

/// The PulsarIntClient struct represents the game client networking subsystem.
pub struct PulsarIntClient {
    pub connection_manager: connection::PulsarIntConnection,
}

/// Implementation of the PulsarIntClient struct.
impl PulsarIntClient {
    /// Creates a new PulsarIntClient instance.
    pub fn new() -> Self {
        Self {
            connection_manager: connection::PulsarIntConnection::new(None, None)
        }
    }

    pub fn init_connection_now(&mut self, ip: [u8; 4], port: u16) {
        self.connection_manager.set_connection_info(Some(ip), Some(port));
        self.connection_manager.connect();
    }
}