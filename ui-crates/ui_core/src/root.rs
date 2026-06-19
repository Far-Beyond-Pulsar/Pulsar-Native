//! Root wrapper component that contains the titlebar and app

use gpui::UpdateGlobal as _;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, rgba, AnyView, Context, Entity, IntoElement,
    Render, SharedString, Subscription, Window,
};
use std::path::PathBuf;
use ui::{
    notification::Notification, v_flex, ActiveTheme as _, ContextModal as _, Icon, IconName, Root,
    StyledExt as _,
};
use ui_common::menu::{
    AboutApp, AppTitleBar, AppTitleBarEvent, DevInspectEngineState, DevOpenWorkspaceRoot,
    DevReloadAssets, DevSaveAsDefaultLevel, DevShowBuildInfo, Preferences, Settings,
    ShowDocumentation,
};

use window_manager::{
    register_window_wrapper, PulsarWindow, WindowConfig, WindowContentWrapper, WindowRegistry,
    WindowRequest,
};

use crate::app::PulsarApp;

/// Root wrapper that contains the titlebar, matching gpui-component storybook structure
pub struct PulsarRoot {
    app: Entity<PulsarApp>,
}

struct EditorWindowShell {
    title_bar: Entity<AppTitleBar>,
    content: AnyView,
    show_multiplayer: bool,
    friends_screen: Entity<ui_friends::FriendsScreen>,
    _subscriptions: Vec<Subscription>,
}

impl EditorWindowShell {
    fn new(
        title: impl Into<SharedString>,
        content: AnyView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_bar = cx.new(|cx| AppTitleBar::new(title, window, cx));
        let friends_screen = cx.new(|cx| ui_friends::FriendsScreen::new(window, cx));

        let subscriptions = vec![cx.subscribe(
            &title_bar,
            |this, _, event: &AppTitleBarEvent, cx| {
                match event {
                    AppTitleBarEvent::MultiplayerSessionsRequested => {
                        this.show_multiplayer = !this.show_multiplayer;
                        cx.notify();
                    }
                }
            },
        )];

        Self {
            title_bar,
            content,
            show_multiplayer: false,
            friends_screen,
            _subscriptions: subscriptions,
        }
    }
}

impl PulsarRoot {
    pub fn new(app: Entity<PulsarApp>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { app }
    }
}

impl Render for PulsarRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawer_layer = Root::render_drawer_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);
        let kicked_reason = engine_state::EngineContext::global().and_then(|ctx| {
            ctx.multiuser()
                .and_then(|multiuser| match multiuser.status {
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
                        .and_then(|ctx| ctx.store.get_or_init::<engine_state::DevContext>().read().source_path.clone())
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
                            let dev = ctx.store.get_or_init::<engine_state::DevContext>().get();
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
                    .child(div().size_full().overflow_hidden().child(self.app.clone())),
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

impl Render for EditorWindowShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();

        div()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .child(self.title_bar.clone())
                    .child(div().flex_1().overflow_hidden().child(self.content.clone()))
            )
            .when(self.show_multiplayer, |this| {
                this.child(
                    deferred(
                        anchored()
                            .anchor(gpui::Corner::TopRight)
                            .position(point(vp.width - px(8.), px(34.)))
                            .child(
                                div()
                                    .occlude()
                                    .on_mouse_down_out(cx.listener(
                                        |this, _: &gpui::MouseDownEvent, _, cx| {
                                            this.show_multiplayer = false;
                                            cx.notify();
                                        },
                                    ))
                                    .child(self.friends_screen.clone()),
                            ),
                    )
                    .with_priority(1),
                )
            })
    }
}

pub fn register_window_wrappers(_cx: &mut gpui::App) {
    register_window_wrapper(WindowContentWrapper::Editor, |content, window, cx| {
        let wrapped_shell =
            cx.new(|cx| EditorWindowShell::new("Pulsar Engine", content, window, cx));
        let wrapped_root = cx.new(|cx| Root::new(wrapped_shell.into(), window, cx));
        wrapped_root.into()
    });
}

impl PulsarWindow for PulsarRoot {
    type Params = PathBuf;

    fn window_name() -> &'static str {
        "PulsarEditorWindow"
    }

    fn window_options(_path: &PathBuf) -> gpui::WindowOptions {
        WindowConfig::editor()
    }

    fn window_profile(_path: &PathBuf) -> Option<window_manager::WindowProfile> {
        Some(WindowConfig::editor_profile())
    }

    fn window_request(path: &PathBuf) -> WindowRequest {
        WindowRequest::ProjectEditor {
            project_path: path.to_string_lossy().to_string(),
        }
    }

    fn build(path: PathBuf, window: &mut Window, cx: &mut gpui::App) -> Entity<Self> {
        let app = cx.new(|cx| PulsarApp::new_with_project(path, window, cx));
        cx.new(|cx| PulsarRoot::new(app, window, cx))
    }
}
