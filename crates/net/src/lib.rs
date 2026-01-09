use quinn;

pub struct PulsarIntNetConnection {
    pub endpoint: quinn::Endpoint,
    pub connection: quinn::Connection,
}

impl PulsarIntNetConnection {
    pub fn new(endpoint: quinn::Endpoint, connection: quinn::Connection) -> Self {
        Self {
            endpoint,
            connection,
        }
    }

    pub fn close(&self, error_code: u32, reason: &str) {
        self.connection.close(error_code.into(), reason.as_bytes());
    }

    pub fn remote_address(&self) -> Option<std::net::SocketAddr> {
        self.connection.remote_address().ok()
    }

    pub fn local_address(&self) -> Option<std::net::SocketAddr> {
        self.connection.local_address().ok()
    }

    pub fn is_connected(&self) -> bool {
        !self.connection.close_reason().is_some()
    }
}
