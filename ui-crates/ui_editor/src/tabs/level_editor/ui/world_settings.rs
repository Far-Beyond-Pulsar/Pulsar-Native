use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Sizable, StyledExt,
    IconName,
};
use std::sync::Arc;

use super::state::LevelEditorState;

/// World Settings Panel - Configure global world/scene settings
/// Styled to match the Properties (Details) panel
pub struct WorldSettings;

impl WorldSettings {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut App
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header
            .child(self.render_header(cx))
            // Main content area
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .size_full()
                            .scrollable(ScrollbarAxis::Vertical)
                            .child(
                                v_flex()
                                    .w_full()
                                    .p_3()
                                    .gap_4()
                                    .child(Self::render_world_header(cx))
                                    .child(Self::render_environment_section(cx))
                                    .child(Self::render_lighting_section(cx))
                                    .child(Self::render_fog_section(cx))
                                    .child(Self::render_physics_section(cx))
                                    .child(Self::render_audio_section(cx))
                            )
                    )
            )
    }

    fn render_header(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_4()
            .py_3()
            .justify_between()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("World Settings")
                    )
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("reset_world")
                            .icon(IconName::Refresh)
                            .ghost()
                            .xsmall()
                            .tooltip("Reset to Defaults")
                    )
                    .child(
                        Button::new("more_options")
                            .icon(IconName::Ellipsis)
                            .ghost()
                            .xsmall()
                    )
            )
    }

    fn render_world_header(cx: &App) -> impl IntoElement {
        v_flex()
            .w_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .size_10()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .bg(cx.theme().accent.opacity(0.15))
                            .child(
                                ui::Icon::new(IconName::Globe)
                                    .size(px(20.0))
                                    .text_color(cx.theme().accent)
                            )
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child("Untitled Scene")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Last saved: Never")
                            )
                    )
            )
    }

    fn render_collapsible_section(
        title: &str,
        icon: IconName,
        content: impl IntoElement,
        cx: &App
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Section header
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar.opacity(0.8)))
                    .child(
                        ui::Icon::new(IconName::ChevronDown)
                            .size(px(14.0))
                            .text_color(cx.theme().muted_foreground)
                    )
                    .child(
                        ui::Icon::new(icon)
                            .size(px(14.0))
                            .text_color(cx.theme().accent)
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(title.to_string())
                    )
            )
            .child(
                // Section content
                div()
                    .w_full()
                    .p_3()
                    .bg(cx.theme().background)
                    .child(content)
            )
    }

    fn render_environment_section(cx: &App) -> impl IntoElement {
        Self::render_collapsible_section(
            "Environment",
            IconName::Cloud,
            v_flex()
                .gap_3()
                .child(Self::render_dropdown_row("Skybox", "Default Sky", cx))
                .child(Self::render_color_row("Sky Color", Hsla { h: 210.0, s: 0.6, l: 0.7, a: 1.0 }, cx))
                .child(Self::render_color_row("Horizon Color", Hsla { h: 30.0, s: 0.7, l: 0.8, a: 1.0 }, cx))
                .child(Self::render_color_row("Ground Color", Hsla { h: 30.0, s: 0.3, l: 0.3, a: 1.0 }, cx))
                .child(Self::render_property_row("Sky Intensity", "1.0", "", cx))
                .child(Self::render_toggle_row("Enable Clouds", true, cx)),
            cx
        )
    }

    fn render_lighting_section(cx: &App) -> impl IntoElement {
        Self::render_collapsible_section(
            "Global Illumination",
            IconName::Sun,
            v_flex()
                .gap_3()
                .child(Self::render_color_row("Ambient Color", Hsla { h: 220.0, s: 0.2, l: 0.4, a: 1.0 }, cx))
                .child(Self::render_property_row("Ambient Intensity", "0.3", "", cx))
                .child(Self::render_dropdown_row("GI Mode", "Baked", cx))
                .child(Self::render_property_row("Bounce Count", "2", "", cx))
                .child(Self::render_toggle_row("Realtime GI", false, cx))
                .child(Self::render_toggle_row("Ambient Occlusion", true, cx)),
            cx
        )
    }

    fn render_fog_section(cx: &App) -> impl IntoElement {
        Self::render_collapsible_section(
            "Fog & Atmosphere",
            IconName::Fog,
            v_flex()
                .gap_3()
                .child(Self::render_toggle_row("Enable Fog", true, cx))
                .child(Self::render_dropdown_row("Fog Mode", "Exponential", cx))
                .child(Self::render_color_row("Fog Color", Hsla { h: 210.0, s: 0.3, l: 0.7, a: 1.0 }, cx))
                .child(Self::render_property_row("Fog Density", "0.02", "", cx))
                .child(Self::render_property_row("Fog Start", "10", "m", cx))
                .child(Self::render_property_row("Fog End", "500", "m", cx)),
            cx
        )
    }

    fn render_physics_section(cx: &App) -> impl IntoElement {
        Self::render_collapsible_section(
            "Physics",
            IconName::Activity,
            v_flex()
                .gap_3()
                .child(Self::render_vector3_display("Gravity", [0.0, -9.81, 0.0], cx))
                .child(Self::render_property_row("Time Scale", "1.0", "x", cx))
                .child(Self::render_property_row("Fixed Timestep", "0.02", "s", cx))
                .child(Self::render_toggle_row("Enable Physics", true, cx))
                .child(Self::render_toggle_row("Auto Simulation", true, cx)),
            cx
        )
    }

    fn render_audio_section(cx: &App) -> impl IntoElement {
        Self::render_collapsible_section(
            "Audio",
            IconName::MusicNote,
            v_flex()
                .gap_3()
                .child(Self::render_property_row("Master Volume", "1.0", "", cx))
                .child(Self::render_property_row("Speed of Sound", "343", "m/s", cx))
                .child(Self::render_property_row("Doppler Factor", "1.0", "", cx))
                .child(Self::render_dropdown_row("Reverb Preset", "None", cx))
                .child(Self::render_toggle_row("Enable Spatial Audio", true, cx)),
            cx
        )
    }

    // Helper rendering functions (matching Properties panel style)
    
    fn render_property_row(label: &str, value: &str, unit: &str, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .bg(cx.theme().input)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(4.0))
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .cursor_pointer()
                            .hover(|s| s.border_color(cx.theme().accent.opacity(0.5)))
                            .child(value.to_string())
                    )
                    .when(!unit.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(unit.to_string())
                        )
                    })
            )
    }

    fn render_dropdown_row(label: &str, value: &str, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .justify_between()
                    .bg(cx.theme().input)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.border_color(cx.theme().accent.opacity(0.5)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(value.to_string())
                    )
                    .child(
                        ui::Icon::new(IconName::ChevronDown)
                            .size(px(14.0))
                            .text_color(cx.theme().muted_foreground)
                    )
            )
    }

    fn render_color_row(label: &str, color: Hsla, cx: &App) -> impl IntoElement {
        // Convert HSLA to approximate RGB hex for display
        let rgb = hsla_to_rgb_approx(color);
        
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .size_7()
                            .bg(color)
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(cx.theme().border)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .bg(cx.theme().input)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(4.0))
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2))
                    )
            )
    }

    fn render_toggle_row(label: &str, enabled: bool, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                div()
                    .w_9()
                    .h_5()
                    .rounded_full()
                    .bg(if enabled { cx.theme().accent } else { cx.theme().muted })
                    .cursor_pointer()
                    .child(
                        div()
                            .size_4()
                            .mt(px(2.0))
                            .ml(if enabled { px(18.0) } else { px(2.0) })
                            .rounded_full()
                            .bg(white())
                            .shadow_sm()
                    )
            )
    }

    fn render_vector3_display(label: &str, values: [f32; 3], cx: &App) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(Self::render_axis_display("X", Hsla { h: 0.0, s: 0.8, l: 0.5, a: 1.0 }, values[0], cx))
                    .child(Self::render_axis_display("Y", Hsla { h: 50.0, s: 0.9, l: 0.5, a: 1.0 }, values[1], cx))
                    .child(Self::render_axis_display("Z", Hsla { h: 220.0, s: 0.8, l: 0.55, a: 1.0 }, values[2], cx))
            )
    }

    fn render_axis_display(axis: &str, axis_color: Hsla, value: f32, cx: &App) -> impl IntoElement {
        h_flex()
            .flex_1()
            .h_7()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Axis label with color indicator
                div()
                    .w_6()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(axis_color.opacity(0.2))
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(axis_color)
                            .child(axis.to_string())
                    )
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_2()
                    .bg(cx.theme().input)
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
                    .child(format!("{:.2}", value))
            )
    }
}

/// Helper function to convert HSLA to approximate RGB values
fn hsla_to_rgb_approx(hsla: Hsla) -> (u8, u8, u8) {
    let h = hsla.h / 360.0;
    let s = hsla.s;
    let l = hsla.l;
    
    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }
    
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    
    let r = hue_to_rgb(p, q, h + 1.0/3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0/3.0);
    
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 { t += 1.0; }
    if t > 1.0 { t -= 1.0; }
    if t < 1.0/6.0 { return p + (q - p) * 6.0 * t; }
    if t < 1.0/2.0 { return q; }
    if t < 2.0/3.0 { return p + (q - p) * (2.0/3.0 - t) * 6.0; }
    p
}
