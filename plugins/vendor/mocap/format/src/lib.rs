//! The `.mocap` take format (`pulsar.mocap`).
//!
//! A take is a rig schema (joints and typed channels), dense sample columns,
//! markers and a calibration, stored as checksummed chunks so a recording can
//! stream to disk, survive a crash and open without decoding. The format is
//! specified in `.agents/MOCAP.md`; this crate is engine-free and GPUI-free so
//! the capture, the editor, the importer and a game's playback share it.

pub mod codec;
mod error;
mod reader;
mod rig;
pub mod signal;
mod take;
mod writer;

pub use error::FormatError;
pub use reader::{read_take, BlockRef, ReadReport, TakeFile};
pub use rig::{ChannelDesc, ChannelKind, Joint, Quantisation, RigProfile};
pub use take::{Calibration, CameraCalibration, Marker, SourceInfo, Take, TakeMeta};
pub use writer::{write_take, TakeWriter, DEFAULT_BLOCK_FRAMES, MAGIC, VERSION};

/// The file extension, without the dot.
pub const EXTENSION: &str = "mocap";

#[cfg(test)]
mod tests;
