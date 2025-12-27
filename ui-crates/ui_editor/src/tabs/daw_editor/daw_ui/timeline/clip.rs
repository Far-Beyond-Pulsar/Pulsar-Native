use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

pub fn render_clip(
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
                        .child(super::waveform::render_waveform_placeholder(clip_color, cx))
                )
        )
}
