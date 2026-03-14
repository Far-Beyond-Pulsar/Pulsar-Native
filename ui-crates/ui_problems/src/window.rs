//! Problems Window - Displays diagnostics, errors, and warnings from rust-analyzer
//! Similar to VS Code's Problems panel as a separate window

use gpui::EventEmitter;
use ui_common::pulsar_drawer_window;

use crate::{ProblemsDrawer, NavigateToDiagnostic};

pulsar_drawer_window!(ProblemsWindow, ProblemsDrawer, problems_drawer, "Window.Title.Problems", 900.0, 600.0);

impl EventEmitter<NavigateToDiagnostic> for ProblemsWindow {}