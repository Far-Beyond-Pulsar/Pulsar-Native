//! Communication Channels
//!
//! Channels for inter-component communication

use std::sync::mpsc::{channel, Sender, Receiver};
use ui_types_common::window_types::{WindowRequest, WindowId};

pub type WindowRequestSender = Sender<WindowRequest>;
pub type WindowRequestReceiver = Receiver<WindowRequest>;

/// Create a window request channel
pub fn window_request_channel() -> (WindowRequestSender, WindowRequestReceiver) {
    channel()
}