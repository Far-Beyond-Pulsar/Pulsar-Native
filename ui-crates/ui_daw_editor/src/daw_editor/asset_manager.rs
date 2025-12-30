/// Asynchronous audio asset loading and caching system
use super::audio_types::*;
use anyhow::{Context as AnyhowContext, Result};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use smol::channel;

/// Asset manager handles async loading and caching of audio files
pub struct AssetManager {
    cache: Arc<DashMap<PathBuf, Arc<AudioAssetData>>>,
    loading: Arc<DashMap<PathBuf, Arc<channel::Sender<()>>>>,
}

impl AssetManager {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            loading: Arc::new(DashMap::new()),
        }
    }

    /// Load an audio asset asynchronously
    pub async fn load_asset(&self, path: PathBuf) -> Result<Arc<AudioAssetData>> {
        if let Some(cached) = self.cache.get(&path) {
            return Ok(cached.clone());
        }

        let (tx, rx) = {
            if let Some(loading) = self.loading.get(&path) {
                let tx = loading.clone();
                drop(loading);
                let (_, rx) = channel::bounded::<()>(1);
                // Wait for the loading to complete
                let _ = rx.recv().await;

                if let Some(cached) = self.cache.get(&path) {
                    return Ok(cached.clone());
                }
                return Err(anyhow::anyhow!("Asset loading failed"));
            }

            let (tx, rx) = channel::bounded::<()>(1);
            let tx = Arc::new(tx);
            self.loading.insert(path.clone(), tx.clone());
            (tx, rx)
        };

        let result = self.load_asset_internal(&path).await;

        self.loading.remove(&path);
        let _ = tx.send(()).await; // Notify waiters

        match result {
            Ok(data) => {
                let data = Arc::new(data);
                self.cache.insert(path, data.clone());
                Ok(data)
            }
            Err(e) => Err(e),
        }
    }

    async fn load_asset_internal(&self, path: &Path) -> Result<AudioAssetData> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .context("No file extension")?
            .to_lowercase();

        match extension.as_str() {
            "wav" => self.load_wav(path).await,
            "ogg" => self.load_ogg(path).await,
            "flac" => self.load_flac(path).await,
            "mp3" => self.load_mp3(path).await,
            _ => Err(anyhow::anyhow!("Unsupported audio format: {}", extension)),
        }
    }

    async fn load_wav(&self, path: &Path) -> Result<AudioAssetData> {
        let path = path.to_owned();

        smol::unblock(move || {
            let mut reader = hound::WavReader::open(&path)
                .context("Failed to open WAV file")?;
            
            let spec = reader.spec();
            let sample_rate = spec.sample_rate as f32;
            let channels = spec.channels as usize;
            let duration_samples = reader.duration() as usize;

            let samples: Vec<f32> = match spec.sample_format {
                hound::SampleFormat::Float => {
                    reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?
                }
                hound::SampleFormat::Int => {
                    let max_val = (1 << (spec.bits_per_sample - 1)) as f32;
                    reader
                        .samples::<i32>()
                        .map(|s| s.map(|v| v as f32 / max_val))
                        .collect::<Result<Vec<_>, _>>()?
                }
            };

            let samples = if sample_rate != SAMPLE_RATE {
                Self::resample(&samples, sample_rate, SAMPLE_RATE, channels)
            } else {
                samples
            };

            Ok(AudioAssetData {
                asset_ref: AudioAssetRef {
                    path,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    duration_samples: samples.len() / channels,
                },
                samples: Arc::new(samples),
            })
        })
        .await
    }

    async fn load_ogg(&self, path: &Path) -> Result<AudioAssetData> {
        let path = path.to_owned();

        smol::unblock(move || {
            let file = std::fs::File::open(&path)?;
            let mut reader = lewton::inside_ogg::OggStreamReader::new(file)?;
            
            let sample_rate = reader.ident_hdr.audio_sample_rate as f32;
            let channels = reader.ident_hdr.audio_channels as usize;
            
            let mut samples = Vec::new();
            while let Some(packet) = reader.read_dec_packet_itl()? {
                for sample in packet {
                    samples.push(sample as f32 / i16::MAX as f32);
                }
            }

            let samples = if sample_rate != SAMPLE_RATE {
                Self::resample(&samples, sample_rate, SAMPLE_RATE, channels)
            } else {
                samples
            };

            Ok(AudioAssetData {
                asset_ref: AudioAssetRef {
                    path,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    duration_samples: samples.len() / channels,
                },
                samples: Arc::new(samples),
            })
        })
        .await
    }

    async fn load_flac(&self, path: &Path) -> Result<AudioAssetData> {
        let path = path.to_owned();

        smol::unblock(move || {
            let mut reader = claxon::FlacReader::open(&path)?;
            let info = reader.streaminfo();
            
            let sample_rate = info.sample_rate as f32;
            let channels = info.channels as usize;
            let bits_per_sample = info.bits_per_sample;
            
            let max_val = (1 << (bits_per_sample - 1)) as f32;
            let mut samples = Vec::new();
            
            for sample in reader.samples() {
                let sample = sample?;
                samples.push(sample as f32 / max_val);
            }

            let samples = if sample_rate != SAMPLE_RATE {
                Self::resample(&samples, sample_rate, SAMPLE_RATE, channels)
            } else {
                samples
            };

            Ok(AudioAssetData {
                asset_ref: AudioAssetRef {
                    path,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    duration_samples: samples.len() / channels,
                },
                samples: Arc::new(samples),
            })
        })
        .await
    }

    async fn load_mp3(&self, path: &Path) -> Result<AudioAssetData> {
        let path = path.to_owned();

        smol::unblock(move || {
            use symphonia::core::audio::{AudioBufferRef, Signal};
            use symphonia::core::codecs::DecoderOptions;
            use symphonia::core::errors::Error as SymphoniaError;
            use symphonia::core::formats::FormatOptions;
            use symphonia::core::io::MediaSourceStream;
            use symphonia::core::meta::MetadataOptions;
            use symphonia::core::probe::Hint;

            let file = std::fs::File::open(&path)?;
            let mss = MediaSourceStream::new(Box::new(file), Default::default());

            let mut hint = Hint::new();
            hint.with_extension("mp3");

            let mut format = symphonia::default::get_probe()
                .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
                .context("Failed to probe audio format")?
                .format;

            let track = format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
                .context("No valid audio track found")?;

            let mut decoder = symphonia::default::get_codecs()
                .make(&track.codec_params, &DecoderOptions::default())
                .context("Failed to create decoder")?;

            let track_id = track.id;
            let mut samples = Vec::new();
            let mut sample_rate = 0;
            let mut channels = 0;

            loop {
                let packet = match format.next_packet() {
                    Ok(packet) => packet,
                    Err(SymphoniaError::ResetRequired) => {
                        decoder.reset();
                        continue;
                    }
                    Err(SymphoniaError::IoError(ref err)) 
                        if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(err) => return Err(anyhow::anyhow!("Format error: {}", err)),
                };

                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => {
                        if sample_rate == 0 {
                            let spec = decoded.spec();
                            sample_rate = spec.rate;
                            channels = spec.channels.count();
                        }

                        // Convert samples to f32
                        match decoded {
                            AudioBufferRef::F32(buf) => {
                                for ch in 0..buf.spec().channels.count() {
                                    let channel_samples = buf.chan(ch);
                                    samples.extend_from_slice(channel_samples);
                                }
                            }
                            AudioBufferRef::S16(buf) => {
                                for ch in 0..buf.spec().channels.count() {
                                    let channel_samples = buf.chan(ch);
                                    for &sample in channel_samples {
                                        samples.push(sample as f32 / i16::MAX as f32);
                                    }
                                }
                            }
                            AudioBufferRef::S32(buf) => {
                                for ch in 0..buf.spec().channels.count() {
                                    let channel_samples = buf.chan(ch);
                                    for &sample in channel_samples {
                                        samples.push(sample as f32 / i32::MAX as f32);
                                    }
                                }
                            }
                            _ => {
                                return Err(anyhow::anyhow!("Unsupported audio format"));
                            }
                        }
                    }
                    Err(SymphoniaError::IoError(ref err)) 
                        if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(SymphoniaError::DecodeError(_)) => continue,
                    Err(err) => return Err(anyhow::anyhow!("Decode error: {}", err)),
                }
            }

            let sample_rate_f32 = sample_rate as f32;
            let samples = if sample_rate_f32 != SAMPLE_RATE {
                Self::resample(&samples, sample_rate_f32, SAMPLE_RATE, channels)
            } else {
                samples
            };

            Ok(AudioAssetData {
                asset_ref: AudioAssetRef {
                    path,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    duration_samples: samples.len() / channels,
                },
                samples: Arc::new(samples),
            })
        })
        .await
    }

    /// Simple linear resampling
    fn resample(input: &[f32], from_rate: f32, to_rate: f32, channels: usize) -> Vec<f32> {
        if from_rate == to_rate {
            return input.to_vec();
        }

        let ratio = from_rate / to_rate;
        let input_frames = input.len() / channels;
        let output_frames = (input_frames as f32 / ratio).ceil() as usize;
        let mut output = Vec::with_capacity(output_frames * channels);

        for out_frame in 0..output_frames {
            let in_frame_f = out_frame as f32 * ratio;
            let in_frame = in_frame_f as usize;
            let frac = in_frame_f - in_frame as f32;

            for ch in 0..channels {
                let idx1 = in_frame * channels + ch;
                let idx2 = ((in_frame + 1).min(input_frames - 1)) * channels + ch;

                let s1 = input.get(idx1).copied().unwrap_or(0.0);
                let s2 = input.get(idx2).copied().unwrap_or(s1);

                let interpolated = s1 + (s2 - s1) * frac;
                output.push(interpolated);
            }
        }

        output
    }

    /// Get cached asset if available
    pub fn get_cached(&self, path: &Path) -> Option<Arc<AudioAssetData>> {
        self.cache.get(path).map(|v| v.clone())
    }

    /// Clear all cached assets
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        let count = self.cache.len();
        let total_bytes: usize = self
            .cache
            .iter()
            .map(|entry| entry.samples.len() * std::mem::size_of::<f32>())
            .sum();
        (count, total_bytes)
    }

    /// Preload multiple assets
    pub async fn preload_assets(&self, paths: Vec<PathBuf>) -> Vec<Result<Arc<AudioAssetData>>> {
        // Load assets concurrently using futures::future::join_all
        let futures: Vec<_> = paths.into_iter().map(|path| {
            let manager = self.clone();
            async move { manager.load_asset(path).await }
        }).collect();

        futures::future::join_all(futures).await
    }
}

impl Clone for AssetManager {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            loading: self.loading.clone(),
        }
    }
}

impl Default for AssetManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resample() {
        let input = vec![0.0, 0.5, 1.0, 0.5];
        let output = AssetManager::resample(&input, 44100.0, 48000.0, 1);
        assert!(output.len() > input.len());
    }

    #[test]
    fn test_cache_stats() {
        let manager = AssetManager::new();
        let (count, bytes) = manager.cache_stats();
        assert_eq!(count, 0);
        assert_eq!(bytes, 0);
    }
}
