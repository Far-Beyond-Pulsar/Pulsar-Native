//! Friends panel and popover: friend list, pending requests and session invites.
//!
//! Owns: `FriendsScreen` (tabs for online / pending / all, add-friend flow,
//! avatar loading) and `FriendsPopover` (invite a friend to a session), plus the
//! view-model types in `utils::types` (`FriendEntry`, `FriendTab`,
//! `AddFriendState`).
//!
//! Entry points: `FriendsScreen::new`, `FriendsScreen::new_without_invite`,
//! `FriendsPopover`.
//!
//! Must not depend on: GitHub or relay HTTP details. All friend data and
//! mutations go through `friends_engine` (and sign-in state through
//! `pulsar_auth`); blocking calls run on worker threads, never the UI thread.
pub mod components;
mod handlers;
mod screen;
pub mod utils;

pub use components::popover::FriendsPopover;
pub use screen::FriendsScreen;
pub use utils::*;
