use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, divider::Divider, ActiveTheme as _,
};
use super::{types::ProjectSettings, helpers::render_info_section};
use crate::entry_screen::EntryScreen;

#[derive(Clone, Debug)]
struct ProjectConfig {
    name: String,
    version: String,
    engine_version: String,
    default_scene: String,
}

impl ProjectConfig {
    fn from_toml(content: &str) -> Result<Self, String> {
        let parsed = toml::from_str::<toml::Value>(content)
            .map_err(|e| format!("Failed to parse TOML: {}", e))?;
        
        let project_table = parsed.get("project")
            .and_then(|v| v.as_table())
            .ok_or("Missing [project] section")?;
        
        let settings_table = parsed.get("settings")
            .and_then(|v| v.as_table());
        
        Ok(Self {
            name: project_table.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            version: project_table.get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.1.0")
                .to_string(),
            engine_version: project_table.get("engine_version")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            default_scene: settings_table
                .and_then(|t| t.get("default_scene"))
                .and_then(|v| v.as_str())
                .unwrap_or("scenes/main.scene")
                .to_string(),
        })
    }
    
    fn to_toml(&self) -> String {
        format!(
            r#"[project]
name = "{}"
version = "{}"
engine_version = "{}"

[settings]
default_scene = "{}"

[build]
# Add build configuration here

[dependencies]
# Add project dependencies here
"#,
            self.name,
            self.version,
            self.engine_version,
            self.default_scene
        )
    }
}

pub fn render_metadata_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let config_path = settings.project_path.join("Pulsar.toml");
    let has_config = config_path.exists();
    
    // Try to read and parse the config file
    let (config_result, config_content) = if has_config {
        let content = std::fs::read_to_string(&config_path).ok();
        let config = content.as_ref()
            .and_then(|c| ProjectConfig::from_toml(c).ok());
        (config, content)
    } else {
        (None, None)
    };
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Project Metadata & Configuration")
        )
        .child(Divider::horizontal())
        .child(
            div()
                .p_4()
                .rounded_lg()
                .bg(if has_config { theme.accent.opacity(0.1) } else { theme.danger.opacity(0.1) })
                .border_1()
                .border_color(if has_config { theme.accent.opacity(0.3) } else { theme.danger.opacity(0.3) })
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            Icon::new(if has_config { IconName::Folder } else { IconName::WarningTriangle })
                                .size(px(24.))
                                .text_color(if has_config { theme.accent } else { theme.danger })
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.foreground)
                                        .child(if has_config { "Configuration File Found" } else { "Configuration File Missing" })
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.foreground.opacity(0.7))
                                        .child(config_path.to_string_lossy().to_string())
                                )
                        )
                )
        )
        .when_some(config_result.clone(), |this, config| {
            this.child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .bg(theme.sidebar.opacity(0.5))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .mb_2()
                            .child("Project Configuration")
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(render_config_field("Project Name", &config.name, theme))
                            .child(render_config_field("Version", &config.version, theme))
                            .child(render_config_field("Engine Version", &config.engine_version, theme))
                            .child(render_config_field("Default Scene", &config.default_scene, theme))
                    )
            )
        })
        .when(config_result.is_none() && has_config, |this| {
            this.child(
                div()
                    .p_4()
                    .rounded_lg()
                    .bg(theme.warning.opacity(0.1))
                    .border_1()
                    .border_color(theme.warning.opacity(0.3))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::WarningTriangle)
                                    .size(px(20.))
                                    .text_color(theme.warning)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child("Failed to parse Pulsar.toml. Please check the file format.")
                            )
                    )
            )
        })
        .child(render_info_section("Project Structure", vec![
            ("Project Root", settings.project_path.to_string_lossy().to_string()),
            ("Assets Folder", settings.project_path.join("assets").to_string_lossy().to_string()),
            ("Scenes Folder", settings.project_path.join("scenes").to_string_lossy().to_string()),
            ("Scripts Folder", settings.project_path.join("scripts").to_string_lossy().to_string()),
        ], &theme))
        .child(
            div()
                .p_4()
                .rounded_lg()
                .bg(theme.accent.opacity(0.1))
                .border_1()
                .border_color(theme.accent.opacity(0.3))
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.accent)
                                .child("📋 Pulsar.toml Format")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground.opacity(0.7))
                                .child("• [project] - name, version, engine_version")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground.opacity(0.7))
                                .child("• [settings] - default_scene, window settings")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground.opacity(0.7))
                                .child("• [build] - target platforms, optimization")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground.opacity(0.7))
                                .child("• [dependencies] - project dependencies")
                        )
                )
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Button::new("edit-config")
                                .label("Edit in External Editor")
                                .icon(IconName::Code)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_click({
                                    let path = config_path.clone();
                                    move |_, _, _| {
                                        // Try to open with system default editor
                                        #[cfg(windows)]
                                        {
                                            use std::os::windows::process::CommandExt;
                                            const DETACHED_PROCESS: u32 = 0x00000008;
                                            const CREATE_NO_WINDOW: u32 = 0x08000000;
                                            let _ = std::process::Command::new("cmd")
                                                .args(&["/c", "start", "", path.to_str().unwrap_or("")])
                                                .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
                                                .spawn();
                                        }
                                        #[cfg(not(windows))]
                                        {
                                            let _ = open::that(&path);
                                        }
                                    }
                                })
                        )
                        .child(
                            Button::new("create-template")
                                .label("Create Template Config")
                                .icon(IconName::Plus)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = config_path.clone();
                                    move |_, _, _| {
                                        if !path.exists() {
                                            let template = ProjectConfig {
                                                name: path.parent()
                                                    .and_then(|p| p.file_name())
                                                    .and_then(|n| n.to_str())
                                                    .unwrap_or("MyProject")
                                                    .to_string(),
                                                version: "0.1.0".to_string(),
                                                engine_version: "0.1.23".to_string(),
                                                default_scene: "scenes/main.scene".to_string(),
                                            };
                                            let _ = std::fs::write(&path, template.to_toml());
                                        }
                                    }
                                })
                        )
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Button::new("validate-config")
                                .label("Validate Project")
                                .icon(IconName::Activity)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        // Validate project structure
                                        let required_dirs = ["assets", "scenes", "scripts"];
                                        let mut missing = Vec::new();
                                        
                                        for dir in required_dirs {
                                            let dir_path = path.join(dir);
                                            if !dir_path.exists() {
                                                missing.push(dir);
                                            }
                                        }
                                        
                                        if missing.is_empty() {
                                            tracing::debug!("✓ Project structure is valid");
                                        } else {
                                            tracing::debug!("⚠ Missing directories: {}", missing.join(", "));
                                        }
                                    }
                                })
                        )
                        .child(
                            Button::new("create-missing")
                                .label("Create Missing Folders")
                                .icon(IconName::FolderOpen)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        let dirs = ["assets", "scenes", "scripts", "prefabs"];
                                        for dir in dirs {
                                            let _ = std::fs::create_dir_all(path.join(dir));
                                        }
                                        tracing::debug!("✓ Created project folders");
                                    }
                                })
                        )
                )
        )
}

fn render_config_field(label: &str, value: &str, theme: &ui::theme::Theme) -> impl IntoElement {
    h_flex()
        .items_center()
        .justify_between()
        .px_3()
        .py_2()
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .rounded_md()
        .bg(theme.background.opacity(0.5))
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.foreground.opacity(0.7))
                .child(label.to_string())
        )
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(value.to_string())
        )
}
