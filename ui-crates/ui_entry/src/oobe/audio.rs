//! Intro Audio Manager
//!
//! Manages ambient sounds and UI audio for the intro experience
//! Uses rodio for audio playback with embedded MP3 file

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use parking_lot::Mutex;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

// Embedded intro audio from assets/sound/intro.mp3
const INTRO_AUDIO: &[u8] = include_bytes!("../../../../assets/sound/intro.mp3");

/// Audio state that must live on the audio thread
struct AudioState {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    ambient_sink: Option<Sink>,
}

/// Audio manager for intro sounds
pub struct IntroAudio {
    enabled: Arc<AtomicBool>,
    /// Commands to send to the audio thread
    command_tx: Option<std::sync::mpsc::Sender<AudioCommand>>,
    /// Handle to the audio thread
    _audio_thread: Option<thread::JoinHandle<()>>,
}

enum AudioCommand {
    PlayAmbient,
    StopAll,
    SetVolume(f32),
    Shutdown,
}

impl Clone for IntroAudio {
    fn clone(&self) -> Self {
        // Audio can't really be cloned, just share the enabled flag and command channel
        Self {
            enabled: self.enabled.clone(),
            command_tx: self.command_tx.clone(),
            _audio_thread: None, // Don't clone the thread handle
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
        let enabled = Arc::new(AtomicBool::new(true));
        let (command_tx, command_rx) = std::sync::mpsc::channel::<AudioCommand>();
        
        // Spawn audio thread to avoid blocking the main thread
        // Audio initialization can hang on some systems
        let audio_thread = thread::Builder::new()
            .name("oobe-audio".to_string())
            .spawn(move || {
                tracing::debug!("🔊 [OOBE] Audio thread starting...");
                
                // Try to create audio output with a timeout approach
                // OutputStream::try_default() can hang on some systems
                let audio_state = match OutputStream::try_default() {
                    Ok((stream, handle)) => {
                        tracing::debug!("🔊 [OOBE] Audio output initialized successfully");
                        Some(AudioState {
                            _stream: stream,
                            stream_handle: handle,
                            ambient_sink: None,
                        })
                    }
                    Err(e) => {
                        tracing::warn!("🔇 [OOBE] Could not initialize audio: {}", e);
                        None
                    }
                };
                
                let mut state = audio_state;
                
                // Process commands until shutdown
                while let Ok(cmd) = command_rx.recv() {
                    match cmd {
                        AudioCommand::PlayAmbient => {
                            if let Some(ref mut s) = state {
                                let cursor = Cursor::new(INTRO_AUDIO);
                                match Decoder::new(cursor) {
                                    Ok(source) => {
                                        match Sink::try_new(&s.stream_handle) {
                                            Ok(sink) => {
                                                sink.set_volume(0.5);
                                                sink.append(source);
                                                tracing::debug!("🔊 [OOBE] Playing intro audio (embedded MP3, {} bytes)", INTRO_AUDIO.len());
                                                s.ambient_sink = Some(sink);
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
                        }
                        AudioCommand::StopAll => {
                            if let Some(ref mut s) = state {
                                if let Some(sink) = s.ambient_sink.take() {
                                    sink.stop();
                                }
                                tracing::debug!("🔊 [OOBE] Stopped all sounds");
                            }
                        }
                        AudioCommand::SetVolume(vol) => {
                            if let Some(ref s) = state {
                                if let Some(ref sink) = s.ambient_sink {
                                    sink.set_volume(vol);
                                }
                            }
                        }
                        AudioCommand::Shutdown => {
                            tracing::debug!("🔊 [OOBE] Audio thread shutting down");
                            if let Some(ref mut s) = state {
                                if let Some(sink) = s.ambient_sink.take() {
                                    sink.stop();
                                }
                            }
                            break;
                        }
                    }
                }
                
                tracing::debug!("🔊 [OOBE] Audio thread exited");
            });
        
        let audio_thread = match audio_thread {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("🔇 [OOBE] Could not spawn audio thread: {}", e);
                None
            }
        };
        
        Self {
            enabled,
            command_tx: Some(command_tx),
            _audio_thread: audio_thread,
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
        
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(AudioCommand::PlayAmbient);
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
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(AudioCommand::StopAll);
        }
    }
}

impl Drop for IntroAudio {
    fn drop(&mut self) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(AudioCommand::Shutdown);
        }
    }
}
