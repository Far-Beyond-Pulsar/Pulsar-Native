/// Transport Controls Component
/// Play, stop, record, loop, metronome, and timeline position

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, divider::Divider,
};
use crate::tabs::daw_editor::audio_types::SAMPLE_RATE;
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_transport<V: 'static>(state: &mut DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<V>) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(60.0))
        .px_4()
        .gap_3()
        .items_center()
        .bg(cx.theme().muted.opacity(0.15))
        .border_b_1()
        .border_color(cx.theme().border)
        // Transport buttons
        .child(render_transport_buttons(state, state_arc.clone(), cx))
        .child(Divider::vertical().h(px(36.0)).bg(cx.theme().border))
        // Timeline position display
        .child(render_position_display(state, cx))
        .child(Divider::vertical().h(px(36.0)).bg(cx.theme().border))
        // Tempo and time signature
        .child(render_tempo_section(state, cx))
        .child(div().flex_1())
        // Loop section
        .child(render_loop_section(state, state_arc.clone(), cx))
        .child(Divider::vertical().h(px(36.0)).bg(cx.theme().border))
        // Metronome and count-in
        .child(render_metronome_section(state, state_arc.clone(), cx))
}

fn render_transport_buttons<V: 'static>(state: &mut DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<V>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        // Go to start
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-start")
                .icon(Icon::new(IconName::ChevronLeft))
                .ghost()
                .small()
                .tooltip("Go to Start")
                .on_click(move |_, window, _cx| {
                    state_arc.write().set_playhead(0.0);
                    window.refresh();
                })
        })
        // Stop
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-stop")
                .icon(Icon::new(IconName::Square))
                .ghost()
                .small()
                .tooltip("Stop")
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.is_playing = false;
                    state.selection.playhead_position = 0.0;
                    window.refresh();
                })
        })
        // Play/Pause
        .child({
            let tooltip_text = if state.is_playing { "Pause" } else { "Play" };
            let is_playing = state.is_playing;
            let state_arc = state_arc.clone();
            Button::new("transport-play")
                .icon(Icon::new(if is_playing {
                    IconName::Pause
                } else {
                    IconName::Play
                }))
                .primary()
                .small()
                .tooltip(tooltip_text)
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.is_playing = !state.is_playing;
                    window.refresh();
                })
        })
        // Record
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-record")
                .icon(Icon::new(IconName::Circle))
                .danger()
                .ghost()
                .when(state.is_recording, |b| b.danger())
                .small()
                .tooltip("Record")
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.is_recording = !state.is_recording;
                    window.refresh();
                })
        })
}

fn render_position_display<V: 'static>(state: &mut DawUiState, cx: &mut Context<V>) -> impl IntoElement {
    let position = state.selection.playhead_position;
    let tempo = state.project.as_ref()
        .map(|p| p.transport.tempo)
        .unwrap_or(120.0);

    // Convert beats to time format
    let seconds = (position / tempo as f64) * 60.0;
    let minutes = (seconds / 60.0).floor() as u32;
    let secs = (seconds % 60.0).floor() as u32;
    let millis = ((seconds % 1.0) * 1000.0).floor() as u32;

    // Convert to bars:beats format
    let bars = (position / 4.0).floor() as u32 + 1;
    let beats = (position % 4.0).floor() as u32 + 1;
    let subdivisions = ((position % 1.0) * 100.0).floor() as u32;

    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_family("monospace")
                        .text_color(cx.theme().foreground)
                        .child(format!("{:02}:{:02}.{:03}", minutes, secs, millis))
                )
        )
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_family("monospace")
                        .text_color(cx.theme().foreground)
                        .child(format!("{:03}.{}.{:02}", bars, beats, subdivisions))
                )
        )
}

fn render_tempo_section<V: 'static>(state: &mut DawUiState, cx: &mut Context<V>) -> impl IntoElement {
    let tempo = state.project.as_ref()
        .map(|p| p.transport.tempo)
        .unwrap_or(120.0);

    let time_sig_num = state.project.as_ref()
        .map(|p| p.transport.time_signature_numerator)
        .unwrap_or(4);

    let time_sig_denom = state.project.as_ref()
        .map(|p| p.transport.time_signature_denominator)
        .unwrap_or(4);

    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .hover(|d| d.bg(cx.theme().muted.opacity(0.2)))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Timer).size_4().text_color(cx.theme().muted_foreground))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(format!("{:.1}", tempo))
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("BPM")
                        )
                )
        )
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .hover(|d| d.bg(cx.theme().muted.opacity(0.2)))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Heart).size_4().text_color(cx.theme().muted_foreground))
                        .child(
                            div()
                                .text_sm()
                                .font_family("monospace")
                                .text_color(cx.theme().foreground)
                                .child(format!("{}/{}", time_sig_num, time_sig_denom))
                        )
                )
        )
}

