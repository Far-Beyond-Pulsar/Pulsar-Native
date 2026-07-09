// Problems Drawer - Studio-quality diagnostics panel

use gpui::{prelude::*, *};
use rust_i18n::t;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    popup_menu::PopupMenuExt,
    v_flex, ActiveTheme as _, IconName, Sizable as _,
};

pub use crate::filter::{Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic};

actions!(
    problems_drawer,
    [FilterAll, FilterErrors, FilterWarnings, FilterInfo]
);

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

    fn on_filter_all(&mut self, _: &FilterAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(None, cx);
    }
    fn on_filter_errors(&mut self, _: &FilterErrors, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(DiagnosticSeverity::Error), cx);
    }
    fn on_filter_warnings(&mut self, _: &FilterWarnings, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(DiagnosticSeverity::Warning), cx);
    }
    fn on_filter_info(&mut self, _: &FilterInfo, _: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(Some(DiagnosticSeverity::Information), cx);
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
            self.render_empty_state(cx).into_any_element()
        } else if group_by_file {
            self.render_grouped_view(selected_index, window, cx)
                .into_any_element()
        } else {
            self.render_flat_view(filtered_diagnostics, selected_index, window, cx)
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::on_filter_all))
            .on_action(cx.listener(Self::on_filter_errors))
            .on_action(cx.listener(Self::on_filter_warnings))
            .on_action(cx.listener(Self::on_filter_info))
            .child(self.render_header(error_count, warning_count, info_count, total_count, cx))
            .child(div().flex_1().overflow_hidden().child(content))
    }
}

impl ProblemsDrawer {
    fn render_header(
        &mut self,
        error_count: usize,
        warning_count: usize,
        info_count: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current_filter_label = match &self.filtered_severity {
            None => format!("All Problems ({})", total_count),
            Some(DiagnosticSeverity::Error) => format!("Errors ({})", error_count),
            Some(DiagnosticSeverity::Warning) => format!("Warnings ({})", warning_count),
            Some(DiagnosticSeverity::Information) => format!("Info ({})", info_count),
            Some(DiagnosticSeverity::Hint) => "Hints".to_string(),
        };

        let is_all_selected = self.filtered_severity.is_none();
        let is_errors_selected = self.filtered_severity == Some(DiagnosticSeverity::Error);
        let is_warnings_selected = self.filtered_severity == Some(DiagnosticSeverity::Warning);
        let is_info_selected = self.filtered_severity == Some(DiagnosticSeverity::Information);

        v_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(t!("Problems.Title").to_string()),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .when(error_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Error,
                                            error_count,
                                            cx,
                                        ))
                                    })
                                    .when(warning_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Warning,
                                            warning_count,
                                            cx,
                                        ))
                                    })
                                    .when(info_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Information,
                                            info_count,
                                            cx,
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("toggle-grouping")
                                    .ghost()
                                    .small()
                                    .icon(if self.group_by_file {
                                        IconName::List
                                    } else {
                                        IconName::Folder
                                    })
                                    .tooltip(if self.group_by_file {
                                        t!("Problems.Action.ShowFlatList").to_string()
                                    } else {
                                        t!("Problems.Action.GroupByFile").to_string()
                                    })
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.toggle_grouping(cx)),
                                    ),
                            )
                            .child(
                                Button::new("clear-all")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .tooltip(t!("Problems.Action.ClearAll").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.clear_diagnostics(cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().min_w(px(200.0)).child(
                            TextInput::new(&self.search_input).w_full().prefix(
                                ui::Icon::new(IconName::Search)
                                    .size_4()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                        ),
                    )
                    .child(
                        Button::new("filter-dropdown")
                            .ghost()
                            .small()
                            .icon(IconName::Filter)
                            .label(current_filter_label.clone())
                            .popup_menu_with_anchor(
                                Corner::BottomRight,
                                move |menu, _window, _cx| {
                                    menu.menu_with_check(
                                        t!("Problems.Filter.All").to_string(),
                                        is_all_selected,
                                        Box::new(FilterAll),
                                    )
                                    .separator()
                                    .menu_with_check(
                                        t!("Problems.Filter.Errors").to_string(),
                                        is_errors_selected,
                                        Box::new(FilterErrors),
                                    )
                                    .menu_with_check(
                                        t!("Problems.Filter.Warnings").to_string(),
                                        is_warnings_selected,
                                        Box::new(FilterWarnings),
                                    )
                                    .menu_with_check(
                                        t!("Problems.Filter.Information").to_string(),
                                        is_info_selected,
                                        Box::new(FilterInfo),
                                    )
                                },
                            ),
                    ),
            )
    }

    fn render_severity_badge(
        &self,
        severity: DiagnosticSeverity,
        count: usize,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_0p5()
            .rounded_md()
            .bg(severity.color(cx).opacity(0.15))
            .child(
                ui::Icon::new(severity.icon())
                    .size_3()
                    .text_color(severity.color(cx)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(severity.color(cx))
                    .child(count.to_string()),
            )
    }

    fn render_empty_state(&self, cx: &App) -> Div {
        div().size_full().child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .gap_4()
                        .items_center()
                        .max_w(px(400.0))
                        .px_6()
                        .py_8()
                        .rounded_xl()
                        .bg(cx.theme().secondary.opacity(0.2))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.3))
                        .child(
                            div()
                                .w(px(64.0))
                                .h(px(64.0))
                                .rounded_full()
                                .bg(cx.theme().success.opacity(0.15))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    ui::Icon::new(IconName::Check)
                                        .size(px(32.0))
                                        .text_color(cx.theme().success),
                                ),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child("No problems detected"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .line_height(rems(1.5))
                                .child(if !self.search_query.is_empty() {
                                    t!("Problems.Empty.NoMatch").to_string()
                                } else {
                                    t!("Problems.Empty.AllGood").to_string()
                                }),
                        ),
                ),
        )
    }
}
