//! The multiuser editing session's network side: the WebSocket client of the
//! relay (`multiuser`) and the project file sync it carries (`simple_sync`).
//! Moved here from `engine_backend`, whose games have no use for it
//! (Pulsar-Native#1605).

pub mod multiuser;
pub mod simple_sync;
