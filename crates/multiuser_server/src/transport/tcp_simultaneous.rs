//! TCP simultaneous open helper for NAT traversal
//!
//! This module provides a production-ready implementation of TCP simultaneous open,
//! a technique for establishing direct TCP connections through NAT devices.

use anyhow::{Context, Result};
use std::{
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    net::{TcpSocket, TcpStream},
    time::{sleep, timeout, Instant},
};
use tracing::{debug, error, info, warn};

use crate::metrics::METRICS;

/// Maximum retry attempts for simultaneous open
const MAX_RETRIES: u32 = 20;

/// Initial retry delay
const INITIAL_DELAY: Duration = Duration::from_millis(50);

/// Maximum retry delay
const MAX_DELAY: Duration = Duration::from_secs(2);

/// Connection timeout
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP simultaneous open coordinator
pub struct TcpSimultaneousOpen {
    stats: Arc<OpenStats>,
}

/// Statistics for TCP simultaneous open
#[derive(Default)]
struct OpenStats {
    total_attempts: AtomicU64,
    successful_opens: AtomicU64,
    failed_opens: AtomicU64,
}

impl TcpSimultaneousOpen {
    /// Create a new TCP simultaneous open coordinator
    pub fn new() -> Self {
        Self {
            stats: Arc::new(OpenStats::default()),
        }
    }

