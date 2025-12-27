use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, Icon, IconName, divider::Divider, ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    indicator::Indicator,
    Sizable,
};
use super::types::{ProjectSettings, ToolInfo, AvailableTools};
use crate::entry_screen::EntryScreen;

pub fn render_integrations_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let project_path = settings.project_path.clone();
    let is_updating = settings.is_updating_tools;

    // Try to get tools from cache, or load from disk, or create empty
    let tools = if let Some(ref cached) = settings.available_tools_cache {
        cached.clone()
    } else {
        // Try loading from cache file
        if let Some(cached_tools) = AvailableTools::load_from_cache() {
            // Update the cache in the background
            let tools_clone = cached_tools.clone();
            let cached_for_spawn = cached_tools.clone();
            cx.spawn(async move |this, cx| {
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |screen, cx| {
                        if let Some(ref mut proj_settings) = screen.project_settings {
                            proj_settings.available_tools_cache = Some(cached_for_spawn);
                            if !proj_settings.is_updating_tools {
                                proj_settings.is_updating_tools = true;
                            }
                            cx.notify();
                        }
                    });
                });
            }).detach();

            // Return cached tools to show immediately
            tools_clone
        } else {
            // No cache, return empty and will trigger detection
            AvailableTools {
                editors: vec![],
                git_tools: vec![],
                terminals: vec![],
            }
        }
    };

    // If we just loaded from cache or have no cache, trigger background detection
    if (settings.available_tools_cache.is_some() && is_updating) ||
       (settings.available_tools_cache.is_none() && !is_updating) {
        cx.spawn(async move |this, cx| {
            // Mark as updating if not already
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    if let Some(ref mut proj_settings) = screen.project_settings {
                        if !proj_settings.is_updating_tools {
                            proj_settings.is_updating_tools = true;
                            cx.notify();
                        }
                    }
                });
            });

            // Detect tools in background using std::thread (not tokio!)
            let detected = std::thread::spawn(|| AvailableTools::detect())
                .join()
                .ok();

            if let Some(tools) = detected {
                // Save to cache file
                tools.save_to_cache();

                // Update in-memory cache and mark as done
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |screen, cx| {
                        if let Some(ref mut proj_settings) = screen.project_settings {
                            proj_settings.available_tools_cache = Some(tools);
                            proj_settings.is_updating_tools = false;
                            cx.notify();
                        }
                    });
                });
            }
        }).detach();
    }

    v_flex()
        .gap_6()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Tool Integrations & Preferences")
                )
                .when(is_updating, |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(theme.primary.opacity(0.1))
                            .child(
                                Indicator::new()
                                    .color(theme.primary)
                                    .with_size(ui::Size::XSmall)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.primary)
                                    .child("Detecting tools...")
                            )
                    )
                })
        )
        .child(Divider::horizontal())
        .child(
            v_flex()
                .gap_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Configure default tools for opening projects. Detected tools are shown first, followed by unavailable tools.")
                )
        )
        .child(render_tool_category(
            "Code Editors",
            &tools.editors,
            settings.preferred_editor.as_deref(),
            &project_path,
            "editor",
            &theme,
        ))
        .child(render_tool_category(
            "Git GUI Tools",
            &tools.git_tools,
            settings.preferred_git_tool.as_deref(),
            &project_path,
            "git",
            &theme,
        ))
        .child(render_tool_category(
            "Terminals",
            &tools.terminals,
            None,
            &project_path,
            "terminal",
            &theme,
        ))
}

