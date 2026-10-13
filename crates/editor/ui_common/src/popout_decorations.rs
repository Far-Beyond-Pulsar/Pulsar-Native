//! The engine's chrome for pop-out windows.
//!
//! A tab dragged out of a dock area becomes a secondary window with no
//! operating-system decorations. [`install`] makes those windows wear a
//! simplified version of the main window's title bar (see [`AppTitleBar`]):
//! the same unified surface and window controls, with the panel's title on
//! the left and the theme toggle on the right in place of the menu bar and
//! account controls, which belong to the main window.
//!
//! [`AppTitleBar`]: crate::menu::AppTitleBar

use gpui::{
    div, App, ElementId, InteractiveElement as _, IntoElement as _, MouseButton, ParentElement as _,
    Styled as _, Window,
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{set_popout_decorations, PopoutContext, PopoutDecorations},
    h_flex, v_flex, window_border, ActiveTheme as _, IconName, Sizable as _, Theme, ThemeMode,
    TitleBar,
};

/// Use the engine's title bar for every pop-out window, open or future.
pub fn install(cx: &mut App) {
    set_popout_decorations(EnginePopoutDecorations, cx);
}

/// The simplified engine title bar around a pop-out window's dock area.
pub struct EnginePopoutDecorations;

impl PopoutDecorations for EnginePopoutDecorations {
    fn decorate(
        &self,
        content: gpui::AnyElement,
        popout: &PopoutContext,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::AnyElement {
        let background = cx.theme().background;
        let to_mode = if cx.theme().mode.is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        let close = popout.clone();

        let title_bar = TitleBar::new()
            .unified_background(background)
            .on_close_window(move |_, window, cx| close.close(window, cx))
            .child(h_flex().items_center().child(popout.title(window, cx)))
            .child(
                h_flex()
                    .items_center()
                    .justify_end()
                    .px_2()
                    .gap_2()
                    // Controls must not start a window drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new(ElementId::from("popout-theme-mode"))
                            .icon(if to_mode == ThemeMode::Light {
                                IconName::Sun
                            } else {
                                IconName::Moon
                            })
                            .small()
                            .ghost()
                            .tooltip("Toggle light / dark mode")
                            .on_click(move |_, _, cx| Theme::change(to_mode, None, cx)),
                    ),
            );

        window_border()
            .child(
                v_flex()
                    .size_full()
                    .bg(background)
                    .child(title_bar)
                    .child(div().flex_1().overflow_hidden().child(content)),
            )
            .into_any_element()
    }
}
