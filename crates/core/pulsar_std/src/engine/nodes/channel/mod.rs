//! # Channel Module
//!
//! Channel operations for the Pulsar visual programming system.
//!
//! This module provides thread-safe message-passing channel operations including:
//! - Creating channels (new)
//! - Sending messages (send)
//! - Receiving messages (recv)
//!
//! All operations use Rust's standard library MPSC (Multi-Producer, Single-Consumer) channels from `std::sync::mpsc`.

use crate::blueprint;
use std::sync::mpsc::{Receiver, Sender};

// =============================================================================
// Channel Operations
// =============================================================================

/// Creates a new channel for communication between threads.
///
/// # Returns
/// A tuple containing the sender and receiver ends of the channel
///
/// # Example
/// Use the sender to send messages from one thread, and the receiver to receive them in another thread.
///
/// # Notes
/// Channels are commonly used for passing data or signals between threads. The sender can be cloned to allow multiple producers.
/// The channel is unbounded and will buffer messages until they are received.
/// # Channel New
/// Creates a new message-passing channel for thread communication.
#[blueprint(type: NodeTypes::pure, category: "Channel", color: "#3498DB")]
pub fn channel_new() -> (Sender<String>, Receiver<String>) {
    std::sync::mpsc::channel()
}

/// Sends a message through a channel.
///
/// # Inputs
/// - `sender`: The channel sender to use for sending the message
/// - `message`: The message to send
///
/// # Returns
/// Ok(()) if the message was sent successfully, or an error string if sending failed
///
/// # Example
/// If the channel is open, the message will be delivered to the receiver. If the channel is closed, an error is returned.
///
/// # Notes
/// Use this node to implement inter-thread or inter-process communication patterns.
/// # Channel Send
/// Sends a message through a channel.
#[blueprint(type: NodeTypes::impure, category: "Channel", color: "#3498DB")]
pub fn channel_send(sender: Sender<String>, message: String) -> Result<(), String> {
    match sender.send(message) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Failed to send message: {}", e)),
    }
}

/// Receives a message from a channel.
///
/// # Inputs
/// - `receiver`: The receiver end of the channel to receive messages from
///
/// # Returns
/// The received message as a string, or an error message if receiving fails
///
/// # Example
/// If a message "hello" is sent to the channel, the output will be Ok("hello").
/// If the channel is closed, the output will be an Err with an error message.
///
/// # Notes
/// Use this node for thread synchronization or passing data between concurrent tasks.
/// The node blocks until a message is available or the channel is closed.
/// # Channel Recv
/// Receives a message from a channel (blocks until message is available).
#[blueprint(type: NodeTypes::impure, category: "Channel", color: "#3498DB")]
pub fn channel_recv(receiver: Receiver<String>) -> Result<String, String> {
    match receiver.recv() {
        Ok(message) => Ok(message),
        Err(e) => Err(format!("Failed to receive message: {}", e)),
    }
}
