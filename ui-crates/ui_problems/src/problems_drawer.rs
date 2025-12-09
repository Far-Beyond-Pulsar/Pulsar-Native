//! Problems Drawer - Studio-quality diagnostics panel
//! Displays rust-analyzer diagnostics with professional UI and search capabilities

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _, ButtonVariant},
    h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

// Local diagnostic types
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
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
}

impl ProblemsDrawer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));

        Self {
            focus_handle,
            diagnostics,
            filtered_severity: None,
            selected_index: None,
            search_query: String::new(),
            group_by_file: true,
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
        cx.notify();
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        {
            let mut diag = self.diagnostics.lock().unwrap();
            *diag = diagnostics;
        }
        self.selected_index = None;
        cx.notify();
    }

    fn get_filtered_diagnostics(&self) -> Vec<Diagnostic> {
        let diagnostics = self.diagnostics.lock().unwrap().clone();

        let mut filtered = diagnostics;

        // Filter by severity
        if let Some(severity) = &self.filtered_severity {
            filtered.retain(|d| &d.severity == severity);
        }

        // Filter by search query
        if !self.search_query.is_empty() {
            let query = self.search_query.to_lowercase();
            filtered.retain(|d| {
                d.message.to_lowercase().contains(&query) ||
                d.file_path.to_lowercase().contains(&query) ||
                d.source.as_ref().map_or(false, |s| s.to_lowercase().contains(&query))
            });
        }

        filtered
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
}

impl Focusable for ProblemsDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProblemsDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error_count = self.count_by_severity(DiagnosticSeverity::Error);
        let warning_count = self.count_by_severity(DiagnosticSeverity::Warning);
        let info_count = self.count_by_severity(DiagnosticSeverity::Information);
        let total_count = self.total_count();

        let filtered_diagnostics = self.get_filtered_diagnostics();
        let selected_index = self.selected_index;
        let group_by_file = self.group_by_file;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header with search
            .child(self.render_header(error_count, warning_count, info_count, total_count, cx))
            // Main content area
            .child(
                div()
                    .flex_1()
                    .when(filtered_diagnostics.is_empty(), |container| {
                        self.render_empty_state(container, cx)
                    })
                    .when(!filtered_diagnostics.is_empty(), |container| {
                        if group_by_file {
                            self.render_grouped_view(container, selected_index, cx)
                        } else {
                            self.render_flat_view(container, filtered_diagnostics, selected_index, cx)
                        }
                    })
            )
    }
}

