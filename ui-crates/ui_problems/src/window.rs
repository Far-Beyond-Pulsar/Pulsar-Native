//! Problems Window - Displays diagnostics, errors, and warnings from rust-analyzer
//! Similar to VS Code's Problems panel as a separate window

use gpui::*;
use ui::drawer_window_entity;
use ui_common::translate;

use crate::{ProblemsDrawer, NavigateToDiagnostic};

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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drawer_window_entity("Window.Title.Problems", self.problems_drawer.clone(), cx)
    }
}
