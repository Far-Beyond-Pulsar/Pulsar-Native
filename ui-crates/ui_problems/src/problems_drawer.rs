// Problems Drawer - Studio-quality diagnostics panel
// Displays rust-analyzer diagnostics with professional UI and search capabilities

use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _,
    input::{InputState, TextInput},
    indicator::Indicator,
    scroll::ScrollbarAxis,
    popup_menu::{PopupMenu, PopupMenuExt},
};
use ui::StyledExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::fs;
use similar::{ChangeTag, TextDiff};

// Define actions for filter menu
actions!(problems_drawer, [FilterAll, FilterErrors, FilterWarnings, FilterInfo]);

/// Represents the type of a diff line for rendering
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffLineType {
    Unchanged,
    Added,
    Deleted,
    Spacer, // Empty line to align both sides
}

/// Compute aligned diff lines for side-by-side display
/// Returns (left_lines, right_lines) where each line is (line_number, type, content)
fn compute_aligned_diff(before: &str, after: &str) -> (Vec<(Option<usize>, DiffLineType, String)>, Vec<(Option<usize>, DiffLineType, String)>) {
    let diff = TextDiff::from_lines(before, after);
    
    let mut left_lines: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    let mut right_lines: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    
    let mut left_line_num = 1usize;
    let mut right_line_num = 1usize;
    
    for change in diff.iter_all_changes() {
        let content = change.value().trim_end_matches('\n').to_string();
        
        match change.tag() {
            ChangeTag::Equal => {
                // Unchanged line appears on both sides
                left_lines.push((Some(left_line_num), DiffLineType::Unchanged, content.clone()));
                right_lines.push((Some(right_line_num), DiffLineType::Unchanged, content));
                left_line_num += 1;
                right_line_num += 1;
            }
            ChangeTag::Delete => {
                // Deleted line only appears on left side, add spacer on right
                left_lines.push((Some(left_line_num), DiffLineType::Deleted, content));
                right_lines.push((None, DiffLineType::Spacer, "".to_string()));
                left_line_num += 1;
            }
            ChangeTag::Insert => {
                // Added line only appears on right side, add spacer on left
                left_lines.push((None, DiffLineType::Spacer, "".to_string()));
                right_lines.push((Some(right_line_num), DiffLineType::Added, content));
                right_line_num += 1;
            }
        }
    }
    
    (left_lines, right_lines)
}

// Local diagnostic types
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    /// End line of the diagnostic range (for code action requests)
    pub end_line: Option<usize>,
    /// End column of the diagnostic range (for code action requests)
    pub end_column: Option<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub hints: Vec<Hint>,
    pub subitems: Vec<Diagnostic>,
    /// Whether we're currently loading code actions for this diagnostic
    pub loading_actions: bool,
}

#[derive(Clone, Debug)]
pub struct Hint {
    pub message: String,
    /// The original code before the suggested fix
    pub before_content: Option<String>,
    /// The suggested fix (code after applying the hint)
    pub after_content: Option<String>,
    /// The file path this hint applies to
    pub file_path: Option<String>,
    /// The line number this hint applies to
    pub line: Option<usize>,
    /// Whether we're currently loading code actions for this hint
    pub loading: bool,
}

