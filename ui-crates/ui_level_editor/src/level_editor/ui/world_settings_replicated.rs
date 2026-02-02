//! World Settings Panel - Fully Replicated Edition
//!
//! Complete replacement for world_settings.rs with multiuser replication support.
//! All fields are automatically synchronized across connected users.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, scroll::ScrollbarAxis,
    input::{InputState, NumberInput},
    ActiveTheme, Sizable, StyledExt, IconName, Icon, Disableable,
};
use ui::replication::{
    ReplicationMode, ReplicationRegistry,
    InputStateReplicationExt,
    PresenceStack, FieldPresenceIndicator, UserPresence,
};
use std::sync::Arc;
use std::collections::HashSet;
use parking_lot::RwLock;
use rust_i18n::t;

use super::state::LevelEditorState;
use crate::level_editor::WorldSettingsPanel;
use crate::level_editor::world_settings_data::WorldSettingsData;

/// World Settings Panel with full multiuser replication
pub struct WorldSettingsReplicated {
    /// Shared settings data
    pub settings: Arc<RwLock<WorldSettingsData>>,

    /// Input entities for float fields
    sky_intensity_input: Entity<InputState>,
    ambient_intensity_input: Entity<InputState>,
    fog_density_input: Entity<InputState>,
    fog_start_input: Entity<InputState>,
    fog_end_input: Entity<InputState>,
    time_scale_input: Entity<InputState>,
    fixed_timestep_input: Entity<InputState>,
    master_volume_input: Entity<InputState>,
    speed_of_sound_input: Entity<InputState>,
    doppler_factor_input: Entity<InputState>,
    input_subscriptions: Vec<Subscription>,
}

impl WorldSettingsReplicated {
    pub fn new(window: &mut Window, cx: &mut Context<WorldSettingsPanel>) -> Self {
        let settings = Arc::new(RwLock::new(WorldSettingsData::default()));

        // Initialize UI replication system if not already done
        // SessionContext is always available via global
        ui::replication::init(cx);

        // Create input entities for all numeric fields
        let (sky_intensity_input, sky_intensity_sub) = Self::create_f32_input(
            "world_sky_intensity",
            settings.clone(),
            |s| s.sky_intensity,
            |s, v| s.sky_intensity = v,
            window,
            cx,
        );

        let (ambient_intensity_input, ambient_intensity_sub) = Self::create_f32_input(
            "world_ambient_intensity",
            settings.clone(),
            |s| s.ambient_intensity,
            |s, v| s.ambient_intensity = v,
            window,
            cx,
        );

        let (fog_density_input, fog_density_sub) = Self::create_f32_input(
            "world_fog_density",
            settings.clone(),
            |s| s.fog_density,
            |s, v| s.fog_density = v,
            window,
            cx,
        );

        let (fog_start_input, fog_start_sub) = Self::create_f32_input(
            "world_fog_start",
            settings.clone(),
            |s| s.fog_start,
            |s, v| s.fog_start = v,
            window,
            cx,
        );

        let (fog_end_input, fog_end_sub) = Self::create_f32_input(
            "world_fog_end",
            settings.clone(),
            |s| s.fog_end,
            |s, v| s.fog_end = v,
            window,
            cx,
        );

        let (time_scale_input, time_scale_sub) = Self::create_f32_input(
            "world_time_scale",
            settings.clone(),
            |s| s.time_scale,
            |s, v| s.time_scale = v,
            window,
            cx,
        );

        let (fixed_timestep_input, fixed_timestep_sub) = Self::create_f32_input(
            "world_fixed_timestep",
            settings.clone(),
            |s| s.fixed_timestep,
            |s, v| s.fixed_timestep = v,
            window,
            cx,
        );

        let (master_volume_input, master_volume_sub) = Self::create_f32_input(
            "world_master_volume",
            settings.clone(),
            |s| s.master_volume,
            |s, v| s.master_volume = v,
            window,
            cx,
        );

        let (speed_of_sound_input, speed_of_sound_sub) = Self::create_f32_input(
            "world_speed_of_sound",
            settings.clone(),
            |s| s.speed_of_sound,
            |s, v| s.speed_of_sound = v,
            window,
            cx,
        );

        let (doppler_factor_input, doppler_factor_sub) = Self::create_f32_input(
            "world_doppler_factor",
            settings.clone(),
            |s| s.doppler_factor,
            |s, v| s.doppler_factor = v,
            window,
            cx,
        );

        Self {
            settings,
            sky_intensity_input,
            ambient_intensity_input,
            fog_density_input,
            fog_start_input,
            fog_end_input,
            time_scale_input,
            fixed_timestep_input,
            master_volume_input,
            speed_of_sound_input,
            doppler_factor_input,
            input_subscriptions: vec![
                sky_intensity_sub,
                ambient_intensity_sub,
                fog_density_sub,
                fog_start_sub,
                fog_end_sub,
                time_scale_sub,
                fixed_timestep_sub,
                master_volume_sub,
                speed_of_sound_sub,
                doppler_factor_sub,
            ],
        }
    }

