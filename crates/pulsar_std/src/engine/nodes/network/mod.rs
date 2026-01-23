//! # Network Module
//!
//! Network utility nodes for the Pulsar visual programming system.
//!
//! This module provides nodes for:
//! - IP address validation and parsing
//! - Port number validation
//! - Network address formatting
//! - Protocol helpers

use crate::blueprint;

// =============================================================================
// Constants
// =============================================================================

/// Maximum valid port number (TCP/UDP)
const PORT_MAX: i64 = 65535;

/// Minimum valid port number
const PORT_MIN: i64 = 1;

/// Upper bound of well-known ports
const WELL_KNOWN_PORT_MAX: i64 = 1023;

/// Lower bound of registered ports
const REGISTERED_PORT_MIN: i64 = 1024;

/// Upper bound of registered ports
const REGISTERED_PORT_MAX: i64 = 49151;

/// Lower bound of dynamic/private ports
const DYNAMIC_PORT_MIN: i64 = 49152;

/// Maximum value for an IP octet
const OCTET_MAX: i64 = 255;

/// Minimum value for an IP octet
const OCTET_MIN: i64 = 0;

/// Number of octets in an IPv4 address
const IPV4_OCTET_COUNT: usize = 4;

/// Maximum CIDR mask bits for IPv4
const IPV4_CIDR_MAX: i64 = 32;

/// Minimum CIDR mask bits
const CIDR_MIN: i64 = 0;

// =============================================================================
// IP Address Operations
// =============================================================================

/// Validate an IPv4 address.
///
/// # Inputs
/// - `ip`: The IP address string to validate
///
/// # Returns
/// True if the IP address is valid IPv4 format
///
/// # Network Validate IPv4
/// Validates an IPv4 address string.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn validate_ipv4(ip: String) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != IPV4_OCTET_COUNT {
        return false;
    }

    parts.iter().all(|part| {
        part.parse::<u8>().is_ok()
    })
}

/// Parse IPv4 address to octets.
///
/// # Inputs
/// - `ip`: The IP address string
///
/// # Returns
/// Comma-separated octets (or error message)
///
/// # Network Parse IPv4
/// Parses an IPv4 address into octets.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn parse_ipv4(ip: String) -> String {
    if validate_ipv4(ip.clone()) {
        ip
    } else {
        "Invalid IPv4".to_string()
    }
}

/// Create IPv4 address from octets.
///
/// # Inputs
/// - `octet1`: First octet (0-255)
/// - `octet2`: Second octet (0-255)
/// - `octet3`: Third octet (0-255)
/// - `octet4`: Fourth octet (0-255)
///
/// # Returns
/// Formatted IPv4 address string
///
/// # Network Create IPv4
/// Creates an IPv4 address from four octets.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn create_ipv4(octet1: i64, octet2: i64, octet3: i64, octet4: i64) -> String {
    format!("{}.{}.{}.{}",
        octet1.clamp(OCTET_MIN, OCTET_MAX),
        octet2.clamp(OCTET_MIN, OCTET_MAX),
        octet3.clamp(OCTET_MIN, OCTET_MAX),
        octet4.clamp(OCTET_MIN, OCTET_MAX)
    )
}

/// Check if IP is localhost.
///
/// # Inputs
/// - `ip`: The IP address string
///
/// # Returns
/// True if the IP is localhost (127.0.0.1)
///
/// # Network Is Localhost
/// Checks if an IP address is localhost.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn is_localhost(ip: String) -> bool {
    ip == "127.0.0.1" || ip == "localhost" || ip == "::1"
}

/// Check if IP is in private range.
///
/// # Inputs
/// - `ip`: The IP address string
///
/// # Returns
/// True if the IP is in a private network range
///
/// # Network Is Private IP
/// Checks if an IP address is in a private network range.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn is_private_ip(ip: String) -> bool {
    if !validate_ipv4(ip.clone()) {
        return false;
    }
    
    ip.starts_with("10.") || 
    ip.starts_with("192.168.") ||
    ip.starts_with("172.16.") || ip.starts_with("172.17.") ||
    ip.starts_with("172.18.") || ip.starts_with("172.19.") ||
    ip.starts_with("172.20.") || ip.starts_with("172.21.") ||
    ip.starts_with("172.22.") || ip.starts_with("172.23.") ||
    ip.starts_with("172.24.") || ip.starts_with("172.25.") ||
    ip.starts_with("172.26.") || ip.starts_with("172.27.") ||
    ip.starts_with("172.28.") || ip.starts_with("172.29.") ||
    ip.starts_with("172.30.") || ip.starts_with("172.31.")
}

// =============================================================================
// Port Operations
// =============================================================================

/// Validate a port number.
///
/// # Inputs
/// - `port`: The port number to validate
///
/// # Returns
/// True if the port is valid (1-65535)
///
/// # Network Validate Port
/// Validates a port number.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn validate_port(port: i64) -> bool {
    port >= PORT_MIN && port <= PORT_MAX
}

/// Check if port is well-known (0-1023).
///
/// # Inputs
/// - `port`: The port number
///
/// # Returns
/// True if port is in well-known range
///
/// # Network Is Well Known Port
/// Checks if a port is in the well-known range (0-1023).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn is_well_known_port(port: i64) -> bool {
    port >= OCTET_MIN && port <= WELL_KNOWN_PORT_MAX
}

