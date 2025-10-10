//! Problems Window - Displays diagnostics, errors, and warnings from rust-analyzer
//! Similar to VS Code's Problems panel as a separate window

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName, StyledExt, TitleBar,
};

use super::problems_drawer::{Diagnostic, DiagnosticSeverity, ProblemsDrawer, NavigateToDiagnostic};

pub struct ProblemsWindow {
    problems_drawer: Entity<ProblemsDrawer>,
}

impl ProblemsWindow {
    pub fn new(
        problems_drawer: Entity<ProblemsDrawer>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { problems_drawer }
    }

    pub fn problems_drawer(&self) -> &Entity<ProblemsDrawer> {
        &self.problems_drawer
    }
}

impl EventEmitter<NavigateToDiagnostic> for ProblemsWindow {}

impl Render for ProblemsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child("Problems"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.problems_drawer.clone())
            )
    }
}