fn render_loop_section<V: 'static>(state: &mut DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<V>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        // Loop
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-loop")
                .icon(Icon::new(IconName::Repeat))
                .ghost()
                .small()
                .when(state.is_looping, |b| b.primary())
                .tooltip("Loop")
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.is_looping = !state.is_looping;
                    window.refresh();
                })
        })
        .when(state.is_looping, |flex| {
            let loop_start = state.selection.loop_start.unwrap_or(0.0);
            let loop_end = state.selection.loop_end.unwrap_or(16.0);
            
            flex.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(cx.theme().accent.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().accent)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().accent_foreground)
                            .child(format!("{:.1} - {:.1}", loop_start, loop_end))
                    )
            )
        })
}

fn render_metronome_section<V: 'static>(state: &mut DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<V>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-metronome")
                .icon(Icon::new(IconName::Heart))
                .ghost()
                .small()
                .when(state.metronome_enabled, |b| b.primary())
                .tooltip("Metronome")
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.metronome_enabled = !state.metronome_enabled;
                    window.refresh();
                })
        })
        .child({
            let state_arc = state_arc.clone();
            Button::new("transport-countin")
                .icon(Icon::new(IconName::Clock))
                .ghost()
                .small()
                .when(state.count_in_enabled, |b| b.primary())
                .tooltip("Count-In")
                .on_click(move |_, window, _cx| {
                    let mut state = state_arc.write();
                    state.count_in_enabled = !state.count_in_enabled;
                    window.refresh();
                })
        })
}

// Event handlers

fn handle_play_pause(state: &mut DawUiState, window: &mut Window, cx: &mut Context<DawPanel>) {
    state.is_playing = !state.is_playing;

    if let Some(ref service) = state.audio_service {
        let service = service.clone();
        let playing = state.is_playing;

        cx.spawn(async move |_this, _cx| {
            if playing {
                let _ = service.play().await;
            } else {
                let _ = service.pause().await;
            }
        }).detach();
    }

    cx.notify();
}

fn handle_stop(state: &mut DawUiState, window: &mut Window, cx: &mut Context<DawPanel>) {
    state.is_playing = false;
    state.set_playhead(0.0);

    if let Some(ref service) = state.audio_service {
        let service = service.clone();

        cx.spawn(async move |_this, _cx| {
            let _ = service.stop().await;
        }).detach();
    }

    cx.notify();
}

fn handle_loop_toggle(state: &mut DawUiState, window: &mut Window, cx: &mut Context<DawPanel>) {
    state.is_looping = !state.is_looping;

    // Get loop points in samples
    let tempo = state.get_tempo();
    let loop_start_beats = state.selection.loop_start.unwrap_or(0.0);
    let loop_end_beats = state.selection.loop_end.unwrap_or(16.0);

    // Convert beats to samples
    let samples_per_beat = (SAMPLE_RATE * 60.0) / tempo;
    let loop_start_samples = (loop_start_beats * samples_per_beat as f64) as u64;
    let loop_end_samples = (loop_end_beats * samples_per_beat as f64) as u64;

    if let Some(ref service) = state.audio_service {
        let service = service.clone();
        let enabled = state.is_looping;

        cx.spawn(async move |_this, _cx| {
            let _ = service.set_loop(enabled, loop_start_samples, loop_end_samples).await;
        }).detach();
    }

    cx.notify();
}

fn handle_metronome_toggle(state: &mut DawUiState, window: &mut Window, cx: &mut Context<DawPanel>) {
    state.metronome_enabled = !state.metronome_enabled;

    if let Some(ref service) = state.audio_service {
        let service = service.clone();
        let enabled = state.metronome_enabled;

        cx.spawn(async move |_this, _cx| {
            let _ = service.set_metronome(enabled).await;
        }).detach();
    }

    cx.notify();
}