/// Check if port is registered (1024-49151).
///
/// # Inputs
/// - `port`: The port number
///
/// # Returns
/// True if port is in registered range
///
/// # Network Is Registered Port
/// Checks if a port is in the registered range (1024-49151).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn is_registered_port(port: i64) -> bool {
    port >= REGISTERED_PORT_MIN && port <= REGISTERED_PORT_MAX
}

/// Check if port is dynamic/private (49152-65535).
///
/// # Inputs
/// - `port`: The port number
///
/// # Returns
/// True if port is in dynamic/private range
///
/// # Network Is Dynamic Port
/// Checks if a port is in the dynamic/private range (49152-65535).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn is_dynamic_port(port: i64) -> bool {
    port >= DYNAMIC_PORT_MIN && port <= PORT_MAX
}

// =============================================================================
// Network Address Formatting
// =============================================================================

/// Format network address (IP:Port).
///
/// # Inputs
/// - `ip`: The IP address
/// - `port`: The port number
///
/// # Returns
/// Formatted address string "IP:Port"
///
/// # Network Format Address
/// Formats an IP and port into "IP:Port" string.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn format_address(ip: String, port: i64) -> String {
    format!("{}:{}", ip, port)
}

/// Parse network address (IP:Port).
///
/// # Inputs
/// - `address`: The address string "IP:Port"
///
/// # Returns
/// The IP portion of the address
///
/// # Network Parse Address IP
/// Extracts the IP from "IP:Port" string.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn parse_address_ip(address: String) -> String {
    address.split(':').next().unwrap_or("").to_string()
}

/// Parse port from network address (IP:Port).
///
/// # Inputs
/// - `address`: The address string "IP:Port"
///
/// # Returns
/// The port number (or 0 if invalid)
///
/// # Network Parse Address Port
/// Extracts the port from "IP:Port" string.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn parse_address_port(address: String) -> i64 {
    address.split(':')
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

// =============================================================================
// Protocol Helpers
// =============================================================================

/// Get default port for HTTP.
///
/// # Returns
/// Default HTTP port (80)
///
/// # Network HTTP Port
/// Returns the default HTTP port (80).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn http_port() -> i64 {
    80
}

/// Get default port for HTTPS.
///
/// # Returns
/// Default HTTPS port (443)
///
/// # Network HTTPS Port
/// Returns the default HTTPS port (443).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn https_port() -> i64 {
    443
}

/// Get default port for FTP.
///
/// # Returns
/// Default FTP port (21)
///
/// # Network FTP Port
/// Returns the default FTP port (21).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn ftp_port() -> i64 {
    21
}

/// Get default port for SSH.
///
/// # Returns
/// Default SSH port (22)
///
/// # Network SSH Port
/// Returns the default SSH port (22).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn ssh_port() -> i64 {
    22
}

/// Get default port for SMTP.
///
/// # Returns
/// Default SMTP port (25)
///
/// # Network SMTP Port
/// Returns the default SMTP port (25).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn smtp_port() -> i64 {
    25
}

/// Get default port for DNS.
///
/// # Returns
/// Default DNS port (53)
///
/// # Network DNS Port
/// Returns the default DNS port (53).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn dns_port() -> i64 {
    53
}

/// Get default port for MySQL.
///
/// # Returns
/// Default MySQL port (3306)
///
/// # Network MySQL Port
/// Returns the default MySQL port (3306).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn mysql_port() -> i64 {
    3306
}

/// Get default port for PostgreSQL.
///
/// # Returns
/// Default PostgreSQL port (5432)
///
/// # Network PostgreSQL Port
/// Returns the default PostgreSQL port (5432).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn postgresql_port() -> i64 {
    5432
}

/// Get default port for Redis.
///
/// # Returns
/// Default Redis port (6379)
///
/// # Network Redis Port
/// Returns the default Redis port (6379).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn redis_port() -> i64 {
    6379
}

/// Get default port for MongoDB.
///
/// # Returns
/// Default MongoDB port (27017)
///
/// # Network MongoDB Port
/// Returns the default MongoDB port (27017).
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn mongodb_port() -> i64 {
    27017
}

// =============================================================================
// Subnet and CIDR
// =============================================================================

/// Parse CIDR notation.
///
/// # Inputs
/// - `cidr`: CIDR string (e.g., "192.168.1.0/24")
///
/// # Returns
/// The network address portion
///
/// # Network Parse CIDR
/// Parses CIDR notation and returns the network address.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn parse_cidr(cidr: String) -> String {
    cidr.split('/').next().unwrap_or("").to_string()
}

/// Get subnet mask from CIDR.
///
/// # Inputs
/// - `cidr`: CIDR string (e.g., "192.168.1.0/24")
///
/// # Returns
/// The subnet mask bits (0-32, or -1 if invalid)
///
/// # Network CIDR to Mask
/// Extracts subnet mask bits from CIDR notation.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn cidr_to_mask(cidr: String) -> i64 {
    cidr.split('/')
        .nth(1)
        .and_then(|m| m.parse().ok())
        .unwrap_or(-1)
}

/// Create CIDR notation.
///
/// # Inputs
/// - `ip`: The network address
/// - `mask_bits`: The subnet mask bits (0-32)
///
/// # Returns
/// CIDR notation string
///
/// # Network Create CIDR
/// Creates CIDR notation from IP and mask bits.
#[blueprint(type: crate::NodeTypes::pure, category: "Network", color: "#16A085")]
pub fn create_cidr(ip: String, mask_bits: i64) -> String {
    format!("{}/{}", ip, mask_bits.clamp(CIDR_MIN, IPV4_CIDR_MAX))
}