#[derive(Clone, Debug)]
pub struct NavigateToDiagnostic {
    pub file_path: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl EventEmitter<NavigateToDiagnostic> for ProblemsDrawer {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn icon(&self) -> IconName {
        match self {
            Self::Error => IconName::Close,
            Self::Warning => IconName::TriangleAlert,
            Self::Information => IconName::Info,
            Self::Hint => IconName::Info,
        }
    }

    pub fn color(&self, cx: &App) -> Hsla {
        match self {
            Self::Error => Hsla { h: 0.0, s: 0.85, l: 0.55, a: 1.0 },
            Self::Warning => Hsla { h: 38.0, s: 0.95, l: 0.55, a: 1.0 },
            Self::Information => Hsla { h: 210.0, s: 0.80, l: 0.60, a: 1.0 },
            Self::Hint => cx.theme().muted_foreground,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Information => "Info",
            Self::Hint => "Hint",
        }
    }
}

pub struct ProblemsDrawer {
    focus_handle: FocusHandle,
    diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
    filtered_severity: Option<DiagnosticSeverity>,
    selected_index: Option<usize>,
    search_query: String,
    group_by_file: bool,
    /// Cache of InputState entities for file previews, keyed by (file_path, line)
    preview_inputs: HashMap<(String, usize), Entity<InputState>>,
    /// Cache of InputState entities for diff views (before, after), keyed by diagnostic index and hint index
    diff_editors: HashMap<(usize, usize), (Entity<InputState>, Entity<InputState>)>,
    /// InputState for the search bar
    search_input: Entity<InputState>,
    /// Project root path for computing relative paths
    project_root: Option<PathBuf>,
}

impl ProblemsDrawer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));

        // Create search input state
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search problems...")
        });

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
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            diagnostics.push(diagnostic);
        }
        cx.notify();
    }

    pub fn clear_diagnostics(&mut self, cx: &mut Context<Self>) {
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            diagnostics.clear();
        }
        self.selected_index = None;
        self.preview_inputs.clear();
        cx.notify();
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        {
            let mut diag = self.diagnostics.lock().unwrap();
            *diag = diagnostics;
        }
        self.selected_index = None;
        self.preview_inputs.clear();
        self.diff_editors.clear();
        cx.notify();
    }

    /// Update a specific diagnostic with loaded code actions
    /// This MERGES new hints with existing ones (doesn't replace)
    pub fn update_diagnostic_hints(&mut self, diagnostic_index: usize, new_hints: Vec<Hint>, cx: &mut Context<Self>) {
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            if let Some(diag) = diagnostics.get_mut(diagnostic_index) {
                // Merge: keep existing hints that don't have before/after content,
                // add new hints that do have before/after content
                if !new_hints.is_empty() {
                    // Add new code action hints
                    for hint in new_hints {
                        diag.hints.push(hint);
                    }
                }
                diag.loading_actions = false;
            }
        }
        // Clear cached diff editors for this diagnostic so they get recreated
        self.diff_editors.retain(|(d_idx, _), _| *d_idx != diagnostic_index);
        cx.notify();
    }

    /// Mark a diagnostic as loading code actions
    pub fn set_diagnostic_loading(&mut self, diagnostic_index: usize, loading: bool, cx: &mut Context<Self>) {
        {
            let mut diagnostics = self.diagnostics.lock().unwrap();
            if let Some(diag) = diagnostics.get_mut(diagnostic_index) {
                diag.loading_actions = loading;
            }
        }
        cx.notify();
    }

    /// Get diagnostic info for requesting code actions
    pub fn get_diagnostic_info(&self, index: usize) -> Option<(String, usize, usize, usize, usize)> {
        let diagnostics = self.diagnostics.lock().unwrap();
        diagnostics.get(index).map(|d| {
            (
                d.file_path.clone(),
                d.line,
                d.column,
                d.end_line.unwrap_or(d.line),
                d.end_column.unwrap_or(d.column),
            )
        })
    }

    /// Set the project root path for computing relative paths
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.project_root = project_root;
        cx.notify();
    }

    /// Compute relative path from absolute path using project root
    fn get_display_path(&self, absolute_path: &str) -> String {
        if let Some(project_root) = &self.project_root {
            // Normalize both paths to use forward slashes for comparison
            let abs_path = PathBuf::from(absolute_path);
            
            // Try to strip the project root prefix
            if let Ok(relative) = abs_path.strip_prefix(project_root) {
                // Get the project folder name
                if let Some(project_name) = project_root.file_name() {
                    // Return project_name/relative_path
                    let mut display_path = PathBuf::from(project_name);
                    display_path.push(relative);
                    return display_path.to_string_lossy().replace('\\', "/");
                }
            }
        }
        
        // Fallback to absolute path if project root not set or path doesn't match
        absolute_path.replace('\\', "/")
    }

    fn get_filtered_diagnostics(&self) -> Vec<Diagnostic> {
        let diagnostics = self.diagnostics.lock().unwrap();

        // Early return if no filtering needed
        if self.filtered_severity.is_none() && self.search_query.is_empty() {
            return diagnostics.clone();
        }

        // Build filtered vec with a single pass
        let query = if !self.search_query.is_empty() {
            Some(self.search_query.to_lowercase())
        } else {
            None
        };

        diagnostics
            .iter()
            .filter(|d| {
                // Filter by severity
                if let Some(severity) = &self.filtered_severity {
                    if &d.severity != severity {
                        return false;
                    }
                }

                // Filter by search query
                if let Some(q) = &query {
                    return d.message.to_lowercase().contains(q) ||
                           d.file_path.to_lowercase().contains(q) ||
                           d.source.as_ref().map_or(false, |s| s.to_lowercase().contains(q));
                }

                true
            })
            .cloned()
            .collect()
    }

    fn get_grouped_diagnostics(&self) -> HashMap<String, Vec<Diagnostic>> {
        let diagnostics = self.get_filtered_diagnostics();
        let mut grouped: HashMap<String, Vec<Diagnostic>> = HashMap::new();

        for diagnostic in diagnostics {
            grouped
                .entry(diagnostic.file_path.clone())
                .or_insert_with(Vec::new)
                .push(diagnostic);
        }

        grouped
    }

    pub fn count_by_severity(&self, severity: DiagnosticSeverity) -> usize {
        let diagnostics = self.diagnostics.lock().unwrap();
        diagnostics.iter().filter(|d| d.severity == severity).count()
    }

    pub fn total_count(&self) -> usize {
        self.diagnostics.lock().unwrap().len()
    }

    fn set_filter(&mut self, severity: Option<DiagnosticSeverity>, cx: &mut Context<Self>) {
        self.filtered_severity = severity;
        self.selected_index = None;
        cx.notify();
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.selected_index = None;
        cx.notify();
    }

    fn toggle_grouping(&mut self, cx: &mut Context<Self>) {
        self.group_by_file = !self.group_by_file;
        cx.notify();
    }

    fn select_diagnostic(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_index = Some(index);
        cx.notify();
    }

    fn navigate_to_diagnostic(&mut self, diagnostic: &Diagnostic, cx: &mut Context<Self>) {
        let file_path = PathBuf::from(&diagnostic.file_path);
        cx.emit(NavigateToDiagnostic {
            file_path,
            line: diagnostic.line,
            column: diagnostic.column,
        });
    }

    // Action handlers for filter menu
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
        // Update search query from input state
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

        // Pre-render content area based on state
        let content: AnyElement = if filtered_diagnostics.is_empty() {
            self.render_empty_state(cx).into_any_element()
        } else if group_by_file {
            self.render_grouped_view(selected_index, window, cx).into_any_element()
        } else {
            self.render_flat_view(filtered_diagnostics, selected_index, window, cx).into_any_element()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Register action handlers for filter menu
            .on_action(cx.listener(Self::on_filter_all))
            .on_action(cx.listener(Self::on_filter_errors))
            .on_action(cx.listener(Self::on_filter_warnings))
            .on_action(cx.listener(Self::on_filter_info))
            // Professional header with search
            .child(self.render_header(error_count, warning_count, info_count, total_count, cx))
            // Main content area - flex_1 + overflow_hidden to constrain scroll
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(content)
            )
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

        v_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            // Top row: Title, stats, and actions
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
                                    .child(t!("Problems.Title").to_string())
                            )
                            // Severity counts with professional styling
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .when(error_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Error,
                                            error_count,
                                            cx
                                        ))
                                    })
                                    .when(warning_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Warning,
                                            warning_count,
                                            cx
                                        ))
                                    })
                                    .when(info_count > 0, |this| {
                                        this.child(self.render_severity_badge(
                                            DiagnosticSeverity::Information,
                                            info_count,
                                            cx
                                        ))
                                    })
                            )
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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_grouping(cx);
                                    }))
                            )
                            .child(
                                Button::new("clear-all")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .tooltip(t!("Problems.Action.ClearAll").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_diagnostics(cx);
                                    }))
                            )
                    )
            )
            // Search and filter row
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    // Functional search bar
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.0))
                            .child(
                                TextInput::new(&self.search_input)
                                    .w_full()
                                    .prefix(
                                        ui::Icon::new(IconName::Search)
                                            .size_4()
                                            .text_color(cx.theme().muted_foreground)
                                    )
                            )
                    )
                    // Filter dropdown button using proper PopupMenu
                    .child({
                        let is_all_selected = self.filtered_severity.is_none();
                        let is_errors_selected = self.filtered_severity == Some(DiagnosticSeverity::Error);
                        let is_warnings_selected = self.filtered_severity == Some(DiagnosticSeverity::Warning);
                        let is_info_selected = self.filtered_severity == Some(DiagnosticSeverity::Information);

                        Button::new("filter-dropdown")
                            .ghost()
                            .small()
                            .icon(IconName::Filter)
                            .label(current_filter_label.clone())
                            .popup_menu_with_anchor(Corner::BottomRight, move |menu, _window, _cx| {
                                menu.menu_with_check(t!("Problems.Filter.All").to_string(), is_all_selected, Box::new(FilterAll))
                                    .separator()
                                    .menu_with_check(t!("Problems.Filter.Errors").to_string(), is_errors_selected, Box::new(FilterErrors))
                                    .menu_with_check(t!("Problems.Filter.Warnings").to_string(), is_warnings_selected, Box::new(FilterWarnings))
                                    .menu_with_check(t!("Problems.Filter.Information").to_string(), is_info_selected, Box::new(FilterInfo))
                            })
                    })
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
                    .text_color(severity.color(cx))
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(severity.color(cx))
                    .child(count.to_string())
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
                                        .text_color(cx.theme().success)
                                )
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child("No problems detected")
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
                                })
                        )
                )
        )
    }

    fn render_flat_view(
        &mut self,
        filtered_diagnostics: Vec<Diagnostic>,
        selected_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let drawer_entity = cx.entity().clone();
        
        // Pre-render all diagnostic items with mutable access
        let items: Vec<Div> = filtered_diagnostics
            .into_iter()
            .enumerate()
            .map(|(index, diagnostic)| {
                let is_selected = selected_index == Some(index);
                let drawer = drawer_entity.clone();
                let diag = diagnostic.clone();

                self.render_diagnostic_item(index, diagnostic, is_selected, move |_window, cx| {
                    drawer.update(cx, |drawer, cx| {
                        drawer.select_diagnostic(index, cx);
                        drawer.navigate_to_diagnostic(&diag, cx);
                    });
                }, window, cx)
            })
            .collect();
        
        div()
            .id("problems-scroll-container")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .children(items)
            )
    }

    fn render_grouped_view(
        &mut self,
        selected_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let grouped = self.get_grouped_diagnostics();
        let mut files: Vec<_> = grouped.keys().cloned().collect();
        files.sort();

        let drawer_entity = cx.entity().clone();
        let mut global_index = 0;

        // Pre-build all file groups with their diagnostic items
        let mut file_groups: Vec<Div> = Vec::new();
        
        for file_path in files {
            let diagnostics = grouped.get(&file_path).unwrap();
            let file_error_count = diagnostics.iter().filter(|d| matches!(d.severity, DiagnosticSeverity::Error)).count();
            let file_warning_count = diagnostics.iter().filter(|d| matches!(d.severity, DiagnosticSeverity::Warning)).count();

            // Compute display path for this file
            let display_path = self.get_display_path(&file_path);

            // Pre-render diagnostic items for this file
            let items: Vec<Div> = diagnostics.iter().map(|diagnostic| {
                let is_selected = selected_index == Some(global_index);
                let drawer = drawer_entity.clone();
                // Clone once for the closure
                let diag = diagnostic.clone();
                let idx = global_index;
                global_index += 1;

                // Move diag into closure instead of cloning again
                self.render_diagnostic_item(idx, diag.clone(), is_selected, move |_window, cx| {
                    drawer.update(cx, |drawer, cx| {
                        drawer.select_diagnostic(idx, cx);
                        drawer.navigate_to_diagnostic(&diag, cx);
                    });
                }, window, cx)
            }).collect();

            let file_group = v_flex()
                .w_full()
                .px_3()
                .child(
                    // File header - styled like a section header
                    div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .mb_2()
                        .rounded_md()
                        .bg(cx.theme().secondary.opacity(0.3))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.3))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_3()
                                .items_center()
                                .child(
                                    ui::Icon::new(IconName::Folder)
                                        .size_4()
                                        .text_color(cx.theme().accent)
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().foreground)
                                        .child(display_path.clone())
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when(file_error_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(DiagnosticSeverity::Error.color(cx).opacity(0.15))
                                                    .child(
                                                        ui::Icon::new(IconName::Close)
                                                            .size_3()
                                                            .text_color(DiagnosticSeverity::Error.color(cx))
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(DiagnosticSeverity::Error.color(cx))
                                                            .child(file_error_count.to_string())
                                                    )
                                            )
                                        })
                                        .when(file_warning_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(DiagnosticSeverity::Warning.color(cx).opacity(0.15))
                                                    .child(
                                                        ui::Icon::new(IconName::TriangleAlert)
                                                            .size_3()
                                                            .text_color(DiagnosticSeverity::Warning.color(cx))
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(DiagnosticSeverity::Warning.color(cx))
                                                            .child(file_warning_count.to_string())
                                                    )
                                            )
                                        })
                                )
                        )
                )
                .children(items);
            
            file_groups.push(file_group);
        }

        div()
            .id("problems-scroll-container-grouped")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .children(file_groups)
            )
    }

    fn render_diagnostic_item<F>(
        &mut self,
        diagnostic_index: usize,
        diagnostic: Diagnostic,
        is_selected: bool,
        on_click: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        let on_click = Arc::new(on_click);

        let mut main = v_flex()
            .gap_2()
            .w_full()
            // Severity and location
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .w_full()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                ui::Icon::new(diagnostic.severity.icon())
                                    .size_4()
                                    .text_color(diagnostic.severity.color(cx))
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(diagnostic.severity.color(cx))
                                    .child(diagnostic.severity.label())
                            )
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_family("monospace")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{}:{}",
                                diagnostic.line,
                                diagnostic.column
                            ))
                    )
                    .when_some(diagnostic.source.as_ref(), |this, source| {
                        this.child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(cx.theme().border)
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(source.clone())
                        )
                    })
            )
            // Message
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .line_height(rems(1.4))
                    .child(diagnostic.message.clone())
            );

        // Show loading indicator while fetching code actions
        if diagnostic.loading_actions {
            main = main.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .mt_2()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .child(
                        Indicator::new()
                            .with_size(ui::Size::Small)
                            .color(cx.theme().muted_foreground)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("Problems.Loading").to_string())
                    )
            );
        }

        // Render hints with side-by-side diff editors
        tracing::debug!("🎨 Rendering diagnostic {}: hints={}, loading={}", 
            diagnostic_index, diagnostic.hints.len(), diagnostic.loading_actions);
        
        if !diagnostic.hints.is_empty() && !diagnostic.loading_actions {
            tracing::debug!("🎨 Rendering {} hints for diagnostic {}", diagnostic.hints.len(), diagnostic_index);
            let mut hints_container = v_flex()
                .gap_2()
                .w_full()
                .mt_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("Problems.SuggestedFixes").to_string())
                );
            
            for (hint_index, hint) in diagnostic.hints.iter().enumerate() {
                tracing::debug!("🎨 Rendering hint {}: before={} chars, after={} chars",
                    hint_index,
                    hint.before_content.as_ref().map(|s| s.len()).unwrap_or(0),
                    hint.after_content.as_ref().map(|s| s.len()).unwrap_or(0));
                let hint_el = self.render_hint_diff(diagnostic_index, hint_index, hint, window, cx);
                hints_container = hints_container.child(hint_el);
            }
            
            main = main.child(hints_container);
        }

        // Render subitems inline (one level only, no recursion)
        if !diagnostic.subitems.is_empty() {
            let mut subitems_container = v_flex()
                .gap_1()
                .w_full()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("Problems.Related").to_string())
                );
            
            for sub in &diagnostic.subitems {
                let subitem_el = div()
                    .pl_4()
                    .py_1()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(sub.severity.icon())
                                            .size_3()
                                            .text_color(sub.severity.color(cx))
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{}:{}", sub.line, sub.column))
                                    )
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(sub.message.clone())
                            )
                    );
                
                subitems_container = subitems_container.child(subitem_el);
            }
            
            main = main.child(subitems_container);
        }

        div()
            .w_full()
            .px_3()
            .py_2()
            .child(
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(if is_selected {
                        cx.theme().accent
                    } else {
                        cx.theme().border.opacity(0.5)
                    })
                    .bg(if is_selected {
                        cx.theme().accent.opacity(0.08)
                    } else {
                        cx.theme().sidebar.opacity(0.5)
                    })
                    .shadow_sm()
                    .when(is_selected, |this| {
                        this.border_l_3()
                            .border_color(cx.theme().accent)
                    })
                    .hover(|this| {
                        this.bg(cx.theme().secondary.opacity(0.7))
                            .border_color(cx.theme().accent.opacity(0.5))
                    })
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, move |_, _window, cx| {
                        on_click(_window, cx);
                    })
                    .child(main)
            )
    }

    /// Render a hint with side-by-side diff view (before/after)
    /// Uses line-by-line diff with highlighting and spacers for alignment
    fn render_hint_diff(
        &mut self,
        _diagnostic_index: usize,
        hint_index: usize,
        hint: &Hint,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        // If we don't have diff content, just show the message
        if hint.before_content.is_none() && hint.after_content.is_none() {
            tracing::debug!("🎨 Hint {} has no diff content, showing message only", hint_index);
            return v_flex()
                .gap_1()
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().sidebar)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(format!("💡 {}", hint.message))
                );
        }
        
        // Get the content for before/after
        let before_content = hint.before_content.clone().unwrap_or_default();
        let after_content = hint.after_content.clone().unwrap_or_default();
        
        // Compute the aligned diff lines
        let (left_lines, right_lines) = compute_aligned_diff(&before_content, &after_content);
        
        // Colors for diff highlighting
        let deleted_bg = Hsla { h: 0.0, s: 0.4, l: 0.15, a: 1.0 };      // Dark red background
        let deleted_text = Hsla { h: 0.0, s: 0.7, l: 0.7, a: 1.0 };     // Light red text
        let added_bg = Hsla { h: 120.0, s: 0.4, l: 0.15, a: 1.0 };      // Dark green background
        let added_text = Hsla { h: 120.0, s: 0.7, l: 0.7, a: 1.0 };     // Light green text
        let spacer_bg = Hsla { h: 0.0, s: 0.0, l: 0.12, a: 1.0 };       // Dark gray for spacers
        let unchanged_bg = cx.theme().sidebar;
        let unchanged_text = cx.theme().foreground;
        let line_num_color = cx.theme().muted_foreground;
        
        // Build left side (before) lines
        let mut left_container = v_flex().w_full();
        for (line_num, line_type, content) in &left_lines {
            let (bg, text_color) = match line_type {
                DiffLineType::Deleted => (deleted_bg, deleted_text),
                DiffLineType::Spacer => (spacer_bg, line_num_color),
                DiffLineType::Unchanged => (unchanged_bg, unchanged_text),
                _ => (unchanged_bg, unchanged_text),
            };
            
            left_container = left_container.child(
                h_flex()
                    .w_full()
                    .h(px(20.0))
                    .bg(bg)
                    .child(
                        div()
                            .w(px(40.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(line_num_color)
                            .child(if *line_type == DiffLineType::Spacer { 
                                "".to_string() 
                            } else { 
                                line_num.map(|n| n.to_string()).unwrap_or_default()
                            })
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .pl_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(text_color)
                            .overflow_x_hidden()
                            .child(content.clone())
                    )
            );
        }
        
        // Build right side (after) lines
        let mut right_container = v_flex().w_full();
        for (line_num, line_type, content) in &right_lines {
            let (bg, text_color) = match line_type {
                DiffLineType::Added => (added_bg, added_text),
                DiffLineType::Spacer => (spacer_bg, line_num_color),
                DiffLineType::Unchanged => (unchanged_bg, unchanged_text),
                _ => (unchanged_bg, unchanged_text),
            };
            
            right_container = right_container.child(
                h_flex()
                    .w_full()
                    .h(px(20.0))
                    .bg(bg)
                    .child(
                        div()
                            .w(px(40.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(line_num_color)
                            .child(if *line_type == DiffLineType::Spacer { 
                                "".to_string() 
                            } else { 
                                line_num.map(|n| n.to_string()).unwrap_or_default()
                            })
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .pl_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(text_color)
                            .overflow_x_hidden()
                            .child(content.clone())
                    )
            );
        }
        
        // Calculate the total height based on number of lines
        let total_lines = left_lines.len().max(right_lines.len());
        let content_height = (total_lines as f32 * 20.0).max(40.0);
        
        // Build the hint element with enhanced styling
        v_flex()
            .gap_0()
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().accent.opacity(0.3))
            .bg(cx.theme().background.opacity(0.5))
            .overflow_hidden()
            .shadow_md()
            // Hint message header with accent
            .child(
                div()
                    .px_4()
                    .py_2p5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().accent.opacity(0.05))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                ui::Icon::new(IconName::Info)
                                    .size_4()
                                    .text_color(cx.theme().accent)
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child(hint.message.clone())
                            )
                    )
            )
            // Side-by-side diff headers with improved styling
            .child(
                h_flex()
                    .w_full()
                    .child(
                        div()
                            .w_1_2()
                            .min_w_0()
                            .px_3()
                            .py_1p5()
                            .border_r_1()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.6))
                            .bg(Hsla { h: 0.0, s: 0.4, l: 0.12, a: 1.0 })
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(IconName::Close)
                                            .size_3()
                                            .text_color(Hsla { h: 0.0, s: 0.8, l: 0.6, a: 1.0 })
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(Hsla { h: 0.0, s: 0.7, l: 0.65, a: 1.0 })
                                            .child(t!("Problems.Before").to_string())
                                    )
                            )
                    )
                    .child(
                        div()
                            .w_1_2()
                            .min_w_0()
                            .px_3()
                            .py_1p5()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.6))
                            .bg(Hsla { h: 120.0, s: 0.4, l: 0.12, a: 1.0 })
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(IconName::Check)
                                            .size_3()
                                            .text_color(Hsla { h: 120.0, s: 0.8, l: 0.5, a: 1.0 })
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(Hsla { h: 120.0, s: 0.7, l: 0.55, a: 1.0 })
                                            .child(t!("Problems.After").to_string())
                                    )
                            )
                    )
            )
            // Side-by-side diff content
            .child(
                div()
                    .id("diff-scroll-container")
                    .w_full()
                    .h(px(content_height))
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .child(
                        h_flex()
                            .w_full()
                            .child(
                                div()
                                    .w_1_2()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .border_r_1()
                                    .border_color(cx.theme().border.opacity(0.4))
                                    .child(left_container)
                            )
                            .child(
                                div()
                                    .w_1_2()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(right_container)
                            )
                    )
            )
    }

    fn render_file_preview(&mut self, diagnostic: &Diagnostic, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Try to read the file and show a few lines around the error
        let context_lines = 2; // Number of lines before and after the error line
        
        if let Ok(content) = fs::read_to_string(&diagnostic.file_path) {
            let lines: Vec<&str> = content.lines().collect();
            let error_line = diagnostic.line.saturating_sub(1); // Convert to 0-indexed
            
            if error_line < lines.len() {
                let start_line = error_line.saturating_sub(context_lines);
                let end_line = (error_line + context_lines + 1).min(lines.len());
                
                // Build the preview content with line numbers
                let preview_content: String = lines[start_line..end_line]
                    .iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                
                // Get or create the InputState for this preview
                let key = (diagnostic.file_path.clone(), diagnostic.line);
                let input_state = if let Some(existing) = self.preview_inputs.get(&key) {
                    existing.clone()
                } else {
                    // Calculate number of visible rows
                    let num_lines = end_line - start_line;
                    
                    // Determine language from file extension
                    let language = std::path::Path::new(&diagnostic.file_path)
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| match ext {
                            "rs" => "rust",
                            "js" => "javascript",
                            "ts" => "typescript",
                            "py" => "python",
                            "toml" => "toml",
                            "json" => "json",
                            "md" => "markdown",
                            "html" => "html",
                            "css" => "css",
                            _ => "text",
                        })
                        .unwrap_or("text");
                    
                    let new_state = cx.new(|cx| {
                        let mut state = InputState::new(window, cx)
                            .code_editor(language)
                            .soft_wrap(false)
                            .rows(num_lines);
                        state.set_value(&preview_content, window, cx);
                        state
                    });
                    self.preview_inputs.insert(key, new_state.clone());
                    new_state
                };
                
                // Calculate height to match diff view (20px per line + some padding)
                let num_lines = end_line - start_line;
                let calculated_height = num_lines as f32 * 20.0 + 16.0; // Match diff view line height
                
                return div()
                    .w_full()
                    .mt_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .overflow_hidden()
                    .child(
                        TextInput::new(&input_state)
                            .w_full()
                            .h(px(calculated_height))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .border_0()
                    );
            }
        }
        
        // Return empty div if file can't be read
        div()
    }
}
