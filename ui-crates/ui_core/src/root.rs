//! Root wrapper component that contains the titlebar and app

use gpui::{div, prelude::*, Context, Entity, IntoElement, Render, SharedString, Window, px, rgba};
use std::path::PathBuf;
use ui::{
    notification::Notification,
    v_flex,
    ActiveTheme as _,
    ContextModal as _,
    Icon,
    IconName,
    StyledExt as _,
    Root,
};
use gpui::UpdateGlobal as _;
use ui_common::menu::{
    AboutApp, AppTitleBar, DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets,
    DevSaveAsDefaultLevel, DevShowBuildInfo, Preferences, Settings, ShowDocumentation,
};

use window_manager::{PulsarWindow, WindowConfig, WindowRegistry};

use crate::app::PulsarApp;

/// Root wrapper that contains the titlebar, matching gpui-component storybook structure
pub struct PulsarRoot {
    title_bar: Entity<AppTitleBar>,
    app: Entity<PulsarApp>,
}

impl PulsarRoot {
    pub fn new(
        title: impl Into<SharedString>,
        app: Entity<PulsarApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_bar = cx.new(|cx| AppTitleBar::new(title, window, cx));
        Self { title_bar, app }
    }
}

impl Render for PulsarRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawer_layer = Root::render_drawer_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);
        let kicked_reason = engine_state::EngineContext::global().and_then(|ctx| {
            ctx.multiuser().and_then(|multiuser| match multiuser.status {
                engine_state::MultiuserStatus::Error(ref message)
                    if message.contains("Kicked from session") =>
                {
                    Some(message.clone())
                }
                _ => None,
            })
        });

        // Belt-and-suspenders action handlers at the root level so that menu
        // actions are always caught regardless of which child element has focus
        // at the time the popup menu fires dispatch_action.
        div()
            .size_full()
            .on_action(cx.listener(|_, _: &Settings, _, cx| {
                WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
            }))
            .on_action(cx.listener(|_, _: &ui::OpenSettings, _, cx| {
                WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
            }))
            .on_action(cx.listener(|_, _: &Preferences, _, cx| {
                WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
            }))
            .on_action(cx.listener(|_, _: &AboutApp, _, cx| {
                WindowRegistry::update_global(cx, |reg, cx| reg.open("AboutWindow", cx));
            }))
            .on_action(cx.listener(|_, _: &ShowDocumentation, _, cx| {
                WindowRegistry::update_global(cx, |reg, cx| reg.open("DocumentationWindow", cx));
            }))
            .on_action(cx.listener(
                |_: &mut PulsarRoot, _: &DevSaveAsDefaultLevel, window, cx| {
                    window.push_notification(
                        Notification::info("Dev")
                            .message("Use \"Save as Default\" in the level editor toolbar."),
                        cx,
                    );
                },
            ))
            .on_action(
                cx.listener(|_: &mut PulsarRoot, _: &DevOpenWorkspaceRoot, window, cx| {
                    if let Some(path) = engine_state::EngineContext::global()
                        .and_then(|ctx| ctx.dev.read().source_path.clone())
                    {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                        #[cfg(target_os = "windows")]
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                        #[cfg(target_os = "linux")]
                        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                        window.push_notification(
                            Notification::info("Dev")
                                .message(format!("Opening {}", path.display())),
                            cx,
                        );
                    }
                }),
            )
            .on_action(
                cx.listener(|_: &mut PulsarRoot, _: &DevShowBuildInfo, window, cx| {
                    let info = engine_state::EngineContext::global()
                        .map(|ctx| {
                            let dev = ctx.dev.read();
                            format!(
                                "Source build: {}\nWorkspace root: {}",
                                dev.is_source_build,
                                dev.source_path
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "N/A".into()),
                            )
                        })
                        .unwrap_or_else(|| "Engine context unavailable".into());
                    window.push_notification(Notification::info("Build Info").message(info), cx);
                }),
            )
            .on_action(
                cx.listener(|_: &mut PulsarRoot, _: &DevReloadAssets, window, cx| {
                    window.push_notification(
                        Notification::info("Dev").message("Asset reload not yet implemented."),
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(
                |_: &mut PulsarRoot, _: &DevInspectEngineState, window, cx| {
                    window.push_notification(
                        Notification::info("Dev")
                            .message("Engine state inspector not yet implemented."),
                        cx,
                    );
                },
            ))
            .child(
                v_flex()
                    .size_full()
                    .child(self.title_bar.clone())
                    .child(div().flex_1().overflow_hidden().child(self.app.clone())),
            )
            .children(drawer_layer)
            .children(modal_layer)
            .children(notification_layer)
            .when_some(kicked_reason, |this, reason| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x000000e6))
                        .child(
                            v_flex()
                                .w(px(520.))
                                .gap_4()
                                .p_6()
                                .rounded(px(12.))
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .shadow_lg()
                                .child(
                                    div()
                                        .flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(24.))
                                                .text_color(cx.theme().danger),
                                        )
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_bold()
                                                .text_color(cx.theme().foreground)
                                                .child("Disconnected"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(reason),
                                ),
                        ),
                )
            })
    }
}

impl PulsarWindow for PulsarRoot {
    type Params = PathBuf;

    fn window_name() -> &'static str {
        "PulsarEditorWindow"
    }

    fn window_options(_path: &PathBuf) -> gpui::WindowOptions {
        WindowConfig::editor()
    }

    fn build(path: PathBuf, window: &mut Window, cx: &mut gpui::App) -> Entity<Self> {
        let app = cx.new(|cx| PulsarApp::new_with_project(path, window, cx));
        cx.new(|cx| PulsarRoot::new("Pulsar Engine", app, window, cx))
    }
}
