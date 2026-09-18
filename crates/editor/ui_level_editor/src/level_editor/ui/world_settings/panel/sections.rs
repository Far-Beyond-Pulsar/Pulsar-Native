use super::*;

impl WorldSettingsPanelImpl {

    pub(super) fn render_environment_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.sky_intensity_input,
                "world_sky_intensity",
                t!("LevelEditor.WorldSettings.SkyIntensity").as_ref(),
                "",
                cx,
            ))
            .child(self.render_bool_field(
                "world_enable_clouds",
                t!("LevelEditor.WorldSettings.EnableClouds").as_ref(),
                settings.enable_clouds,
                |s, v| s.enable_clouds = v,
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_gi_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.ambient_intensity_input,
                "world_ambient_intensity",
                t!("LevelEditor.WorldSettings.AmbientIntensity").as_ref(),
                "",
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_fog_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_bool_field(
                "world_enable_fog",
                t!("LevelEditor.WorldSettings.EnableFog").as_ref(),
                settings.enable_fog,
                |s, v| s.enable_fog = v,
                cx,
            ))
            .child(self.render_f32_field(
                &self.fog_density_input,
                "world_fog_density",
                t!("LevelEditor.WorldSettings.FogDensity").as_ref(),
                "",
                cx,
            ))
            .child(self.render_f32_field(
                &self.fog_start_input,
                "world_fog_start",
                t!("LevelEditor.WorldSettings.FogStart").as_ref(),
                "m",
                cx,
            ))
            .child(self.render_f32_field(
                &self.fog_end_input,
                "world_fog_end",
                t!("LevelEditor.WorldSettings.FogEnd").as_ref(),
                "m",
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_physics_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_vector3_display(
                t!("LevelEditor.WorldSettings.Gravity").as_ref(),
                settings.gravity,
                cx,
            ))
            .child(self.render_f32_field(
                &self.time_scale_input,
                "world_time_scale",
                t!("LevelEditor.WorldSettings.TimeScale").as_ref(),
                "x",
                cx,
            ))
            .child(self.render_f32_field(
                &self.fixed_timestep_input,
                "world_fixed_timestep",
                t!("LevelEditor.WorldSettings.FixedTimestep").as_ref(),
                "s",
                cx,
            ))
            .child(self.render_bool_field(
                "world_enable_physics",
                t!("LevelEditor.WorldSettings.EnablePhysics").as_ref(),
                settings.enable_physics,
                |s, v| s.enable_physics = v,
                cx,
            ))
            .child(self.render_bool_field(
                "world_auto_simulation",
                t!("LevelEditor.WorldSettings.AutoSimulation").as_ref(),
                settings.auto_simulation,
                |s, v| s.auto_simulation = v,
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_audio_section(&self, cx: &Context<WorldSettingsPanel>) -> AnyElement {
        let settings = self.settings.read();

        v_flex()
            .gap_3()
            .child(self.render_f32_field(
                &self.master_volume_input,
                "world_master_volume",
                t!("LevelEditor.WorldSettings.MasterVolume").as_ref(),
                "",
                cx,
            ))
            .child(self.render_f32_field(
                &self.speed_of_sound_input,
                "world_speed_of_sound",
                t!("LevelEditor.WorldSettings.SpeedOfSound").as_ref(),
                "m/s",
                cx,
            ))
            .child(self.render_f32_field(
                &self.doppler_factor_input,
                "world_doppler_factor",
                t!("LevelEditor.WorldSettings.DopplerFactor").as_ref(),
                "",
                cx,
            ))
            .child(self.render_bool_field(
                "world_enable_spatial_audio",
                t!("LevelEditor.WorldSettings.EnableSpatialAudio").as_ref(),
                settings.enable_spatial_audio,
                |s, v| s.enable_spatial_audio = v,
                cx,
            ))
            .into_any_element()
    }
}
