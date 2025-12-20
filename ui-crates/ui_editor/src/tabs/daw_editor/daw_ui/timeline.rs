use super::state::*;
use super::panel::DawPanel;
use super::track_header;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};
use std::rc::Rc;
use std::ops::Range;
use crate::tabs::daw_editor::audio_types::SAMPLE_RATE;

const TIMELINE_HEADER_HEIGHT: f32 = 40.0;
const TRACK_HEADER_WIDTH: f32 = 200.0;
const MIN_TRACK_HEIGHT: f32 = 60.0;
const MAX_TRACK_HEIGHT: f32 = 300.0;

pub fn render_timeline(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Prepare virtualization item sizes for tracks
    let track_sizes: Rc<Vec<Size<Pixels>>> = {
        let tracks = state.project.as_ref()
            .map(|p| p.tracks.as_slice())
            .unwrap_or(&[]);
        
        Rc::new(
            tracks.iter()
                .map(|track| {
                    let height = *state.track_heights.get(&track.id)
                        .unwrap_or(&state.viewport.track_height);
                    Size {
                        width: px(9999.0), // Will be constrained by layout
                        height: px(height),
                    }
                })
                .collect()
        )
    };

    let panel_entity = cx.entity().clone();

    v_flex()
        .size_full()
        .bg(cx.theme().background)
        // Ruler/timeline header
        .child(render_ruler(state, cx))
        // Scrollable track area with virtualization
        .child(render_virtual_track_area(state, track_sizes, cx))
}

fn render_ruler(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let _tempo = state.project.as_ref().map(|p| p.transport.tempo).unwrap_or(120.0);
    let _zoom = state.viewport.zoom;
    let horizontal_scroll_handle = state.timeline_scroll_handle.clone();
    let view = cx.entity().clone();

    h_flex()
        .w_full()
        .h(px(TIMELINE_HEADER_HEIGHT))
        .bg(cx.theme().muted)
        .border_b_1()
        .border_color(cx.theme().border)
        // Track header spacer
        .child(
            div()
                .w(px(TRACK_HEADER_WIDTH))
                .h_full()
                .border_r_1()
                .border_color(cx.theme().border)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .child("TRACKS")
                )
        )
        // Timeline ruler with beat markings - now scrollable!
        .child(
            div()
                .flex_1()
                .h_full()
                .relative()
                .overflow_hidden()
                .child(
                    h_virtual_list(
                        view,
                        "ruler",
                        {
                            let total_width = state.beats_to_pixels(500.0);
                            let segment_width = 100.0;
                            let num_segments = (total_width / segment_width).ceil() as usize;
                            Rc::new(
                                (0..num_segments).map(|_| Size {
                                    width: px(segment_width),
                                    height: px(TIMELINE_HEADER_HEIGHT),
                                }).collect()
                            )
                        },
                        move |panel, visible_range, _window, cx| {
                            let total_width = panel.state.beats_to_pixels(500.0);
                            let segment_width = 100.0;
                            
                            visible_range.into_iter().map(|segment_idx| {
                                let start_x = segment_idx as f32 * segment_width;
                                render_ruler_segment(&panel.state, start_x, segment_width, total_width, cx)
                            }).collect()
                        },
                    )
                    .track_scroll(&horizontal_scroll_handle)
                )
        )
}

/// Render a segment of the ruler for horizontal virtual scrolling
fn render_ruler_segment(
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    div()
        .w(px(segment_width))
        .h_full()
        .relative()
        .bg(cx.theme().muted)
        // Render beat markings that intersect with this segment
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(total_width))  // Full timeline width
                .h_full()
                .children((0..=125).filter_map(|bar| {  // 500 beats / 4 = 125 bars
                    let beat = (bar * 4) as f64;
                    let x = state.beats_to_pixels(beat);
                    
                    // Only render if bar marking intersects with this segment
                    if x >= start_x && x < start_x + segment_width {
                        Some(div()
                            .absolute()
                            .left(px(x))
                            .h_full()
                            .child(
                                v_flex()
                                    .h_full()
                                    .child(
                                        div()
                                            .px_2()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().foreground)
                                            .child(format!("{}", bar + 1))
                                    )
                                    .child(
                                        div()
                                            .w_px()
                                            .flex_1()
                                            .bg(cx.theme().border)
                                    )
                            ))
                    } else {
                        None
                    }
                }))
        )
        // Playhead (full width, offset to account for segment position)
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(total_width))  // Full timeline width
                .h_full()
                .child(render_playhead(state, cx))
        )
}


fn render_playhead(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let x = state.beats_to_pixels(state.selection.playhead_position);

    div()
        .absolute()
        .left(px(x))
        .top_0()
        .bottom_0()
        .w(px(2.0))
        .bg(cx.theme().accent)
        
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(-6.0))
                .w(px(14.0))
                .h(px(14.0))
                .bg(cx.theme().accent)
                .child(
                    Icon::new(IconName::Play)
                        .size_3()
                        .text_color(cx.theme().accent_foreground)
                )
        )
}

