//! Problems Window - Displays diagnostics, errors, and warnings from rust-analyzer
//! Similar to VS Code's Problems panel as a separate window

use gpui::EventEmitter;
use ui_common::drawer_window;

use crate::{ProblemsDrawer, NavigateToDiagnostic};

drawer_window!(ProblemsWindow, ProblemsDrawer, problems_drawer, "Window.Title.Problems");

impl EventEmitter<NavigateToDiagnostic> for ProblemsWindow {}