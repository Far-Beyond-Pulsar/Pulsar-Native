use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use gpui::{prelude::*, *};
use ui::{input::InputState, v_flex, ActiveTheme as _};

use crate::utils::types::{Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic};

/// Lock policy: recover from poisoning. The diagnostics list is always replaced or
/// mutated element-wise, so a panicking holder cannot leave it inconsistent.
pub(crate) fn lock_diagnostics(m: &Mutex<Vec<Diagnostic>>) -> MutexGuard<'_, Vec<Diagnostic>> {
    m.lock().unwrap_or_else(|poisoned| {
        tracing::error!("problems diagnostics mutex was poisoned by a panic; recovering");
        poisoned.into_inner()
    })
}

/// What one source (a language server, a compiler, a play session) last
/// published.
#[derive(Default)]
pub(crate) struct SourceSlot {
    /// The set as the source published it, before fetched code-action hints
    /// were added. A republish of the same set is compared against this, not
    /// against the enriched copy (which always differs from a fresh publish).
    pub(crate) published: Vec<Diagnostic>,
    /// The set with the hints fetched since.
    pub(crate) current: Vec<Diagnostic>,
}

pub struct ProblemsDrawer {
    pub(crate) focus_handle: FocusHandle,
    /// Every source's diagnostics, in source-name order: what the drawer shows
    /// (the severity and source filters and the search apply to it).
    pub(crate) diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
    /// Each source's own slot. A source publishes its complete set (an empty
    /// set removes its slot), so several files from several sources are shown
    /// at once and a fixed one goes.
    pub(crate) sources: BTreeMap<String, SourceSlot>,
    pub(crate) filtered_severity: Option<DiagnosticSeverity>,
    /// Show only this source's diagnostics.
    pub(crate) filtered_source: Option<String>,
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
            sources: BTreeMap::new(),
            filtered_severity: None,
            filtered_source: None,
            selected_index: None,
            search_query: String::new(),
            group_by_file: true,
            preview_inputs: HashMap::new(),
            diff_editors: HashMap::new(),
            search_input,
            project_root: None,
        }
    }

    pub fn clear_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.sources.clear();
        self.filtered_source = None;
        self.rebuild();
        self.selected_index = None;
        self.preview_inputs.clear();
        self.diff_editors.clear();
        cx.notify();
    }

    /// The shown list again from the slots, in source-name order.
    fn rebuild(&mut self) {
        let all: Vec<Diagnostic> = self
            .sources
            .values()
            .flat_map(|slot| slot.current.iter().cloned())
            .collect();
        *lock_diagnostics(&self.diagnostics) = all;
    }

    /// Replace everything `source` publishes (empty removes it). Returns
    /// whether anything changed.
    ///
    /// A server republishes the same set repeatedly. Treating each publish as
    /// a change reset the selection and caches and notified the drawer (and,
    /// through the caller, the whole app) several times a second with
    /// nothing new to show -- each notify turning a ~0.5 ms display-only
    /// frame into a multi-millisecond full draw.
    pub fn set_source_diagnostics(
        &mut self,
        source: &str,
        diagnostics: Vec<Diagnostic>,
        cx: &mut Context<Self>,
    ) -> bool {
        let unchanged = match self.sources.get(source) {
            Some(slot) => slot.published == diagnostics,
            None => diagnostics.is_empty(),
        };
        if unchanged {
            return false;
        }
        if diagnostics.is_empty() {
            self.sources.remove(source);
            if self.filtered_source.as_deref() == Some(source) {
                self.filtered_source = None;
            }
        } else {
            self.sources.insert(
                source.to_owned(),
                SourceSlot {
                    published: diagnostics.clone(),
                    current: diagnostics,
                },
            );
        }
        self.rebuild();
        self.selected_index = None;
        self.preview_inputs.clear();
        self.diff_editors.clear();
        cx.notify();
        true
    }

    /// Add fetched code-action `new_hints` to the `index`th diagnostic of
    /// `source`, and clear its loading indicator. Only a real change is worth
    /// a redraw: new hints, or clearing a loading indicator that was showing.
    pub fn update_source_hints(
        &mut self,
        source: &str,
        index: usize,
        new_hints: Vec<Hint>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.sources.get_mut(source) else {
            return;
        };
        let Some(diagnostic) = slot.current.get_mut(index) else {
            return;
        };
        let mut changed = false;
        if !new_hints.is_empty() {
            diagnostic.hints.extend(new_hints);
            changed = true;
        }
        if diagnostic.loading_actions {
            diagnostic.loading_actions = false;
            changed = true;
        }
        if !changed {
            return;
        }
        self.rebuild();
        self.diff_editors.clear();
        cx.notify();
    }

    /// The sources that currently have something to show.
    pub fn source_names(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    /// The number of diagnostics of `source`.
    pub fn source_count(&self, source: &str) -> usize {
        self.sources
            .get(source)
            .map_or(0, |slot| slot.current.len())
    }

    pub fn get_diagnostic_info(
        &self,
        index: usize,
    ) -> Option<(String, usize, usize, usize, usize)> {
        lock_diagnostics(&self.diagnostics).get(index).map(|d| {
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
            .on_action(cx.listener(crate::handlers::on_filter_source))
            .on_action(cx.listener(crate::handlers::on_clear_source_filter))
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

#[cfg(test)]
mod tests {
    use super::{lock_diagnostics, Diagnostic, ProblemsDrawer};
    use std::sync::{Arc, Mutex};

    #[test]
    fn lock_diagnostics_recovers_from_a_poisoned_mutex() {
        let diagnostics = Arc::new(Mutex::new(Vec::<Diagnostic>::new()));
        let poisoner = Arc::clone(&diagnostics);
        let result = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the diagnostics");
        })
        .join();
        assert!(result.is_err());
        assert!(diagnostics.is_poisoned());

        assert!(lock_diagnostics(&diagnostics).is_empty());
    }

    fn problem(file: &str, message: &str) -> Diagnostic {
        Diagnostic {
            file_path: file.into(),
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
            severity: crate::DiagnosticSeverity::Error,
            message: message.into(),
            source: None,
            hints: Vec::new(),
            subitems: Vec::new(),
            loading_actions: false,
        }
    }

    struct Host;

    impl gpui::Render for Host {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    /// The drawer showed only the last file a server published and never
    /// cleared a fixed one. A source now publishes its complete set: two
    /// files are shown at once, and fixing one removes only it.
    #[gpui::test]
    fn several_files_show_at_once_and_a_fixed_file_clears(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let (_host, cx) = cx.add_window_view(|_, _| Host);
        let drawer = cx.update(|window, app| {
            ui::init(app);
            app.new(|cx| ProblemsDrawer::new(window, cx))
        });
        drawer.update(cx, |drawer, cx| {
            assert!(drawer.set_source_diagnostics(
                "rust-analyzer",
                vec![problem("/p/a.rs", "in a"), problem("/p/b.rs", "in b")],
                cx
            ));
            assert_eq!(drawer.total_count(), 2);
            let files = drawer.get_grouped_diagnostics();
            assert_eq!(files.len(), 2, "both files are listed");

            // The same set again is not a change.
            assert!(!drawer.set_source_diagnostics(
                "rust-analyzer",
                vec![problem("/p/a.rs", "in a"), problem("/p/b.rs", "in b")],
                cx
            ));

            // a.rs was fixed: the server's next complete set no longer has it.
            assert!(drawer.set_source_diagnostics("rust-analyzer", vec![problem("/p/b.rs", "in b")], cx));
            assert_eq!(drawer.total_count(), 1);
            assert_eq!(drawer.get_grouped_diagnostics().len(), 1);

            // All fixed: the source disappears.
            assert!(drawer.set_source_diagnostics("rust-analyzer", Vec::new(), cx));
            assert_eq!(drawer.total_count(), 0);
            assert!(drawer.source_names().is_empty());
        });
    }

    #[gpui::test]
    fn sources_are_independent_and_filterable(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let (_host, cx) = cx.add_window_view(|_, _| Host);
        let drawer = cx.update(|window, app| {
            ui::init(app);
            app.new(|cx| ProblemsDrawer::new(window, cx))
        });
        drawer.update(cx, |drawer, cx| {
            drawer.set_source_diagnostics("rust-analyzer", vec![problem("/p/a.rs", "ra")], cx);
            drawer.set_source_diagnostics("typescript compiler", vec![problem("/p/c.ts", "ts")], cx);
            assert_eq!(drawer.source_names(), ["rust-analyzer", "typescript compiler"]);
            assert_eq!(drawer.total_count(), 2);

            // One source's republish does not touch the other's.
            drawer.set_source_diagnostics("rust-analyzer", Vec::new(), cx);
            assert_eq!(drawer.source_names(), ["typescript compiler"]);

            drawer.set_source_diagnostics("rust-analyzer", vec![problem("/p/a.rs", "ra")], cx);
            drawer.set_source_filter(Some("typescript compiler".into()), cx);
            let shown = drawer.get_filtered_diagnostics();
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].message, "ts");
            drawer.set_source_filter(None, cx);
            assert_eq!(drawer.get_filtered_diagnostics().len(), 2);

            // A source that empties while it is the filter drops the filter.
            drawer.set_source_filter(Some("rust-analyzer".into()), cx);
            drawer.set_source_diagnostics("rust-analyzer", Vec::new(), cx);
            assert_eq!(drawer.get_filtered_diagnostics().len(), 1);
        });
    }

    #[gpui::test]
    fn fetched_hints_attach_to_their_diagnostic_and_survive_other_sources_changing(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext as _;
        let (_host, cx) = cx.add_window_view(|_, _| Host);
        let drawer = cx.update(|window, app| {
            ui::init(app);
            app.new(|cx| ProblemsDrawer::new(window, cx))
        });
        drawer.update(cx, |drawer, cx| {
            let mut loading = problem("/p/a.rs", "ra");
            loading.loading_actions = true;
            drawer.set_source_diagnostics("rust-analyzer", vec![loading], cx);
            drawer.update_source_hints(
                "rust-analyzer",
                0,
                vec![crate::Hint {
                    message: "fix".into(),
                    before_content: None,
                    after_content: None,
                    file_path: None,
                    line: None,
                    loading: false,
                }],
                cx,
            );
            drawer.set_source_diagnostics("play", vec![problem("Class", "boom")], cx);
            let shown = drawer.get_filtered_diagnostics();
            let ra = shown.iter().find(|d| d.message == "ra").unwrap();
            assert_eq!(ra.hints.len(), 1);
            assert!(!ra.loading_actions);
        });
    }
}
