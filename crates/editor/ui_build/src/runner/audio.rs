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
    if !enabled("play_success_sound") {
        return;
    }
    play(BUILD_SUCCESS_SOUND);
}

pub fn play_build_error() {
    if !enabled("play_error_sound") {
        return;
    }
    play(BUILD_ERROR_SOUND);
}

fn enabled(key: &str) -> bool {
    engine_state::global_config()
        .get(
            engine_state::settings::NS_EDITOR,
            "build_notifications",
            key,
        )
        .ok()
        .and_then(|value| value.as_bool().ok())
        .unwrap_or(true)
}

fn volume() -> f32 {
    engine_state::global_config()
        .get(
            engine_state::settings::NS_EDITOR,
            "build_notifications",
            "volume",
        )
        .ok()
        .and_then(|value| value.as_float().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0) as f32
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
        sink.set_volume(volume());
        sink.append(decoder);
        sink.detach();
    });
}
