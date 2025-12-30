/// Inspector Panel Component
/// Right sidebar with track properties, clip editor, automation, and effects

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme,
    Selectable, divider::Divider,
};

const INSPECTOR_WIDTH: f32 = 300.0;

pub fn render_inspector(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    v_flex()
        .w(px(INSPECTOR_WIDTH))
        .h_full()
        .bg(cx.theme().muted.opacity(0.3))
        .border_l_1()
        .border_color(cx.theme().border)
        // Tab bar
        .child(render_inspector_tabs(state, cx))
        // Content
        .child(render_inspector_content(state, cx))
}

fn render_inspector_tabs(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(40.0))
        .px_2()
        .gap_1()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(render_inspector_tab("Track", InspectorTab::Track, state, cx))
        .child(render_inspector_tab("Clip", InspectorTab::Clip, state, cx))
        .child(render_inspector_tab("Auto", InspectorTab::Automation, state, cx))
        .child(render_inspector_tab("FX", InspectorTab::Effects, state, cx))
}

fn render_inspector_tab(
    label: &'static str,
    tab: InspectorTab,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    Button::new(ElementId::Name(format!("inspector-tab-{:?}", tab).into()))
        .label(label)
        .ghost()
        .compact()
        .small()
        
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.state.inspector_tab = tab;
            cx.notify();
        }))
}

fn render_inspector_content(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    div()
        .flex_1()
        .overflow_hidden()
        .child(match state.inspector_tab {
            InspectorTab::Track => render_track_inspector(state, cx).into_any_element(),
            InspectorTab::Clip => render_clip_inspector(state, cx).into_any_element(),
            InspectorTab::Automation => render_automation_inspector(state, cx).into_any_element(),
            InspectorTab::Effects => render_effects_inspector(state, cx).into_any_element(),
        })
}

fn render_track_inspector(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let selected_track = state.selection.selected_track_ids.iter().next()
        .and_then(|id| state.get_track(*id));
    
    if let Some(track) = selected_track {
        v_flex()
            .w_full()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child("Track Properties")
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(&track.name)
                    )
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Volume: {:.1} dB", track.volume_db()))
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Pan: {:.0}%", track.pan * 100.0))
                    )
            )
    } else {
        render_empty_inspector("No Track Selected", cx)
    }
}

fn render_clip_inspector(_state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_inspector("No Clip Selected", cx)
}

fn render_automation_inspector(_state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_inspector("Automation", cx)
}

fn render_effects_inspector(_state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_inspector("No Effects", cx)
}

fn render_empty_inspector(message: &'static str, cx: &mut Context<DawPanel>) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(message)
        )
}
