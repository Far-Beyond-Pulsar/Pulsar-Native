//! Individual initialization step implementations.
//!
//! Each module exposes a single `run` function with the signature
//! `fn(&mut InitContext) -> Result<(), InitError>`, registered with the
//! [`InitGraph`](crate::init::InitGraph) via the `init_task!` macro in `main.rs`.

pub mod appdata;
pub mod backend;
pub mod dev_detect;
pub mod discord;
pub mod engine_context;
pub mod file_association;
pub mod logging;
pub mod runtime;
pub mod set_global;
pub mod settings;
pub mod uri_registration;