/// Render virtualized track area - using Table pattern with uniform_list
fn render_virtual_track_area(
    state: &DawUiState,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let panel_entity = cx.entity().clone();
    let num_tracks = state.project.as_ref().map(|p| p.tracks.len()).unwrap_or(0);
    let total_beats = 500.0;
    let total_width = state.beats_to_pixels(total_beats);
    let vertical_scroll_handle = state.timeline_vertical_scroll_handle.clone();
    
    div()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
        .child(
            uniform_list(
                "timeline-track-rows",
                num_tracks,
                cx.processor(move |panel: &mut DawPanel, visible_range: Range<usize>, _window, cx| {
                    let tracks = panel.state.project.as_ref()
                        .map(|p| &p.tracks)
                        .map(|t| t.as_slice())
                        .unwrap_or(&[]);
                    let total_width = panel.state.beats_to_pixels(500.0);
                    
                    visible_range.into_iter().filter_map(|track_idx| {
                        tracks.get(track_idx).map(|track| {
                            render_track_row(track, &panel.state, total_width, cx)
                        })
                    }).collect()
                })
            )
            .size_full()
            .track_scroll(vertical_scroll_handle)
            .with_sizing_behavior(ListSizingBehavior::Infer)
        )
        // Scrollbar overlay for horizontal scrolling
        .child(
            div()
                .absolute()
                .inset_0()
                .child(
                    Scrollbar::both(
                        &state.timeline_scroll_state,
                        &state.timeline_scroll_handle,
                    )
                    .axis(ScrollbarAxis::Horizontal)
                )
        )
}

/// Render a single track row - fixed header on left, scrollable content on right (Table pattern)
fn render_track_row(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;
    let track_height = *state.track_heights.get(&track_id)
        .unwrap_or(&state.viewport.track_height);
    let horizontal_scroll_handle = state.timeline_scroll_handle.clone();
    let view = cx.entity().clone();
    
    // For horizontal virtual scrolling, we need to split the timeline into segments
    // Let's use 100px wide segments
    let segment_width = 100.0;
    let num_segments = (total_width / segment_width).ceil() as usize;
    let segment_sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
        (0..num_segments).map(|_| Size {
            width: px(segment_width),
            height: px(track_height),
        }).collect()
    );
    
    h_flex()
        .w_full()
        .h(px(track_height))
        .border_b_1()
        .border_color(cx.theme().border)
        // Fixed left: track header (like Table's fixed left columns)
        .child(
            div()
                .w(px(TRACK_HEADER_WIDTH))
                .h_full()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(track_header::render_track_header(track, state, cx))
        )
        // Scrollable right: track content using horizontal virtual_list (like Table's scrollable columns)
        .child(
            div()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .relative()
                .child(
                    h_virtual_list(
                        view.clone(),
                        track_id,
                        segment_sizes,
                        {
                            let track = track.clone();
                            move |panel, visible_range, _window, cx| {
                                let total_width = panel.state.beats_to_pixels(500.0);
                                let segment_width = 100.0;
                                
                                // Render the visible segments of the timeline
                                visible_range.into_iter().map(|segment_idx| {
                                    let start_x = segment_idx as f32 * segment_width;
                                    let end_x = ((segment_idx + 1) as f32 * segment_width).min(total_width);
                                    let segment_w = end_x - start_x;
                                    
                                    render_track_content_segment(&track, &panel.state, start_x, segment_w, total_width, cx)
                                }).collect()
                            }
                        },
                    )
                    .track_scroll(&horizontal_scroll_handle)
                )
                // Add ScrollableMask as a sibling to enable scroll wheel handling
                .child(
                    ui::scroll::ScrollableMask::new(
                        view.entity_id(),
                        Axis::Horizontal,
                        horizontal_scroll_handle.as_ref(),
                    )
                )
        )
}

/// Render a segment of the track content for horizontal virtual scrolling
fn render_track_content_segment(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    _total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;

    div()
        .w(px(segment_width))
        .h_full()
        .relative()
        .bg(cx.theme().background)
        // Grid lines for this segment
        .child(render_grid_lines_segment(state, start_x, segment_width, cx))
        // Drop zone for the entire timeline (full width, positioned to cover this segment)
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(state.beats_to_pixels(500.0)))  // Full timeline width
                .h_full()
                .child(render_drop_zone(track_id, state, cx))
        )
        // Render clips that intersect with this segment
        // Clips are absolutely positioned, so we render the full content but clipped to segment
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(state.beats_to_pixels(500.0)))  // Full timeline width
                .h_full()
                .children(track.clips.iter().filter_map(|clip| {
                    let tempo = state.get_tempo();
                    let clip_start = state.beats_to_pixels(clip.start_beat(tempo));
                    let clip_end = clip_start + state.beats_to_pixels(clip.duration_beats(tempo));
                    
                    // Only render if clip intersects with this segment
                    if clip_end >= start_x && clip_start < start_x + segment_width {
                        Some(render_clip(clip, track_id, state, cx))
                    } else {
                        None
                    }
                }))
        )
}

