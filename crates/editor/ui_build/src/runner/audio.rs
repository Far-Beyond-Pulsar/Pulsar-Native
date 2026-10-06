//! Audio feedback for build outcomes.

use std::io::Cursor;
use std::sync::OnceLock;

use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink};

const BUILD_SUCCESS_SOUND: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../assets/sound/success.mp3"
));
const BUILD_ERROR_SOUND: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../assets/sound/error.mp3"
));

static OUTPUT_STREAM: OnceLock<Option<OutputStream>> = OnceLock::new();

pub fn play_build_success() {
    play(BUILD_SUCCESS_SOUND);
}

pub fn play_build_error() {
    play(BUILD_ERROR_SOUND);
}

fn play(sound: &'static [u8]) {
    // Device setup and MP3 decoding can take a moment. Keep it off the UI task.
    std::thread::spawn(move || {
        let Some(stream) = OUTPUT_STREAM
            .get_or_init(|| OutputStreamBuilder::open_default_stream().ok())
            .as_ref()
        else {
            tracing::warn!("Could not open the default audio output for build sound");
            return;
        };

        let decoder = match Decoder::try_from(Cursor::new(sound)) {
            Ok(decoder) => decoder,
            Err(error) => {
                tracing::warn!(%error, "Could not decode build notification sound");
                return;
            }
        };

        let sink = Sink::connect_new(stream.mixer());
        sink.append(decoder);
        sink.detach();
    });
}
