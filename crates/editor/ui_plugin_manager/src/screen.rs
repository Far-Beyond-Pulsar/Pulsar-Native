use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::PluginMetadata;
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, TitleBar,
};

use crate::components::{render_empty_state, render_plugin_item};
use crate::handlers;

pub struct PluginManagerWindow {
    pub(crate) plugins: Vec<PluginMetadata>,
    pub(crate) focus_handle: FocusHandle,
}

impl PluginManagerWindow {
    pub fn new_global(cx: &mut Context<Self>) -> Self {
        let plugins = if let Some(pm_lock) = plugin_manager::global() {
            pm_lock.read().get_plugins().into_iter().cloned().collect()
        } else {
            Vec::new()
        };

        Self {
            plugins,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(pm_lock) = plugin_manager::global() {
            self.plugins = pm_lock.read().get_plugins().into_iter().cloned().collect();
        }
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

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::Puzzle)
                                .size(px(16.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("Plugin Manager"),
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
            .child(if has_plugins {
                v_flex()
                    .flex_1()
                    .w_full()
                    .p_6()
                    .gap_3()
                    .children(
                        self.plugins
                            .iter()
                            .map(|plugin| render_plugin_item(plugin, cx)),
                    )
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