/// Render grid lines for a segment
fn render_grid_lines_segment(
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let beat_width = state.beats_to_pixels(1.0);
    let start_beat = (start_x / beat_width).floor() as i32;
    let end_beat = ((start_x + segment_width) / beat_width).ceil() as i32;
    
    div()
        .absolute()
        .inset_0()
        .children((start_beat..=end_beat).map(|beat| {
            let x = state.beats_to_pixels(beat as f64) - start_x;
            let is_bar = beat % 4 == 0;
            
            div()
                .absolute()
                .left(px(x))
                .top_0()
                .bottom_0()
                .w_px()
                .bg(if is_bar {
                    cx.theme().border
                } else {
                    cx.theme().border.opacity(0.3)
                })
        }))
}

/// Render track timeline content (clips, automation, etc.)
fn render_track_content(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;
    let track_height = *state.track_heights.get(&track_id)
        .unwrap_or(&state.viewport.track_height);

    div()
        .w(px(total_width))
        .h_full()
        .relative()
        .bg(cx.theme().background)
        // Grid lines
        .child(render_grid_lines(state, cx))
        // Drop zone for dragging files/clips
        .child(render_drop_zone(track_id, state, cx))
        // Render clips
        .children(track.clips.iter().map(|clip| {
            render_clip(clip, track_id, state, cx)
        }))
}

fn render_drop_zone(
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

fn render_clip(
    clip: &crate::tabs::daw_editor::audio_types::AudioClip,
    track_id: uuid::Uuid,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let tempo = state.get_tempo();
    let x = state.beats_to_pixels(clip.start_beat(tempo));
    let width = state.beats_to_pixels(clip.duration_beats(tempo));
    let is_selected = state.selection.selected_clip_ids.contains(&clip.id);
    let clip_id = clip.id;

    let file_name = std::path::Path::new(&clip.asset_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Clip".to_string());

    // Get track color for clip coloring
    let track_idx = state.project.as_ref()
        .and_then(|p| p.tracks.iter().position(|t| t.id == track_id))
        .unwrap_or(0);

    // Generate consistent color per track
    let track_hue = (track_idx as f32 * 137.5) % 360.0; // Golden angle
    let clip_color = hsla(track_hue / 360.0, 0.5, 0.45, 1.0);
    let clip_border_color = hsla(track_hue / 360.0, 0.7, 0.35, 1.0);

    let track_height = *state.track_heights.get(&track_id)
        .unwrap_or(&state.viewport.track_height);

    div()
        .id(ElementId::Name(format!("clip-{}", clip_id).into()))
        .absolute()
        .left(px(x))
        .top(px(4.0))
        .w(px(width))
        .h(px(track_height - 8.0))
        .rounded_sm()
        .overflow_hidden()
        .cursor_pointer()
        .when(is_selected, |d| {
            d.border_2().border_color(cx.theme().accent).shadow_lg()
        })
        .when(!is_selected, |d| {
            d.border_1().border_color(clip_border_color)
        })
        .bg(clip_color)
        .hover(|d| d.bg(clip_color.opacity(0.9)))
        .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.state.select_clip(clip_id, false);
            cx.notify();
        }))
        // Make clips draggable with mouse down
        .on_mouse_down(gpui::MouseButton::Left, cx.listener({
            let start_beat = clip.start_beat(tempo);
            move |this, event: &MouseDownEvent, _window, cx| {
                // Use proper coordinate conversion: window → element
                let element_pos = DawPanel::window_to_timeline_pos(event.position, this);
                let mouse_x = element_pos.x.as_f32();
                let clip_x = x;

                this.state.drag_state = DragState::DraggingClip {
                    clip_id,
                    track_id,
                    start_beat,
                    mouse_offset: (mouse_x - clip_x, 0.0),
                };
                // Also select the clip
                this.state.select_clip(clip_id, false);
                cx.notify();
            }
        }))
        .child(
            v_flex()
                .size_full()
                .px_2()
                .py_1()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().background) // Contrast with clip color
                        .child(file_name)
                )
                .child(
                    div()
                        .flex_1()
                        .relative()
                        // Placeholder waveform with track-colored tint
                        .child(render_waveform_placeholder(clip_color, cx))
                )
        )
}

fn render_waveform_placeholder(tint_color: Hsla, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Darken the tint color manually
    let darkened_color = hsla(
        tint_color.h,
        tint_color.s,
        (tint_color.l * 0.7).max(0.0),
        tint_color.a
    );

    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::Activity)
                .size_4()
                .text_color(darkened_color)
        )
}

fn render_grid_lines(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let _zoom = state.viewport.zoom;
    let num_beats = 500; // Total beats to show

    div()
        .absolute()
        .inset_0()
        
        .children((0..num_beats).step_by(4).map(|beat| {
            let x = state.beats_to_pixels(beat as f64);

            div()
                .absolute()
                .left(px(x))
                .top_0()
                .bottom_0()
                .w_px()
                .bg(cx.theme().border.opacity(0.3))
        }))
}