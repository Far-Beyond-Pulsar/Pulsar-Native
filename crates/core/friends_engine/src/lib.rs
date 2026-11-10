//! Friends list, mutual-friend detection and notifications for the editor.
//!
//! Owns: the friends model (`types`), persistence of each user's friend list
//! and relay "home servers" in a public GitHub gist (`gist_storage`), mutual
//! detection (`mutual_detection`), relay room keys / online checks
//! (`relay_integration`), and the WebSocket notification listener
//! (`notification_listener`).
//!
//! Entry points: the re-exported `friends_service` functions
//! (`get_friends_list`, `send_friend_request`, `accept_friend_request`,
//! `remove_friend`, `start_notification_listener`, `take_notifications`,
//! `is_authenticated`) and `normalize_relay_url`.
//!
//! Must not depend on: any UI crate (`ui_*`, `gpui`). Credentials come only
//! from `pulsar_auth`; the UI layer depends on this crate, never the reverse.
//! Network-free logic (URL normalisation, room keys, gist parsing) is unit
//! tested; everything that talks to GitHub or a relay is not.

pub mod friends_service;
pub mod gist_storage;
pub mod mutual_detection;
pub mod notification_listener;
pub mod relay_integration;
pub mod types;

pub use friends_service::*;
pub use gist_storage::normalize_relay_url;
pub use types::*;
