//! Plugin Manager Window

use gpui::*;
use gpui::prelude::FluentBuilder;
use plugin_editor_api::{PluginMetadata, PluginId};
use plugin_manager::PluginManager;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    ActiveTheme as _, Icon, IconName, StyledExt, TitleBar,
};
use ui::Sizable;

/// Plugin Manager Window - Shows loaded plugins with unload functionality
pub struct PluginManagerWindow {
    /// Reference to the plugin manager (shared with main app)
    plugin_manager: *mut PluginManager,

    /// Cached list of plugins for display
    plugins: Vec<PluginMetadata>,

    /// Focus handle for keyboard navigation
    focus_handle: FocusHandle,
}

impl PluginManagerWindow {
    /// Create a new plugin manager window
    pub fn new(plugin_manager: &mut PluginManager, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Load initial plugin list
        let plugins = plugin_manager
            .get_plugins()
            .into_iter()
            .cloned()
            .collect();

        Self {
            plugin_manager: plugin_manager as *mut PluginManager,
            plugins,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Refresh the plugin list
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        unsafe {
            self.plugins = (*self.plugin_manager)
                .get_plugins()
                .into_iter()
                .cloned()
                .collect();
        }
        cx.notify();
    }

    /// Unload a plugin by ID
    fn unload_plugin(&mut self, plugin_id: &PluginId, cx: &mut Context<Self>) {
        unsafe {
            match (*self.plugin_manager).unload_plugin(plugin_id) {
                Ok(_) => {
                    tracing::info!("✅ Plugin unloaded successfully: {}", plugin_id);
                    // Refresh the list
                    self.refresh(cx);
                }
                Err(e) => {
                    tracing::error!("❌ Failed to unload plugin: {:?}", e);
                }
            }
        }
    }
}

impl Render for PluginManagerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_plugins = !self.plugins.is_empty();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Custom titlebar
                TitleBar::new()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_3()
                            .child(
                                Icon::new(IconName::Puzzle)
                                    .size(px(16.))
                                    .text_color(cx.theme().primary)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Plugin Manager")
                            )
                            .child(div().flex_1())  // Spacer
                            .child(
                                Button::new("refresh-plugins")
                                    .icon(IconName::Refresh)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Refresh Plugin List")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.refresh(cx);
                                    }))
                            )
                    )
            )
            .child(
                // Content area
                if has_plugins {
                    // Plugin list
                    v_flex()
                        .flex_1()
                        .w_full()
                        .p_6()
                        .gap_3()
                        .children(
                            self.plugins.iter().map(|plugin| {
                                self.render_plugin_item(plugin, cx)
                            })
                        )
                        .into_any_element()
                } else {
                    // Empty state
                    v_flex()
                        .flex_1()
                        .w_full()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::Puzzle)
                                .size(px(64.))
                                .text_color(cx.theme().muted_foreground.opacity(0.5))
                        )
                        .child(
                            div()
                                .text_lg()
                                .text_color(cx.theme().muted_foreground)
                                .child("No plugins loaded")
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                .child("Place plugin DLLs in the plugins/editor directory")
                        )
                        .into_any_element()
                }
            )
    }
}

impl PluginManagerWindow {
    fn render_plugin_item(&self, plugin: &PluginMetadata, cx: &mut Context<Self>) -> impl IntoElement {
        let plugin_id = plugin.id.clone();
        let plugin_name = plugin.name.clone();
        let plugin_version = plugin.version.clone();
        let plugin_author = plugin.author.clone();

        h_flex()
            .w_full()
            .p_4()
            .gap_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar.opacity(0.5))
            .hover(|style| style.bg(cx.theme().sidebar))
            .child(
                // Plugin icon
                div()
                    .flex_shrink_0()
                    .size(px(48.))
                    .rounded_lg()
                    .bg(cx.theme().primary.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().primary.opacity(0.3))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Puzzle)
                            .size(px(24.))
                            .text_color(cx.theme().primary)
                    )
            )
            .child(
                // Plugin info
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(plugin_name)
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_px()
                                    .rounded(px(4.))
                                    .bg(cx.theme().muted.opacity(0.3))
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(plugin_version)
                            )
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("by {}", plugin_author))
                    )
                    .when(!plugin.description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground.opacity(0.8))
                                .child(plugin.description.clone())
                        )
                    })
            )
            .child(
                // Actions
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .justify_center()
                    .child({
                        let id_for_button = plugin_id.clone();
                        Button::new(SharedString::from(format!("unload-{}", plugin_id)))
                            .label("Unload")
                            .icon(IconName::Trash)
                            .danger()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.unload_plugin(&id_for_button, cx);
                            }))
                    })
            )
    }
}
