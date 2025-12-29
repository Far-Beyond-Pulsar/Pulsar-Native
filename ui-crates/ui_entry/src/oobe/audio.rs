//! Intro Audio Manager
//!
//! Manages ambient sounds and UI audio for the intro experience
//! Uses rodio for audio playback with embedded MP3 file

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

// Embedded intro audio from assets/sound/intro.mp3
const INTRO_AUDIO: &[u8] = include_bytes!("../../../../assets/sound/intro.mp3");

/// Audio manager for intro sounds
pub struct IntroAudio {
    enabled: Arc<AtomicBool>,
    _stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
    ambient_sink: Arc<Mutex<Option<Sink>>>,
}

impl Clone for IntroAudio {
    fn clone(&self) -> Self {
        // Audio can't really be cloned, just share the enabled flag
        Self {
            enabled: self.enabled.clone(),
            _stream: None,
            stream_handle: None,
            ambient_sink: self.ambient_sink.clone(),
        }
    }
}

impl Default for IntroAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl IntroAudio {
    pub fn new() -> Self {
        // Try to create audio output
        let (stream, stream_handle) = match OutputStream::try_default() {
            Ok((s, h)) => (Some(s), Some(h)),
            Err(e) => {
                tracing::warn!("🔇 [OOBE] Could not initialize audio: {}", e);
                (None, None)
            }
        };
        
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            _stream: stream,
            stream_handle,
            ambient_sink: Arc::new(Mutex::new(None)),
        }
    }

    /// Enable or disable all intro audio
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.stop_all();
        }
    }

    /// Check if audio is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Play the ambient intro loop
    pub fn play_ambient(&self) {
        if !self.is_enabled() {
            return;
        }
        
        let Some(ref handle) = self.stream_handle else {
            tracing::info!("🔇 [OOBE] No audio device available");
            return;
        };
        
        // Use embedded intro.mp3
        let cursor = Cursor::new(INTRO_AUDIO);
        
        match Decoder::new(cursor) {
            Ok(source) => {
                match Sink::try_new(handle) {
                    Ok(sink) => {
                        sink.set_volume(0.5); // 50% volume
                        sink.append(source);
                        
                        tracing::info!("🔊 [OOBE] Playing intro audio (embedded MP3, {} bytes)", INTRO_AUDIO.len());
                        
                        // Store the sink so we can stop it later
                        *self.ambient_sink.lock() = Some(sink);
                    }
                    Err(e) => {
                        tracing::warn!("🔇 [OOBE] Could not create audio sink: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("🔇 [OOBE] Could not decode audio: {}", e);
            }
        }
    }

    /// Play a subtle UI click sound (short beep)
    pub fn play_click(&self) {
        if !self.is_enabled() {
            return;
        }
        // Click sound is very short, we won't implement it for now
        tracing::debug!("🔊 [OOBE] Click sound");
    }

    /// Play a whoosh/transition sound
    pub fn play_transition(&self) {
        if !self.is_enabled() {
            return;
        }
        tracing::debug!("🔊 [OOBE] Transition sound");
    }

    /// Play the success/complete sound
    pub fn play_complete(&self) {
        if !self.is_enabled() {
            return;
        }
        tracing::debug!("🔊 [OOBE] Completion sound");
    }

    /// Stop all sounds
    pub fn stop_all(&self) {
        if let Some(sink) = self.ambient_sink.lock().take() {
            sink.stop();
        }
        tracing::info!("🔊 [OOBE] Stopped all sounds");
    }
}

impl Drop for IntroAudio {
    fn drop(&mut self) {
        self.stop_all();
    }
}