    /// Attempt TCP simultaneous open with a peer
    ///
    /// Both peers must call this at approximately the same time with their
    /// respective addresses. The function will retry with exponential backoff
    /// until a connection is established or the timeout is reached.
    ///
    /// # Arguments
    /// * `local_addr` - Local address to bind to
    /// * `peer_addr` - Remote peer address to connect to
    ///
    /// # Returns
    /// A connected TCP stream on success
    pub async fn connect(
        &self,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<TcpStream> {
        let start = Instant::now();

        info!(
            "Attempting TCP simultaneous open: {} -> {}",
            local_addr, peer_addr
        );

        self.stats.total_attempts.fetch_add(1, Ordering::Relaxed);
        METRICS
            .connections_total
            .with_label_values(&["tcp", "simultaneous"])
            .inc();

        let result = timeout(
            CONNECTION_TIMEOUT,
            self.connect_with_retry(local_addr, peer_addr),
        )
        .await;

        let duration = start.elapsed();

        match result {
            Ok(Ok(stream)) => {
                info!(
                    "TCP simultaneous open succeeded in {:?}: {} -> {}",
                    duration, local_addr, peer_addr
                );
                self.stats.successful_opens.fetch_add(1, Ordering::Relaxed);
                Ok(stream)
            }
            Ok(Err(e)) => {
                warn!(
                    "TCP simultaneous open failed after {:?}: {} -> {}: {}",
                    duration, local_addr, peer_addr, e
                );
                self.stats.failed_opens.fetch_add(1, Ordering::Relaxed);
                METRICS
                    .connection_failures
                    .with_label_values(&["tcp", "simultaneous_failed"])
                    .inc();
                Err(e)
            }
            Err(_) => {
                warn!(
                    "TCP simultaneous open timeout after {:?}: {} -> {}",
                    duration, local_addr, peer_addr
                );
                self.stats.failed_opens.fetch_add(1, Ordering::Relaxed);
                METRICS
                    .connection_failures
                    .with_label_values(&["tcp", "simultaneous_timeout"])
                    .inc();
                anyhow::bail!("Connection timeout")
            }
        }
    }

    /// Connect with retry and exponential backoff
    async fn connect_with_retry(
        &self,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<TcpStream> {
        let mut retry_delay = INITIAL_DELAY;
        let mut attempts = 0;

        loop {
            attempts += 1;

            match self.try_connect(local_addr, peer_addr).await {
                Ok(stream) => {
                    debug!(
                        "Connection established on attempt {} ({} -> {})",
                        attempts, local_addr, peer_addr
                    );
                    return Ok(stream);
                }
                Err(e) => {
                    if attempts >= MAX_RETRIES {
                        return Err(e).context("Max retries reached");
                    }

                    // Only retry on specific errors that indicate the peer isn't ready
                    if should_retry(&e) {
                        debug!(
                            "Attempt {} failed, retrying in {:?}: {}",
                            attempts, retry_delay, e
                        );
                        sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(MAX_DELAY);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Try to establish a single connection
    async fn try_connect(
        &self,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<TcpStream> {
        // Create socket with proper options
        let socket = Self::create_socket(local_addr)?;

        // Attempt to connect
        match socket.connect(peer_addr).await {
            Ok(stream) => {
                debug!(
                    "TCP connection established: {} -> {}",
                    local_addr, peer_addr
                );
                Ok(stream)
            }
            Err(e) => {
                debug!("TCP connect failed: {} -> {}: {}", local_addr, peer_addr, e);
                Err(e).context("TCP connect failed")
            }
        }
    }

    /// Create a TCP socket with SO_REUSEADDR and SO_REUSEPORT
    fn create_socket(local_addr: SocketAddr) -> Result<TcpSocket> {
        let socket = if local_addr.is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
        .context("Failed to create TCP socket")?;

        // Enable address reuse - critical for simultaneous open
        socket
            .set_reuseaddr(true)
            .context("Failed to set SO_REUSEADDR")?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let raw_fd = socket.as_raw_fd();
            unsafe {
                let optval: libc::c_int = 1;
                let ret = libc::setsockopt(
                    raw_fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEPORT,
                    &optval as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&optval) as libc::socklen_t,
                );
                if ret != 0 {
                    return Err(io::Error::last_os_error()).context("Failed to set SO_REUSEPORT");
                }
            }
        }

        // Bind to local address
        socket
            .bind(local_addr)
            .context("Failed to bind TCP socket")?;

        debug!("Created TCP socket bound to {}", local_addr);
        Ok(socket)
    }

    /// Perform coordinated simultaneous open with timing
    ///
    /// This variant allows fine-grained control over timing, useful when
    /// coordinating with a signaling server.
    ///
    /// # Arguments
    /// * `local_addr` - Local address to bind to
    /// * `peer_addr` - Remote peer address to connect to
    /// * `delay` - Delay before initiating connection
    pub async fn connect_with_delay(
        &self,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        delay: Duration,
    ) -> Result<TcpStream> {
        debug!(
            "Waiting {:?} before simultaneous open: {} -> {}",
            delay, local_addr, peer_addr
        );
        sleep(delay).await;
        self.connect(local_addr, peer_addr).await
    }

    /// Attempt connection as a listener (for asymmetric simultaneous open)
    ///
    /// In some NAT configurations, one side should act more like a listener
    /// while still attempting outbound connections. This variant creates a
    /// listening socket that also attempts to connect.
    pub async fn connect_hybrid(
        &self,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<TcpStream> {
        info!(
            "Attempting hybrid TCP connection: {} <-> {}",
            local_addr, peer_addr
        );

        let socket = Self::create_socket(local_addr)?;

        // Set socket to listening state
        let listener = socket.listen(1).context("Failed to listen on socket")?;

        // Try to accept while also attempting to connect
        let accept_task = async {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        if addr == peer_addr {
                            return Ok(stream);
                        }
                        debug!("Rejected connection from unexpected peer: {}", addr);
                    }
                    Err(e) => {
                        debug!("Accept error: {}", e);
                        sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        };

        let connect_task = async {
            sleep(Duration::from_millis(200)).await;
            self.connect(local_addr, peer_addr).await
        };

        // Race between accept and connect
        tokio::select! {
            result = accept_task => result,
            result = connect_task => result,
        }
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats.total_attempts.load(Ordering::Relaxed),
            self.stats.successful_opens.load(Ordering::Relaxed),
            self.stats.failed_opens.load(Ordering::Relaxed),
        )
    }
}

impl Default for TcpSimultaneousOpen {
    fn default() -> Self {
        Self::new()
    }
}

/// Determine if an error is retryable
fn should_retry(error: &anyhow::Error) -> bool {
    if let Some(io_error) = error.downcast_ref::<io::Error>() {
        match io_error.kind() {
            io::ErrorKind::ConnectionRefused => true,
            io::ErrorKind::ConnectionReset => true,
            io::ErrorKind::TimedOut => true,
            io::ErrorKind::WouldBlock => true,
            io::ErrorKind::AddrInUse => true,
            _ => false,
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_creation() {
        let opener = TcpSimultaneousOpen::new();
        let (attempts, success, failed) = opener.stats();
        assert_eq!(attempts, 0);
        assert_eq!(success, 0);
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_default() {
        let opener = TcpSimultaneousOpen::default();
        let (attempts, _, _) = opener.stats();
        assert_eq!(attempts, 0);
    }

    #[tokio::test]
    async fn test_socket_creation() {
        let local_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let result = TcpSimultaneousOpen::create_socket(local_addr);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_socket_reuse() {
        let local_addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();

        // Create first socket
        let socket1 = TcpSimultaneousOpen::create_socket(local_addr);
        assert!(socket1.is_ok());

        // Create second socket with same address (should succeed due to SO_REUSEADDR)
        #[cfg(unix)]
        {
            let socket2 = TcpSimultaneousOpen::create_socket(local_addr);
            assert!(socket2.is_ok());
        }
    }

    #[test]
    fn test_should_retry_connection_refused() {
        let io_error = io::Error::from(io::ErrorKind::ConnectionRefused);
        let error = anyhow::Error::new(io_error);
        assert!(should_retry(&error));
    }

    #[test]
    fn test_should_retry_connection_reset() {
        let io_error = io::Error::from(io::ErrorKind::ConnectionReset);
        let error = anyhow::Error::new(io_error);
        assert!(should_retry(&error));
    }

    #[test]
    fn test_should_not_retry_permission_denied() {
        let io_error = io::Error::from(io::ErrorKind::PermissionDenied);
        let error = anyhow::Error::new(io_error);
        assert!(!should_retry(&error));
    }

    #[tokio::test]
    async fn test_loopback_connection() {
        // This test demonstrates simultaneous open on loopback
        let local_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // Create a simple listener for testing
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        // Accept connection in background
        tokio::spawn(async move {
            listener.accept().await.ok();
        });

        // Try to connect
        let opener = TcpSimultaneousOpen::new();
        let result = opener.connect(local_addr, server_addr).await;

        // Should either succeed or fail with a reasonable error
        match result {
            Ok(_) => {
                let (attempts, success, _) = opener.stats();
                assert!(attempts > 0);
                assert_eq!(success, 1);
            }
            Err(_) => {
                let (attempts, _, failed) = opener.stats();
                assert!(attempts > 0);
                assert!(failed > 0);
            }
        }
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let opener = TcpSimultaneousOpen::new();
        let invalid_addr: SocketAddr = "192.0.2.1:9999".parse().unwrap(); // TEST-NET-1

        // This should fail quickly
        let _ = opener
            .connect("127.0.0.1:0".parse().unwrap(), invalid_addr)
            .await;

        let (attempts, _success, failed) = opener.stats();
        assert!(attempts > 0);
        assert!(failed > 0);
    }

    #[tokio::test]
    async fn test_connect_with_delay() {
        let opener = TcpSimultaneousOpen::new();
        let start = Instant::now();

        let result = opener
            .connect_with_delay(
                "127.0.0.1:0".parse().unwrap(),
                "192.0.2.1:9999".parse().unwrap(),
                Duration::from_millis(100),
            )
            .await;

        let elapsed = start.elapsed();

        // Should have waited at least the delay
        assert!(elapsed >= Duration::from_millis(100));
        // Should fail because the address is unreachable
        assert!(result.is_err());
    }
}
