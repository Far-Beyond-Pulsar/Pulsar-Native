// WASM-safe modules — always included
pub mod math;
pub use math::*;
pub mod logic;
pub use logic::*;
pub mod flow;
pub use flow::*;
pub mod string;
pub use string::*;
pub mod vector;
pub use vector::*;
pub mod color;
pub use color::*;
pub mod transform;
pub use transform::*;
pub mod rect;
pub use rect::*;
pub mod easing;
pub use easing::*;
pub mod conversion;
pub use conversion::*;
pub mod validation;
pub use validation::*;
pub mod events;
pub use events::*;
pub mod collections;
pub use collections::*;
pub mod array;
pub use array::*;
pub mod random;
pub use random::*;
pub mod json;
pub use json::*;
pub mod url;
pub use url::*;

// These use std::sync::atomic which is supported in wasm32
pub mod atomic;
pub use atomic::*;

// Testing / assertion nodes — always available, wasm-safe (panic = wasm trap)
pub mod testing;
pub use testing::*;

// Native-only modules: threads, OS, filesystem, network
#[cfg(not(target_arch = "wasm32"))]
pub mod debug;
#[cfg(not(target_arch = "wasm32"))]
pub use debug::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod file_io;
#[cfg(not(target_arch = "wasm32"))]
pub use file_io::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod system;
#[cfg(not(target_arch = "wasm32"))]
pub use system::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod channel;
#[cfg(not(target_arch = "wasm32"))]
pub use channel::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod thread;
#[cfg(not(target_arch = "wasm32"))]
pub use thread::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod timer;
#[cfg(not(target_arch = "wasm32"))]
pub use timer::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod mutex;
#[cfg(not(target_arch = "wasm32"))]
pub use mutex::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod http;
#[cfg(not(target_arch = "wasm32"))]
pub use http::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod path;
#[cfg(not(target_arch = "wasm32"))]
pub use path::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod env;
#[cfg(not(target_arch = "wasm32"))]
pub use env::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod shell;
#[cfg(not(target_arch = "wasm32"))]
pub use shell::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod process;
#[cfg(not(target_arch = "wasm32"))]
pub use process::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod datetime;
#[cfg(not(target_arch = "wasm32"))]
pub use datetime::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod crypto;
#[cfg(not(target_arch = "wasm32"))]
pub use crypto::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod network;
#[cfg(not(target_arch = "wasm32"))]
pub use network::*;
