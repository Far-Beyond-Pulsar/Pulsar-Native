//! Type Debugger Window - Displays type database contents
//! Similar to VS Code's type inspector as a separate window

use gpui::*;
use ui::{
    v_flex, ActiveTheme as _, TitleBar,
};
use ui_common::translate;

use crate::{TypeDebuggerDrawer, NavigateToType};

pub struct TypeDebuggerWindow {
    type_debugger_drawer: Entity<TypeDebuggerDrawer>,
}

impl TypeDebuggerWindow {
    pub fn new(
        type_debugger_drawer: Entity<TypeDebuggerDrawer>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { type_debugger_drawer }
    }

    pub fn type_debugger_drawer(&self) -> &Entity<TypeDebuggerDrawer> {
        &self.type_debugger_drawer
    }
}

impl EventEmitter<NavigateToType> for TypeDebuggerWindow {}

impl Render for TypeDebuggerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child(translate("Window.Title.TypeDebugger")))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.type_debugger_drawer.clone())
            )
    }
}
