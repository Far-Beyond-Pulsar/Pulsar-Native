//! Rendering implementation for PulsarApp

use gpui::UpdateGlobal as _;
use gpui::{
    div, prelude::*, px, rgb, Animation, AnimationExt as _, AnyView, App, Context, Div,
    FocusHandle, Focusable, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Render,
    Stateful, StyleRefinement, Window,
};
use rust_i18n::t;
use std::time::Duration;
use ui::notification::Notification;
use ui::{v_flex, ActiveTheme as _, ContextModal as _};

use super::PulsarApp;

/// Open the window registered as `window_name` when the action `A` fires.
fn open_window_on<A: gpui::Action>(
    el: Div,
    window_name: &'static str,
    cx: &mut Context<PulsarApp>,
) -> Div {
    el.on_action(cx.listener(move |_, _: &A, _, cx| {
        window_manager::WindowRegistry::update_global(cx, |reg, cx| reg.open(window_name, cx));
    }))
}

/// Follow the pointer while the file drawer is being resized, until release.
fn track_drawer_resize<E: InteractiveElement>(el: E, cx: &mut Context<PulsarApp>) -> E {
    el.on_mouse_move(cx.listener(|app, event: &MouseMoveEvent, _window, cx| {
        let mouse_y: f32 = event.position.y.into();
        let drag_delta = mouse_y - app.state.chrome.drawer_resize_start_y;
        let new_height = app.state.chrome.drawer_resize_start_height - drag_delta;
        app.state.chrome.drawer_height = super::chrome::ChromeState::clamp_height(new_height);
        app.chrome_changed(cx);
        cx.notify();
    }))
    .on_mouse_up(
        MouseButton::Left,
        cx.listener(|app, _event, _window, cx| {
            app.state.chrome.drawer_resizing = false;
            cx.notify();
        }),
    )
}

/// The file drawer's top-edge resize handle. `group` names its hover group;
/// `flex_shrink` keeps it from shrinking inside a column that can overflow.
fn drawer_resize_handle(
    id: &'static str,
    group: &'static str,
    flex_shrink: bool,
    resizing: bool,
    cx: &mut Context<PulsarApp>,
) -> Stateful<Div> {
    let resize_handle_blue = Hsla::from(rgb(0x1684ff));
    let resize_handle_black = Hsla::from(rgb(0x000000));
    div()
        .id(id)
        .group(group)
        .relative()
        .w_full()
        .h(px(6.))
        .when(flex_shrink, |this| this.flex_shrink_0())
        .cursor_ns_resize()
        .child(
            div()
                .absolute()
                .top(px(1.5))
                .left_0()
                .right_0()
                .h(px(3.))
                .rounded_tl(px(8.))
                .rounded_tr(px(8.))
                .bg(if resizing {
                    resize_handle_blue
                } else {
                    resize_handle_black
                })
                .when(!resizing, |this| {
                    this.group_hover(group, |style| style.bg(resize_handle_blue.opacity(0.45)))
                }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                this.state.chrome.drawer_resize_start_y = event.position.y.into();
                this.state.chrome.drawer_resize_start_height = this.state.chrome.drawer_height;
                this.state.chrome.drawer_resizing = true;
                cx.notify();
            }),
        )
}

impl PulsarApp {
    /// Repaint the footer after something only it shows changed.
    pub(crate) fn notify_status_bar(&self, cx: &mut Context<Self>) {
        self.state.status_bar.update(cx, |_, cx| cx.notify());
    }

    /// Show the welcome notification on first render if a project was loaded.
    fn show_welcome_notification(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.shown_welcome_notification && self.state.project_path.is_some() {
            self.state.shown_welcome_notification = true;
            if let Some(ref path) = self.state.project_path {
                let project_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project");
                window.push_notification(
                    Notification::info(t!("Notification.Title.ProjectLoaded").to_string()).message(
                        t!(
                            "Notification.Message.WelcomeToProject",
                            project => project_name
                        )
                        .to_string(),
                    ),
                    cx,
                );
            }
        }
    }

