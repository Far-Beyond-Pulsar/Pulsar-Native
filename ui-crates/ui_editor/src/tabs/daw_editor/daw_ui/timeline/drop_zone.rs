use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

pub fn render_drop_zone(
    track_id: uuid::Uuid,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let is_drag_target = matches!(
        &state.drag_state,
        DragState::DraggingFile { .. }
    );

    div()
        .absolute()
        .inset_0()
        // Make sure we capture pointer events when dragging
        .when(is_drag_target, |d| {
            d.border_2()
                .border_color(cx.theme().accent.opacity(0.3))
                .bg(cx.theme().accent.opacity(0.05))
        })
        // Handle mouse up to drop files onto track
        .on_mouse_up(gpui::MouseButton::Left, cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
            eprintln!("🖱️ Mouse up on track {} with drag state: {:?}", track_id, this.state.drag_state);
            if let DragState::DraggingFile { file_path, file_name } = &this.state.drag_state.clone() {
                eprintln!("📁 Dropping file '{}' onto track {}", file_name, track_id);
                // Convert window position to element-local position
                let element_pos = DawPanel::window_to_timeline_pos(event.position, this);
                let mouse_x = element_pos.x.as_f32();

                // Calculate beat position from mouse X
                let beat = this.state.pixels_to_beats(mouse_x);
                let tempo = this.state.get_tempo();
                let snap_mode = this.state.snap_mode;
                let snap_value = this.state.snap_value;

                // Apply snap if enabled
                let snapped_beat = if snap_mode == SnapMode::Grid {
                    let snap_beats = snap_value.to_beats();
                    (beat / snap_beats).round() * snap_beats
                } else {
                    beat
                };

                let file_path_clone = file_path.clone();
                let file_name_clone = file_name.clone();
                let snapped_beat_val = snapped_beat;
                let tempo_val = tempo;
                let track_id_val = track_id;

                // Check if audio service exists before spawning
                let has_service = this.state.audio_service.is_some();
                eprintln!("🔍 Audio service available: {}", has_service);

                if !has_service {
                    eprintln!("❌ No audio service - cannot load audio file");
                    // Clear drag state
                    this.state.drag_state = DragState::None;
                    cx.notify();
                    return;
                }

                // Load audio file asynchronously to get real duration
                cx.spawn(async move |this, cx| {
                    eprintln!("🔄 Async task started for loading audio file");

                    // Get the audio service
                    let service = match cx.update(|cx| {
                        this.update(cx, |this, _cx| {
                            this.state.audio_service.clone()
                        })
                    }) {
                        Ok(Ok(Some(svc))) => svc,
                        _ => {
                            eprintln!("❌ Failed to get audio service in async task");
                            return;
                        }
                    };

                    eprintln!("📂 Loading audio file: {:?}", file_path_clone);
                    match service.load_asset(file_path_clone.clone()).await {
                            Ok(asset) => {
                                let duration_samples = asset.asset_ref.duration_samples as u64;
                                eprintln!("✅ Audio file loaded successfully: {} samples", duration_samples);

                                // Update UI with the clip using real duration
                                cx.update(|cx| {
                                    this.update(cx, |this, cx| {
                                        // Cache the loaded asset
                                        this.state.loaded_assets.insert(file_path_clone.clone(), asset);

                                        // Create new clip with real duration
                                        if let Some(project) = &mut this.state.project {
                                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id_val) {
                                                // Convert beats to samples for start time
                                                let start_time = ((snapped_beat_val * 60.0 * SAMPLE_RATE as f64) / tempo_val as f64) as u64;

                                                let clip = crate::tabs::daw_editor::audio_types::AudioClip::new(
                                                    file_path_clone.clone(),
                                                    start_time,
                                                    duration_samples,
                                                );

                                                let clip_clone = clip.clone();
                                                track.clips.push(clip);

                                                let duration_beats = (duration_samples as f64 * tempo_val as f64) / (60.0 * SAMPLE_RATE as f64);
                                                eprintln!("📎 Created clip '{}' at beat {} on track '{}' (duration: {:.2} beats, {} samples)",
                                                    file_name_clone, snapped_beat_val, track.name, duration_beats, duration_samples);

                                                // IMPORTANT: Add clip to audio service's graph too!
                                                if let Some(ref service) = this.state.audio_service {
                                                    let service = service.clone();
                                                    let track_id = track_id_val;
                                                    cx.spawn(async move |_this, _cx| {
                                                        if let Err(e) = service.add_clip_to_track(track_id, clip_clone).await {
                                                            eprintln!("❌ Failed to add clip to audio service: {}", e);
                                                        } else {
                                                            eprintln!("✅ Added clip to audio service graph");
                                                        }
                                                    }).detach();
                                                }
                                            }
                                        }
                                        cx.notify();
                                    }).ok();
                                }).ok();
                            }
                            Err(e) => {
                                eprintln!("❌ Failed to load audio file '{}': {}", file_name_clone, e);

                                // Fallback: create clip with default duration
                                cx.update(|cx| {
                                    this.update(cx, |this, cx| {
                                        if let Some(project) = &mut this.state.project {
                                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id_val) {
                                                let start_time = ((snapped_beat_val * 60.0 * SAMPLE_RATE as f64) / tempo_val as f64) as u64;
                                                let duration = ((10.0 * 60.0 * SAMPLE_RATE as f64) / tempo_val as f64) as u64; // Fallback: 10 beats

                                                let clip = crate::tabs::daw_editor::audio_types::AudioClip::new(
                                                    file_path_clone.clone(),
                                                    start_time,
                                                    duration,
                                                );
                                                let clip_clone = clip.clone();
                                                track.clips.push(clip);
                                                eprintln!("📎 Created clip '{}' at beat {} with fallback duration (failed to load audio)",
                                                    file_name_clone, snapped_beat_val);

                                                // Add to audio service even if load failed (will try again at playback)
                                                if let Some(ref service) = this.state.audio_service {
                                                    let service = service.clone();
                                                    let track_id = track_id_val;
                                                    cx.spawn(async move |_this, _cx| {
                                                        if let Err(e) = service.add_clip_to_track(track_id, clip_clone).await {
                                                            eprintln!("❌ Failed to add clip to audio service: {}", e);
                                                        } else {
                                                            eprintln!("✅ Added clip to audio service graph (fallback)");
                                                        }
                                                    }).detach();
                                                }
                                            }
                                        }
                                        cx.notify();
                                    }).ok();
                                }).ok();
                            }
                    }
                }).detach();

                // Clear drag state immediately
                this.state.drag_state = DragState::None;
                cx.notify();
            }
        }))
        // Handle mouse move for clip dragging
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
            if let DragState::DraggingClip { clip_id, track_id: drag_track_id, start_beat, mouse_offset } = &this.state.drag_state.clone() {
                // Only update if dragging on THIS track
                if drag_track_id == &track_id {
                    // Convert window position to element-local position
                    let element_pos = DawPanel::window_to_timeline_pos(event.position, this);
                    let mouse_x = element_pos.x.as_f32() - mouse_offset.0;

                    // Calculate new beat position
                    let new_beat = this.state.pixels_to_beats(mouse_x);
                    let snap_mode = this.state.snap_mode;
                    let snap_value = this.state.snap_value;
                    let tempo = this.state.get_tempo();

                    // Apply snap if enabled
                    let snapped_beat = if snap_mode == SnapMode::Grid {
                        let snap_beats = snap_value.to_beats();
                        (new_beat / snap_beats).round() * snap_beats
                    } else {
                        new_beat
                    }.max(0.0);

                    // Update clip position
                    if let Some(project) = &mut this.state.project {
                        if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == *clip_id) {
                                clip.set_start_beat(snapped_beat, tempo);
                            }
                        }
                    }
                    cx.notify();
                }
            }
        }))
        // Handle mouse up for clip drop
        .on_mouse_up(gpui::MouseButton::Left, cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
            if let DragState::DraggingClip { clip_id, track_id: drag_track_id, start_beat, mouse_offset } = &this.state.drag_state.clone() {
                // Convert window position to element-local position
                let element_pos = DawPanel::window_to_timeline_pos(event.position, this);
                let mouse_x = element_pos.x.as_f32() - mouse_offset.0;

                // Calculate final beat position
                let new_beat = this.state.pixels_to_beats(mouse_x);
                let snap_mode = this.state.snap_mode;
                let snap_value = this.state.snap_value;
                let tempo = this.state.get_tempo();

                // Apply snap if enabled
                let snapped_beat = if snap_mode == SnapMode::Grid {
                    let snap_beats = snap_value.to_beats();
                    (new_beat / snap_beats).round() * snap_beats
                } else {
                    new_beat
                }.max(0.0);

                eprintln!("📍 Dropped clip at beat {} (snapped from {})",
                    snapped_beat, new_beat);

                // Finalize clip position
                let updated_clip = if let Some(project) = &mut this.state.project {
                    if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                        if let Some(clip) = track.clips.iter_mut().find(|c| c.id == *clip_id) {
                            clip.set_start_beat(snapped_beat, tempo);
                            eprintln!("✅ Final clip position: beat {}",
                                snapped_beat);
                            Some(clip.clone())
                        } else { None }
                    } else { None }
                } else { None };

                // Sync updated clip to audio service
                if let Some(clip) = updated_clip {
                    if let Some(ref service) = this.state.audio_service {
                        let service = service.clone();
                        let track_id_copy = track_id;
                        cx.spawn(async move |_this, _cx| {
                            // Remove old clip and add updated one
                            if let Err(e) = service.remove_clip_from_track(track_id_copy, clip.id).await {
                                eprintln!("❌ Failed to remove old clip from audio service: {}", e);
                            }
                            if let Err(e) = service.add_clip_to_track(track_id_copy, clip).await {
                                eprintln!("❌ Failed to add updated clip to audio service: {}", e);
                            } else {
                                eprintln!("✅ Updated clip position in audio service");
                            }
                        }).detach();
                    }
                }

                // Clear drag state
                this.state.drag_state = DragState::None;
                cx.notify();
            }
        }))
}