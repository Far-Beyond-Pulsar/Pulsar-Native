//! The multiuser editing session's network side: the WebSocket client of the
//! relay (`multiuser`) and the project file sync it carries (`simple_sync`).
//! Moved here from `engine_backend`, whose games have no use for it
//! (Pulsar-Native#1605).

/// The relay client lives in `pulsar_studio_client` (no UI or engine
/// dependency, so the Hub can use it); re-exported at its old path.
pub mod multiuser {
    pub use pulsar_studio_client::*;
}
pub mod simple_sync;