    /// Bind the app-level actions: panel toggles, file and editor opening,
    /// history navigation (including the mouse's side buttons) and the
    /// windows the menu opens.
    fn bind_app_actions(el: Div, cx: &mut Context<Self>) -> Div {
        let el = el
            .on_action(cx.listener(Self::on_toggle_file_manager))
            .on_action(cx.listener(Self::on_toggle_problems))
            .on_action(cx.listener(Self::on_toggle_type_debugger))
            .on_action(cx.listener(Self::on_toggle_multiplayer))
            .on_action(cx.listener(Self::on_toggle_agent_chat))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_toggle_unified_sidebar))
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_open_asset))
            .on_action(cx.listener(Self::on_activate_open_editor))
            .on_action(cx.listener(Self::on_go_back))
            .on_action(cx.listener(Self::on_go_forward))
            // The mouse's back and forward side buttons (#24). Anything under
            // the pointer that uses them itself stops them first.
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.on_go_back(&ui_common::menu::GoBack {}, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.on_go_forward(&ui_common::menu::GoForward {}, window, cx);
                }),
            );
        let el = open_window_on::<ui::OpenSettings>(el, "SettingsWindow", cx);
        let el = open_window_on::<ui_common::menu::Settings>(el, "SettingsWindow", cx);
        let el = open_window_on::<ui_common::menu::Preferences>(el, "SettingsWindow", cx);
        let el = open_window_on::<ui_common::menu::AboutApp>(el, "AboutWindow", cx);
        open_window_on::<ui_common::menu::ShowDocumentation>(el, "DocumentationWindow", cx)
    }

    /// The file manager drawer's content, cached like the dock: the footer
    /// and the app's own state change far more often than the drawer does.
    fn drawer_body(&self) -> Div {
        div().flex_1().min_h_0().child(
            AnyView::from(self.state.file_manager_drawer.clone())
                .cached(StyleRefinement::default().size_full()),
        )
    }

    /// The editor area: the cached dock, with the file drawer floating over
    /// it when it is open and not docked.
    fn editor_area(&self, floating_drawer: bool, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .relative()
            // Cached, like the file drawer below: the footer and
            // the app's own state change far more often than the
            // editors do.
            .child(
                AnyView::from(self.state.dock_area.clone())
                    .cached(StyleRefinement::default().size_full()),
            )
            .when(floating_drawer, |this| self.with_floating_drawer(this, cx))
    }

    /// Add the floating file drawer, its backdrop and its resize tracking to
    /// the editor area.
    fn with_floating_drawer(&self, area: Div, cx: &mut Context<Self>) -> Div {
        let resizing = self.state.chrome.drawer_resizing;
        area.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(Hsla::black().opacity(0.3))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app, _, _window, cx| {
                        app.state.chrome.drawer_open = false;
                        cx.notify();
                    }),
                ),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_4()
                .right_4()
                .h(px(self.state.chrome.drawer_height))
                .bg(Hsla::black())
                .rounded_tl(px(8.))
                .rounded_tr(px(8.))
                .border_1()
                .border_color(cx.theme().border)
                .shadow_xl()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .size_full()
                        .child(
                            // Resize handle at top
                            drawer_resize_handle(
                                "drawer-resize-handle",
                                "file-drawer-resize-handle",
                                false,
                                resizing,
                                cx,
                            ),
                        )
                        .child(self.drawer_body()),
                )
                .with_animation("slide-up", Animation::new(Duration::from_secs_f64(0.2)), {
                    let height = self.state.chrome.drawer_height;
                    move |this, delta| this.bottom(px(-height) + delta * px(height))
                }),
        )
        .when(resizing, |this| track_drawer_resize(this, cx))
    }

    /// The file drawer docked under the editor area.
    fn docked_file_drawer(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let resizing = self.state.chrome.drawer_resizing;
        div()
            .id("file-manager-docked-panel")
            .w_full()
            .h(px(self.state.chrome.drawer_height))
            .flex_shrink_0()
            .bg(Hsla::black())
            .border_t_1()
            .border_color(cx.theme().border)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                v_flex()
                    .size_full()
                    .child(drawer_resize_handle(
                        "docked-drawer-resize-handle",
                        "docked-file-drawer-resize-handle",
                        true,
                        resizing,
                        cx,
                    ))
                    .child(self.drawer_body()),
            )
            .when(resizing, |this| track_drawer_resize(this, cx))
    }
}

impl Focusable for PulsarApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.state.focus_handle.clone()
    }
}

impl Render for PulsarApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.show_welcome_notification(window, cx);
        self.sync_sidebar_mode(window, cx);

        let command_palette = if self.state.command_palette_open {
            self.state.command_palette_view.clone()
        } else {
            None
        };

        let project_switcher = if self.state.project_switcher_open {
            self.state.project_switcher_view.clone()
        } else {
            None
        };

        let drawer_open = self.state.chrome.drawer_open;
        let drawer_docked = self.state.chrome.drawer_docked;

        let root = v_flex()
            .size_full()
            .track_focus(&self.state.focus_handle)
            .map(|el| Self::with_menu_actions(el, cx));
        Self::bind_app_actions(root, cx)
            .child(self.with_nav_sidebar(self.editor_area(drawer_open && !drawer_docked, cx)))
            .when(drawer_docked, |this| {
                this.child(self.docked_file_drawer(cx))
            })
            .child(
                AnyView::from(self.state.status_bar.clone()).cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(super::status_bar::STATUS_BAR_HEIGHT))
                        .flex_shrink_0(),
                ),
            )
            .children(command_palette)
            .children(project_switcher)
            .child(self.render_radial_menu(window, cx))
            .into_any_element()
    }
}
