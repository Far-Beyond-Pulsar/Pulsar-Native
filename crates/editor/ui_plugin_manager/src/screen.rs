use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::PluginMetadata;
use ui::scroll::ScrollbarAxis;
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, TitleBar,
};

use crate::components::{render_empty_state, render_plugin_item};
use crate::handlers;

pub struct PluginManagerWindow {
    pub(crate) plugins: Vec<PluginEntry>,
    pub(crate) focus_handle: FocusHandle,
}

pub(crate) struct PluginEntry {
    pub(crate) metadata: PluginMetadata,
    pub(crate) is_builtin: bool,
}

fn load_plugins() -> Vec<PluginEntry> {
    let Some(pm_lock) = plugin_manager::global() else {
        return Vec::new();
    };
    let manager = pm_lock.read();
    let mut plugins: Vec<_> = manager
        .get_plugins()
        .into_iter()
        .map(|plugin| PluginEntry {
            metadata: plugin.clone(),
            is_builtin: false,
        })
        .collect();
    plugins.extend(
        manager
            .get_builtin_plugins()
            .into_iter()
            .map(|metadata| PluginEntry {
                metadata,
                is_builtin: true,
            }),
    );
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
            focus_handle: cx.focus_handle(),
        }
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
            .child(if has_plugins {
                div()
                    .id("plugin-manager-list")
                    .flex_1()
                    .w_full()
                    .scrollable(ScrollbarAxis::Vertical)
                    .child(v_flex().w_full().p_4().gap_3().children(
                        self.plugins.iter().map(|plugin| {
                            render_plugin_item(&plugin.metadata, plugin.is_builtin, cx)
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