impl ProblemsDrawer {
    fn render_header(
        &self,
        error_count: usize,
        warning_count: usize,
        info_count: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            // Top row: Title and actions
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
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child("Problems")
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
                            .gap_1()
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
                                        "Show flat list"
                                    } else {
                                        "Group by file"
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
                                    .tooltip("Clear all problems")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_diagnostics(cx);
                                    }))
                            )
                    )
            )
            // Search bar
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .items_center()
                    .child(
                        ui::Icon::new(IconName::Search)
                            .size_4()
                            .text_color(cx.theme().muted_foreground)
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(if self.search_query.is_empty() {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(if self.search_query.is_empty() {
                                "Search problems...".to_string()
                            } else {
                                self.search_query.clone()
                            })
                    )
            )
            // Filter chips
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new("filter-all")
                            .small()
                            .when(self.filtered_severity.is_none(), |btn| {
                                btn.with_variant(ButtonVariant::Primary)
                            })
                            .when(self.filtered_severity.is_some(), |btn| {
                                btn.ghost()
                            })
                            .label(format!("All ({})", total_count))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(None, cx);
                            }))
                    )
                    .child(
                        Button::new("filter-errors")
                            .small()
                            .when(
                                self.filtered_severity == Some(DiagnosticSeverity::Error),
                                |btn| btn.with_variant(ButtonVariant::Danger)
                            )
                            .when(
                                self.filtered_severity != Some(DiagnosticSeverity::Error),
                                |btn| btn.ghost()
                            )
                            .icon(IconName::Close)
                            .label(error_count.to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(DiagnosticSeverity::Error), cx);
                            }))
                    )
                    .child(
                        Button::new("filter-warnings")
                            .small()
                            .when(
                                self.filtered_severity == Some(DiagnosticSeverity::Warning),
                                |btn| btn.with_variant(ButtonVariant::Warning)
                            )
                            .when(
                                self.filtered_severity != Some(DiagnosticSeverity::Warning),
                                |btn| btn.ghost()
                            )
                            .icon(IconName::TriangleAlert)
                            .label(warning_count.to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(DiagnosticSeverity::Warning), cx);
                            }))
                    )
                    .child(
                        Button::new("filter-info")
                            .small()
                            .when(
                                self.filtered_severity == Some(DiagnosticSeverity::Information),
                                |btn| btn.with_variant(ButtonVariant::Primary)
                            )
                            .when(
                                self.filtered_severity != Some(DiagnosticSeverity::Information),
                                |btn| btn.ghost()
                            )
                            .icon(IconName::Info)
                            .label(info_count.to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filter(Some(DiagnosticSeverity::Information), cx);
                            }))
                    )
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

    fn render_empty_state(&self, container: Div, cx: &App) -> Div {
        container.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            ui::Icon::new(IconName::Check)
                                .size(px(48.0))
                                .text_color(cx.theme().success)
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("No problems detected")
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if !self.search_query.is_empty() {
                                    "Try a different search query"
                                } else {
                                    "Your code is looking good!"
                                })
                        )
                )
        )
    }

    fn render_flat_view(
        &self,
        container: Div,
        filtered_diagnostics: Vec<Diagnostic>,
        selected_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let drawer_entity = cx.entity().clone();
        container.child(
            v_flex()
                .w_full()
                .children(
                    filtered_diagnostics
                        .into_iter()
                        .enumerate()
                        .map(|(index, diagnostic)| {
                            let is_selected = selected_index == Some(index);
                            let drawer = drawer_entity.clone();
                            let diag = diagnostic.clone();

                            self.render_diagnostic_item(diagnostic, is_selected, move |_window, cx| {
                                drawer.update(cx, |drawer, cx| {
                                    drawer.select_diagnostic(index, cx);
                                    drawer.navigate_to_diagnostic(&diag, cx);
                                });
                            }, cx)
                        })
                )
        )
    }

    fn render_grouped_view(
        &self,
        container: Div,
        selected_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let grouped = self.get_grouped_diagnostics();
        let mut files: Vec<_> = grouped.keys().collect();
        files.sort();

        let drawer_entity = cx.entity().clone();
        let mut global_index = 0;

        container.child(
            v_flex()
                .w_full()
                .children(files.into_iter().map(|file_path| {
                    let diagnostics = grouped.get(file_path).unwrap();
                    let file_error_count = diagnostics.iter().filter(|d| matches!(d.severity, DiagnosticSeverity::Error)).count();
                    let file_warning_count = diagnostics.iter().filter(|d| matches!(d.severity, DiagnosticSeverity::Warning)).count();

                    let items: Vec<_> = diagnostics.iter().map(|diagnostic| {
                        let is_selected = selected_index == Some(global_index);
                        let drawer = drawer_entity.clone();
                        let diag = diagnostic.clone();
                        let idx = global_index;
                        global_index += 1;

                        self.render_diagnostic_item(diagnostic.clone(), is_selected, move |_window, cx| {
                            drawer.update(cx, |drawer, cx| {
                                drawer.select_diagnostic(idx, cx);
                                drawer.navigate_to_diagnostic(&diag, cx);
                            });
                        }, cx)
                    }).collect();

                    v_flex()
                        .w_full()
                        .child(
                            // File header
                            h_flex()
                                .w_full()
                                .px_4()
                                .py_2()
                                .gap_2()
                                .items_center()
                                .bg(cx.theme().sidebar)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    ui::Icon::new(IconName::Folder)
                                        .size_3()
                                        .text_color(cx.theme().muted_foreground)
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(cx.theme().foreground)
                                        .child(file_path.clone())
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when(file_error_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .child(
                                                        ui::Icon::new(IconName::Close)
                                                            .size_3()
                                                            .text_color(DiagnosticSeverity::Error.color(cx))
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(file_error_count.to_string())
                                                    )
                                            )
                                        })
                                        .when(file_warning_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .child(
                                                        ui::Icon::new(IconName::TriangleAlert)
                                                            .size_3()
                                                            .text_color(DiagnosticSeverity::Warning.color(cx))
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(file_warning_count.to_string())
                                                    )
                                            )
                                        })
                                )
                        )
                        .children(items)
                }))
        )
    }

    fn render_diagnostic_item<F>(
        &self,
        diagnostic: Diagnostic,
        is_selected: bool,
        on_click: F,
        cx: &App,
    ) -> impl IntoElement
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        let on_click = Arc::new(on_click);

        div()
            .w_full()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(is_selected, |this| {
                this.bg(cx.theme().selection)
                    .border_l_2()
                    .border_color(cx.theme().accent)
            })
            .hover(|this| this.bg(cx.theme().secondary))
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, move |_, _window, cx| {
                on_click(_window, cx);
            })
            .child(
                v_flex()
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
                    )
            )
    }
}