fn render_tool_category(
    category_name: &str,
    tools: &[ToolInfo],
    default_tool: Option<&str>,
    project_path: &std::path::PathBuf,
    tool_type: &str,
    theme: &ui::theme::Theme,
) -> impl IntoElement {
    // Clone all borrowed data to avoid lifetime issues
    let category_name = category_name.to_string();
    let default_tool = default_tool.map(|s| s.to_string());
    let project_path = project_path.clone();
    let tool_type = tool_type.to_string();
    
    // Sort tools: available first, then unavailable
    let mut sorted_tools = tools.to_vec();
    sorted_tools.sort_by(|a, b| {
        match (a.available, b.available) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    v_flex()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(category_name)
        )
        .child(
            v_flex()
                .gap_2()
                .children(sorted_tools.iter().map(|tool| {
                    render_tool_row(
                        tool,
                        default_tool.as_deref() == Some(&tool.command),
                        &project_path,
                        &tool_type,
                        theme,
                    )
                }))
        )
}

fn render_tool_row(
    tool: &ToolInfo,
    is_default: bool,
    project_path: &std::path::PathBuf,
    tool_type: &str,
    theme: &ui::theme::Theme,
) -> impl IntoElement {
    let available = tool.available;
    let tool_name = tool.name.clone();
    let tool_command = tool.command.clone();
    let path = project_path.clone();
    let tool_type_owned = tool_type.to_string();

    h_flex()
        .items_center()
        .justify_between()
        .w_full()
        .p_3()
        .rounded_md()
        .bg(if available {
            theme.sidebar
        } else {
            theme.muted.opacity(0.05)
        })
        .border_1()
        .border_color(if is_default {
            theme.primary.opacity(0.5)
        } else if available {
            theme.border
        } else {
            theme.muted.opacity(0.2)
        })
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(
                    Icon::new(match tool_type {
                        "editor" => IconName::Code,
                        "git" => IconName::GitHub,
                        "terminal" => IconName::Terminal,
                        _ => IconName::Settings,
                    })
                    .size(px(20.))
                    .text_color(if available {
                        theme.foreground
                    } else {
                        theme.muted_foreground.opacity(0.4)
                    })
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if available {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground.opacity(0.5)
                                })
                                .child(tool_name.clone())
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground.opacity(0.7))
                                .child(format!("Command: {}", tool_command.clone()))
                        )
                )
                .when(is_default, |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Icon::new(IconName::Check)
                                    .size(px(12.))
                                    .text_color(theme.primary)
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(theme.primary.opacity(0.2))
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.primary)
                                    .child("Default")
                            )
                    )
                })
        )
        .child(
            h_flex()
                .gap_2()
                .when(available && !is_default, |this| {
                    let cmd_clone = tool_command.clone();
                    let path_clone = path.clone();
                    let tool_type_clone = tool_type_owned.clone();
                    this.child(
                        Button::new(SharedString::from(format!("set-default-{}", tool_command)))
                            .label("Set as Default")
                            .with_variant(ui::button::ButtonVariant::Ghost)
                            .on_click(move |_, _, _| {
                                save_tool_preference(&path_clone, &cmd_clone, &tool_type_clone);
                            })
                    )
                })
                .when(available, |this| {
                    let cmd_clone = tool_command.clone();
                    let path_clone = path.clone();
                    let tool_type_clone = tool_type_owned.clone();
                    this.child(
                        Button::new(SharedString::from(format!("launch-{}", tool_command)))
                            .label("Launch")
                            .icon(IconName::ArrowUp)
                            .with_variant(ui::button::ButtonVariant::Secondary)
                            .on_click(move |_, _, _| {
                                use crate::entry_screen::integration_launcher;
                                match tool_type_clone.as_str() {
                                    "editor" => {
                                        let _ = integration_launcher::launch_editor(&cmd_clone, &path_clone);
                                    },
                                    "git" => {
                                        let _ = integration_launcher::launch_git_tool(&cmd_clone, &path_clone);
                                    },
                                    "terminal" => {
                                        let _ = integration_launcher::launch_terminal(&cmd_clone, &path_clone);
                                    },
                                    _ => {}
                                }
                            })
                    )
                })
                .when(!available, |this| {
                    this.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme.muted.opacity(0.1))
                            .text_xs()
                            .text_color(theme.muted_foreground.opacity(0.5))
                            .child("Not Installed")
                    )
                })
        )
}

fn save_tool_preference(project_path: &std::path::PathBuf, tool_command: &str, tool_type: &str) {
    let config_path = project_path.join("Pulsar.toml");

    // Read existing config or create new one
    let mut config = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| toml::from_str::<toml::Value>(&content).ok())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()))
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    // Ensure tools table exists
    if !config.as_table().map(|t| t.contains_key("tools")).unwrap_or(false) {
        if let Some(table) = config.as_table_mut() {
            table.insert("tools".to_string(), toml::Value::Table(toml::map::Map::new()));
        }
    }

    let key = match tool_type {
        "editor" => "editor",
        "git" => "git_tool",
        _ => return,
    };

    // Update the preference
    if let Some(table) = config.as_table_mut() {
        if let Some(tools_table) = table.get_mut("tools").and_then(|v| v.as_table_mut()) {
            tools_table.insert(key.to_string(), toml::Value::String(tool_command.to_string()));
        }
    }

    // Write back to file
    if let Ok(toml_string) = toml::to_string_pretty(&config) {
        let _ = std::fs::write(&config_path, toml_string);
    }
}
