use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::PluginId;
use plugin_manager::PluginDetails;
use ui::scroll::ScrollbarAxis;
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, TitleBar,
};

use crate::components::{render_empty_state, render_plugin_details, render_plugin_item};
use crate::handlers;

pub struct PluginManagerWindow {
    pub(crate) plugins: Vec<PluginDetails>,
    /// The plugin whose details page is open; `None` shows the list.
    pub(crate) selected: Option<PluginId>,
    pub(crate) focus_handle: FocusHandle,
}

fn load_plugins() -> Vec<PluginDetails> {
    let Some(pm_lock) = plugin_manager::global() else {
        return Vec::new();
    };
    let mut plugins = pm_lock.read().plugin_details();
    plugins.sort_by(|a, b| {
        a.metadata
            .name
            .to_lowercase()
            .cmp(&b.metadata.name.to_lowercase())
    });
    plugins
}

impl PluginManagerWindow {
    pub fn new_global(cx: &mut Context<Self>) -> Self {
        Self {
            plugins: load_plugins(),
            selected: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn select(&mut self, plugin: Option<PluginId>, cx: &mut Context<Self>) {
        self.selected = plugin;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.plugins = load_plugins();
        cx.notify();
    }
}

impl Focusable for PluginManagerWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PluginManagerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_plugins = !self.plugins.is_empty();
        let plugin_count = self.plugins.len();
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.plugins.iter().find(|p| &p.metadata.id == id))
            .cloned();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .when(selected.is_some(), |this| {
                            this.child(
                                Button::new("plugin-details-back")
                                    .icon(IconName::ArrowLeft)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Back to all plugins")
                                    .on_click(cx.listener(handlers::on_back)),
                            )
                        })
                        .child(
                            Icon::new(IconName::Puzzle)
                                .size(px(16.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child("Plugin Manager"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(cx.theme().secondary.opacity(0.45))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{plugin_count}")),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("refresh-plugins")
                                .icon(IconName::Refresh)
                                .ghost()
                                .xsmall()
                                .tooltip("Refresh Plugin List")
                                .on_click(cx.listener(handlers::on_refresh)),
                        ),
                ),
            )
            .child(if let Some(plugin) = selected {
                render_plugin_details(&plugin, &self.plugins, cx).into_any_element()
            } else if has_plugins {
                div()
                    .id("plugin-manager-list")
                    .flex_1()
                    .w_full()
                    .scrollable(ScrollbarAxis::Vertical)
                    .child(v_flex().w_full().p_4().gap_3().children(
                        self.plugins.iter().map(|plugin| {
                            render_plugin_item(plugin, cx)
                        }),
                    ))
                    .into_any_element()
            } else {
                render_empty_state(cx).into_any_element()
            })
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for PluginManagerWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "PluginManagerWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(600.0, 500.0)
    }

    fn build(_: (), _window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(PluginManagerWindow::new_global)
    }
}
