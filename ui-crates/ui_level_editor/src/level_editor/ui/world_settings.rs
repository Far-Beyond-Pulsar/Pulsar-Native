use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, Sizable, StyledExt,
    IconName,
};
use std::sync::Arc;
use std::collections::HashSet;
use rust_i18n::t;

use super::state::LevelEditorState;
use crate::level_editor::WorldSettingsPanel;

/// World Settings Panel - Configure global world/scene settings
/// Styled to match the Properties (Details) panel
pub struct WorldSettings;

impl WorldSettings {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        _state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        collapsed_sections: &HashSet<String>,
        cx: &mut Context<WorldSettingsPanel>
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
                                    .child(Self::render_collapsible_section("Environment", IconName::Cloud, collapsed_sections.contains("Environment"), cx))
                                    .child(Self::render_collapsible_section("Global Illumination", IconName::Sun, collapsed_sections.contains("Global Illumination"), cx))
                                    .child(Self::render_collapsible_section("Fog & Atmosphere", IconName::Fog, collapsed_sections.contains("Fog & Atmosphere"), cx))
                                    .child(Self::render_collapsible_section("Physics", IconName::Activity, collapsed_sections.contains("Physics"), cx))
                                    .child(Self::render_collapsible_section("Audio", IconName::MusicNote, collapsed_sections.contains("Audio"), cx))
                            )
                    )
            )
    }

    fn render_header(&self, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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
                            .child(t!("LevelEditor.WorldSettings.Title").to_string())
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
                            .tooltip(t!("LevelEditor.WorldSettings.ResetDefaults"))
                    )
                    .child(
                        Button::new("more_options")
                            .icon(IconName::Ellipsis)
                            .ghost()
                            .xsmall()
                    )
            )
    }

    fn render_world_header(cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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
                                    .child(t!("LevelEditor.WorldSettings.UntitledScene").to_string())
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t!("LevelEditor.WorldSettings.LastSaved").to_string())
                            )
                    )
            )
    }

    fn render_collapsible_section(
        title: &str,
        icon: IconName,
        is_collapsed: bool,
        cx: &mut Context<WorldSettingsPanel>
    ) -> impl IntoElement {
        let section_name = title.to_string();
        let chevron_icon = if is_collapsed { IconName::ChevronRight } else { IconName::ChevronDown };
        let section_id = SharedString::from(format!("section-{}", title));
        
        // Translate the title for display
        let translated_title = match title {
            "Environment" => t!("LevelEditor.WorldSettings.Environment").to_string(),
            "Global Illumination" => t!("LevelEditor.WorldSettings.GlobalIllumination").to_string(),
            "Fog & Atmosphere" => t!("LevelEditor.WorldSettings.FogAtmosphere").to_string(),
            "Physics" => t!("LevelEditor.WorldSettings.Physics").to_string(),
            "Audio" => t!("LevelEditor.WorldSettings.Audio").to_string(),
            _ => title.to_string(),
        };
        
        v_flex()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                // Section header - clickable to toggle
                h_flex()
                    .id(section_id)
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .when(!is_collapsed, |this| this.border_b_1().border_color(cx.theme().border))
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar.opacity(0.8)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _event, _window, cx| {
                        this.toggle_section(section_name.clone(), cx);
                    }))
                    .child(
                        ui::Icon::new(chevron_icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        ui::Icon::new(icon)
                            .size(px(14.0))
                            .text_color(cx.theme().foreground)
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(translated_title)
                    )
            )
            .when(!is_collapsed, |this| {
                this.child(
                    // Section content - only shown when not collapsed
                    div()
                        .w_full()
                        .p_3()
                        .bg(cx.theme().background)
                        .child(Self::render_section_content(title, cx))
                )
            })
    }

    /// Renders the content for a specific section
    fn render_section_content(section_name: &str, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
        match section_name {
            "Environment" => v_flex()
                .gap_3()
                .child(Self::render_dropdown_row(&t!("LevelEditor.WorldSettings.Skybox").to_string(), &t!("LevelEditor.Common.DefaultSky").to_string(), cx))
                .child(Self::render_color_row(&t!("LevelEditor.WorldSettings.SkyColor").to_string(), Hsla { h: 210.0, s: 0.6, l: 0.7, a: 1.0 }, cx))
                .child(Self::render_color_row(&t!("LevelEditor.WorldSettings.HorizonColor").to_string(), Hsla { h: 30.0, s: 0.7, l: 0.8, a: 1.0 }, cx))
                .child(Self::render_color_row(&t!("LevelEditor.WorldSettings.GroundColor").to_string(), Hsla { h: 30.0, s: 0.3, l: 0.3, a: 1.0 }, cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.SkyIntensity").to_string(), "1.0", "", cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.EnableClouds").to_string(), true, cx))
                .into_any_element(),
            "Global Illumination" => v_flex()
                .gap_3()
                .child(Self::render_color_row(&t!("LevelEditor.WorldSettings.AmbientColor").to_string(), Hsla { h: 220.0, s: 0.2, l: 0.4, a: 1.0 }, cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.AmbientIntensity").to_string(), "0.3", "", cx))
                .child(Self::render_dropdown_row(&t!("LevelEditor.WorldSettings.GIMode").to_string(), &t!("LevelEditor.Common.Baked").to_string(), cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.BounceCount").to_string(), "2", "", cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.RealtimeGI").to_string(), false, cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.AmbientOcclusion").to_string(), true, cx))
                .into_any_element(),
            "Fog & Atmosphere" => v_flex()
                .gap_3()
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.EnableFog").to_string(), true, cx))
                .child(Self::render_dropdown_row(&t!("LevelEditor.WorldSettings.FogMode").to_string(), &t!("LevelEditor.Common.Exponential").to_string(), cx))
                .child(Self::render_color_row(&t!("LevelEditor.WorldSettings.FogColor").to_string(), Hsla { h: 210.0, s: 0.3, l: 0.7, a: 1.0 }, cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.FogDensity").to_string(), "0.02", "", cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.FogStart").to_string(), "10", "m", cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.FogEnd").to_string(), "500", "m", cx))
                .into_any_element(),
            "Physics" => v_flex()
                .gap_3()
                .child(Self::render_vector3_display(&t!("LevelEditor.WorldSettings.Gravity").to_string(), [0.0, -9.81, 0.0], cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.TimeScale").to_string(), "1.0", "x", cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.FixedTimestep").to_string(), "0.02", "s", cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.EnablePhysics").to_string(), true, cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.AutoSimulation").to_string(), true, cx))
                .into_any_element(),
            "Audio" => v_flex()
                .gap_3()
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.MasterVolume").to_string(), "1.0", "", cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.SpeedOfSound").to_string(), "343", "m/s", cx))
                .child(Self::render_property_row(&t!("LevelEditor.WorldSettings.DopplerFactor").to_string(), "1.0", "", cx))
                .child(Self::render_dropdown_row(&t!("LevelEditor.WorldSettings.ReverbPreset").to_string(), &t!("LevelEditor.Common.None").to_string(), cx))
                .child(Self::render_toggle_row(&t!("LevelEditor.WorldSettings.EnableSpatialAudio").to_string(), true, cx))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    // Helper rendering functions (matching Properties panel style)
    
    fn render_property_row(label: &str, value: &str, unit: &str, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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

    fn render_dropdown_row(label: &str, value: &str, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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

    fn render_color_row(label: &str, color: Hsla, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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

    fn render_toggle_row(label: &str, enabled: bool, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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

    fn render_vector3_display(label: &str, values: [f32; 3], cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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

    fn render_axis_display(axis: &str, axis_color: Hsla, value: f32, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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