    /// Helper to create a replicated f32 input field
    fn create_f32_input(
        element_id: &str,
        settings: Arc<RwLock<WorldSettingsData>>,
        getter: fn(&WorldSettingsData) -> f32,
        setter: fn(&mut WorldSettingsData, f32),
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> (Entity<InputState>, Subscription) {
        let input = cx.new(|cx| InputState::new(window, cx));

        // Initialize value
        let value = getter(&settings.read());
        input.update(cx, |state, cx| {
            state.set_value(format!("{:.3}", value), window, cx);
        });

        // Enable replication (RequestEdit mode for world settings)
        input.enable_replication(ReplicationMode::RequestEdit, cx);

        // Subscribe to changes
        let settings_clone = settings.clone();
        let input_clone = input.clone();
        let field_sub = cx.subscribe_in(&input, window, move |_panel, _state, event, _window, cx| {
            tracing::info!("WorldSettingsReplicated: Input event {:?}", event);
            if matches!(event, ui::input::InputEvent::Change | ui::input::InputEvent::Blur) {
                input_clone.update(cx, |state, _cx| {
                    let text = state.text().to_string();
                    if let Ok(value) = text.parse::<f32>() {
                        let mut settings = settings_clone.write();
                        setter(&mut settings, value);
                        settings.apply();
                    }
                });

                // Sync to network
                input_clone.sync_if_replicated(cx);
            }
        });

        (input, field_sub)
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
            .child(self.render_header(cx))
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
                                    .child(self.render_world_header(cx))
                                    .child(self.render_section("Environment", IconName::Cloud, collapsed_sections.contains("Environment"), cx))
                                    .child(self.render_section("Global Illumination", IconName::Sun, collapsed_sections.contains("Global Illumination"), cx))
                                    .child(self.render_section("Fog & Atmosphere", IconName::Fog, collapsed_sections.contains("Fog & Atmosphere"), cx))
                                    .child(self.render_section("Physics", IconName::Activity, collapsed_sections.contains("Physics"), cx))
                                    .child(self.render_section("Audio", IconName::MusicNote, collapsed_sections.contains("Audio"), cx))
                            )
                    )
            )
    }

    fn render_header(&self, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
        // Get active editors for presence display
        let registry = ReplicationRegistry::global(cx);
        let all_editors: Vec<UserPresence> = [
            "world_sky_intensity", "world_ambient_intensity", "world_fog_density",
            "world_time_scale", "world_master_volume"
        ]
        .iter()
        .filter_map(|id| registry.get_element_state(id))
        .flat_map(|state| state.active_editors.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter_map(|peer_id| registry.get_user_presence(&peer_id))
        .collect();

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
                    // Show who's currently editing
                    .when(!all_editors.is_empty(), |flex| {
                        flex.child(
                            PresenceStack::new(all_editors)
                                .max_visible(3)
                                .small()
                        )
                    })
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
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                *this.world_settings.settings.write() = WorldSettingsData::default();
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("more_options")
                            .icon(IconName::Ellipsis)
                            .ghost()
                            .xsmall()
                    )
            )
    }

    fn render_world_header(&self, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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
                                Icon::new(IconName::Globe)
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

    fn render_section(
        &self,
        title: &str,
        icon: IconName,
        is_collapsed: bool,
        cx: &mut Context<WorldSettingsPanel>
    ) -> impl IntoElement {
        let section_name = title.to_string();
        let chevron_icon = if is_collapsed { IconName::ChevronRight } else { IconName::ChevronDown };
        let section_id = SharedString::from(format!("section-{}", title));

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
                    .child(Icon::new(chevron_icon).size(px(14.0)).text_color(cx.theme().foreground))
                    .child(Icon::new(icon).size(px(14.0)).text_color(cx.theme().foreground))
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
                    div()
                        .w_full()
                        .p_3()
                        .bg(cx.theme().background)
                        .child(self.render_section_content(title, cx))
                )
            })
    }

    fn render_section_content(&self, section_name: &str, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
        match section_name {
            "Environment" => self.render_environment_section(cx),
            "Global Illumination" => self.render_gi_section(cx),
            "Fog & Atmosphere" => self.render_fog_section(cx),
            "Physics" => self.render_physics_section(cx),
            "Audio" => self.render_audio_section(cx),
            _ => div().into_any_element(),
        }
    }

    fn render_environment_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.sky_intensity_input,
                "world_sky_intensity",
                &t!("LevelEditor.WorldSettings.SkyIntensity").to_string(),
                "",
                cx
            ))
            .child(self.render_bool_field(
                "world_enable_clouds",
                &t!("LevelEditor.WorldSettings.EnableClouds").to_string(),
                settings.enable_clouds,
                |s, v| s.enable_clouds = v,
                cx
            ))
            .into_any_element()
    }

    fn render_gi_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.ambient_intensity_input,
                "world_ambient_intensity",
                &t!("LevelEditor.WorldSettings.AmbientIntensity").to_string(),
                "",
                cx
            ))
            .into_any_element()
    }

    fn render_fog_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_bool_field(
                "world_enable_fog",
                &t!("LevelEditor.WorldSettings.EnableFog").to_string(),
                settings.enable_fog,
                |s, v| s.enable_fog = v,
                cx
            ))
            .child(self.render_f32_field(
                &self.fog_density_input,
                "world_fog_density",
                &t!("LevelEditor.WorldSettings.FogDensity").to_string(),
                "",
                cx
            ))
            .child(self.render_f32_field(
                &self.fog_start_input,
                "world_fog_start",
                &t!("LevelEditor.WorldSettings.FogStart").to_string(),
                "m",
                cx
            ))
            .child(self.render_f32_field(
                &self.fog_end_input,
                "world_fog_end",
                &t!("LevelEditor.WorldSettings.FogEnd").to_string(),
                "m",
                cx
            ))
            .into_any_element()
    }

    fn render_physics_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_vector3_display(
                &t!("LevelEditor.WorldSettings.Gravity").to_string(),
                settings.gravity,
                cx
            ))
            .child(self.render_f32_field(
                &self.time_scale_input,
                "world_time_scale",
                &t!("LevelEditor.WorldSettings.TimeScale").to_string(),
                "x",
                cx
            ))
            .child(self.render_f32_field(
                &self.fixed_timestep_input,
                "world_fixed_timestep",
                &t!("LevelEditor.WorldSettings.FixedTimestep").to_string(),
                "s",
                cx
            ))
            .child(self.render_bool_field(
                "world_enable_physics",
                &t!("LevelEditor.WorldSettings.EnablePhysics").to_string(),
                settings.enable_physics,
                |s, v| s.enable_physics = v,
                cx
            ))
            .child(self.render_bool_field(
                "world_auto_simulation",
                &t!("LevelEditor.WorldSettings.AutoSimulation").to_string(),
                settings.auto_simulation,
                |s, v| s.auto_simulation = v,
                cx
            ))
            .into_any_element()
    }

    fn render_audio_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.master_volume_input,
                "world_master_volume",
                &t!("LevelEditor.WorldSettings.MasterVolume").to_string(),
                "",
                cx
            ))
            .child(self.render_f32_field(
                &self.speed_of_sound_input,
                "world_speed_of_sound",
                &t!("LevelEditor.WorldSettings.SpeedOfSound").to_string(),
                "m/s",
                cx
            ))
            .child(self.render_f32_field(
                &self.doppler_factor_input,
                "world_doppler_factor",
                &t!("LevelEditor.WorldSettings.DopplerFactor").to_string(),
                "",
                cx
            ))
            .child(self.render_bool_field(
                "world_enable_spatial_audio",
                &t!("LevelEditor.WorldSettings.EnableSpatialAudio").to_string(),
                settings.enable_spatial_audio,
                |s, v| s.enable_spatial_audio = v,
                cx
            ))
            .into_any_element()
    }

    // Field rendering helpers

    fn render_f32_field(
        &self,
        input: &Entity<InputState>,
        element_id: &str,
        label: &str,
        unit: &str,
        cx: &Context<WorldSettingsPanel>
    ) -> impl IntoElement {
        let can_edit = input.can_edit_replicated(cx);

        // Get presence info
        let registry = ReplicationRegistry::global(cx);
        let locked_by = registry
            .get_element_state(element_id)
            .and_then(|state| state.locked_by.clone())
            .and_then(|peer_id| registry.get_user_presence(&peer_id));

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
                        NumberInput::new(input)
                            .xsmall()
                            .when(!can_edit, |this| this.disabled(true))
                    )
                    .when(!unit.is_empty(), |flex| {
                        flex.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(unit.to_string())
                        )
                    })
                    .when_some(locked_by, |flex, user| {
                        flex.child(FieldPresenceIndicator::new(user).locked(true))
                    })
            )
    }

    fn render_bool_field(
        &self,
        element_id: &str,
        label: &str,
        value: bool,
        setter: fn(&mut WorldSettingsData, bool),
        cx: &Context<WorldSettingsPanel>
    ) -> impl IntoElement {
        let settings = self.settings.clone();

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
                    .bg(if value { cx.theme().accent } else { cx.theme().muted })
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _event, _window, cx| {
                        let mut s = settings.write();
                        setter(&mut s, !value);
                        s.apply();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size_4()
                            .mt(px(2.0))
                            .ml(if value { px(18.0) } else { px(2.0) })
                            .rounded_full()
                            .bg(white())
                            .shadow_sm()
                    )
            )
    }

    fn render_vector3_display(&self, label: &str, values: [f32; 3], cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
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
                    .child(format!("{:.2}", value))
            )
    }
}
