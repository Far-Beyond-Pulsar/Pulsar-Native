//! Root wrapper component that contains the titlebar and app

use gpui::{div, prelude::*, Context, Entity, IntoElement, Render, SharedString, Window};
use ui::{notification::Notification, v_flex, ContextModal as _, Root};
use ui_common::menu::{
    AboutApp, AppTitleBar, DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets,
    DevSaveAsDefaultLevel, DevShowBuildInfo, Preferences, Settings, ShowDocumentation,
};

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

        // Belt-and-suspenders action handlers at the root level so that menu
        // actions are always caught regardless of which child element has focus
        // at the time the popup menu fires dispatch_action.
        div()
            .size_full()
            .on_action(
                cx.listener(|this: &mut PulsarRoot, _: &Settings, window, cx| {
                    this.app.update(cx, |app, cx| app.open_settings(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this: &mut PulsarRoot, _: &ui::OpenSettings, window, cx| {
                    this.app.update(cx, |app, cx| app.open_settings(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this: &mut PulsarRoot, _: &Preferences, window, cx| {
                    this.app.update(cx, |app, cx| app.open_settings(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this: &mut PulsarRoot, _: &AboutApp, window, cx| {
                    this.app.update(cx, |app, cx| app.open_about(window, cx));
                }),
            )
            .on_action(
                cx.listener(|this: &mut PulsarRoot, _: &ShowDocumentation, window, cx| {
                    this.app
                        .update(cx, |app, cx| app.open_documentation(window, cx));
                }),
            )
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
    }
}
