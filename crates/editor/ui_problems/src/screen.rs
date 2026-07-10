use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    popup_menu::PopupMenuExt,
    v_flex, ActiveTheme as _, IconName, Sizable as _,
};

use crate::utils::types::{Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic};

pub struct ProblemsDrawer {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
    pub(crate) filtered_severity: Option<DiagnosticSeverity>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) search_query: String,
    pub(crate) group_by_file: bool,
    pub(crate) preview_inputs: HashMap<(String, usize), Entity<InputState>>,
    pub(crate) diff_editors: HashMap<(usize, usize), (Entity<InputState>, Entity<InputState>)>,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) project_root: Option<PathBuf>,
}

impl EventEmitter<NavigateToDiagnostic> for ProblemsDrawer {}

impl ProblemsDrawer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search problems..."));
        Self {
            focus_handle,
            diagnostics,
            filtered_severity: None,
            selected_index: None,
            search_query: String::new(),
            group_by_file: true,
            preview_inputs: HashMap::new(),
            diff_editors: HashMap::new(),
            search_input,
            project_root: None,
        }
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic, cx: &mut Context<Self>) {
        self.diagnostics.lock().unwrap().push(diagnostic);
        cx.notify();
    }

    pub fn clear_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics.lock().unwrap().clear();
        self.selected_index = None;
        self.preview_inputs.clear();
        cx.notify();
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        *self.diagnostics.lock().unwrap() = diagnostics;
        self.selected_index = None;
        self.preview_inputs.clear();
        self.diff_editors.clear();
        cx.notify();
    }

    pub fn update_diagnostic_hints(
        &mut self,
        diagnostic_index: usize,
        new_hints: Vec<Hint>,
        cx: &mut Context<Self>,
    ) {
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            if let Some(diag) = diagnostics.get_mut(diagnostic_index) {
                if !new_hints.is_empty() {
                    for hint in new_hints {
                        diag.hints.push(hint);
                    }
                }
                diag.loading_actions = false;
            }
        }
        self.diff_editors
            .retain(|(d_idx, _), _| *d_idx != diagnostic_index);
        cx.notify();
    }

    pub fn set_diagnostic_loading(
        &mut self,
        diagnostic_index: usize,
        loading: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(diag) = self.diagnostics.lock().unwrap().get_mut(diagnostic_index) {
            diag.loading_actions = loading;
        }
        cx.notify();
    }

    pub fn get_diagnostic_info(
        &self,
        index: usize,
    ) -> Option<(String, usize, usize, usize, usize)> {
        self.diagnostics.lock().unwrap().get(index).map(|d| {
            (
                d.file_path.clone(),
                d.line,
                d.column,
                d.end_line.unwrap_or(d.line),
                d.end_column.unwrap_or(d.column),
            )
        })
    }

    pub fn set_project_root(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.project_root = project_root;
        cx.notify();
    }

    pub(crate) fn navigate_to_diagnostic(
        &mut self,
        diagnostic: &Diagnostic,
        cx: &mut Context<Self>,
    ) {
        cx.emit(NavigateToDiagnostic {
            file_path: PathBuf::from(&diagnostic.file_path),
            line: diagnostic.line,
            column: diagnostic.column,
        });
    }

    pub(crate) fn select_diagnostic(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_index = Some(index);
        cx.notify();
    }

    pub(crate) fn toggle_grouping(&mut self, cx: &mut Context<Self>) {
        self.group_by_file = !self.group_by_file;
        cx.notify();
    }
}

impl Focusable for ProblemsDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProblemsDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input_value = self.search_input.read(cx).value().to_string();
        if current_input_value != self.search_query {
            self.search_query = current_input_value;
        }

        let error_count = self.count_by_severity(DiagnosticSeverity::Error);
        let warning_count = self.count_by_severity(DiagnosticSeverity::Warning);
        let info_count = self.count_by_severity(DiagnosticSeverity::Information);
        let total_count = self.total_count();
        let filtered_diagnostics = self.get_filtered_diagnostics();
        let selected_index = self.selected_index;
        let group_by_file = self.group_by_file;

        let content: AnyElement = if filtered_diagnostics.is_empty() {
            crate::components::render_empty_state(self, cx).into_any_element()
        } else if group_by_file {
            crate::components::render_grouped_view(self, selected_index, window, cx)
                .into_any_element()
        } else {
            crate::components::render_flat_view(
                self,
                filtered_diagnostics,
                selected_index,
                window,
                cx,
            )
            .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(crate::handlers::on_filter_all))
            .on_action(cx.listener(crate::handlers::on_filter_errors))
            .on_action(cx.listener(crate::handlers::on_filter_warnings))
            .on_action(cx.listener(crate::handlers::on_filter_info))
            .child(crate::components::render_header(
                self,
                error_count,
                warning_count,
                info_count,
                total_count,
                cx,
            ))
            .child(div().flex_1().overflow_hidden().child(content))
    }
}
