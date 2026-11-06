//! File I/O and persistence layer
//!
//! - [`formats`]: the `.material` asset (its graph is the type the canvas edits)
//! - [`save_load`]: reading and writing it; legacy blueprint files are recognised
//!   by Graphy's codec

pub mod formats;
pub mod save_load;

