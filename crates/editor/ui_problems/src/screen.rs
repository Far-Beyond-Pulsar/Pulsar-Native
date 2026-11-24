//! The Problems drawer's state: every source's code diagnostics, the
//! published findings, and what the person is looking at (the tab, the
//! filters, the open row, the collapsed groups, the dismissed findings).
//! `components` renders it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use gpui::{prelude::*, *};
use pulsar_findings::{Category, Severity};
use ui::{input::InputState, scroll::ScrollbarState};

use crate::utils::items::{self, Item, Row, View};
use crate::utils::types::{
    Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic, RunFindingAction,
};

/// How often the drawer looks for newly published findings (an atomic read).
const FINDINGS_POLL: Duration = Duration::from_millis(500);

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
    /// Every source's diagnostics, in source-name order.
    pub(crate) diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
    /// Each source's own slot. A source publishes its complete set (an empty
    /// set removes its slot), so several files from several sources are shown
    /// at once and a fixed one goes.
    pub(crate) sources: BTreeMap<String, SourceSlot>,
    pub(crate) filtered_severity: Option<DiagnosticSeverity>,
    /// Show only this source's items.
    pub(crate) filtered_source: Option<String>,
    pub(crate) search_query: String,
    /// The open diagnostic's file preview, by (file, line): the editor and its
    /// line count, or `None` when the file has no such line (read once).
    pub(crate) preview_inputs: HashMap<(String, usize), Option<(Entity<InputState>, usize)>>,
    pub(crate) diff_editors: HashMap<(usize, usize), (Entity<InputState>, Entity<InputState>)>,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) project_root: Option<PathBuf>,

    /// The tab shown.
    pub(crate) tab: Category,
    /// Every code diagnostic and finding, as listed.
    pub(crate) items: Vec<Item>,
    /// The current tab's rows (group headings and items).
    pub(crate) rows: Vec<Row>,
    pub(crate) list: ListState,
    pub(crate) scrollbar: ScrollbarState,
    /// The open row, by item id: only it shows its details.
    pub(crate) expanded: Option<String>,
    /// Collapsed groups, by `<category>/<group>`.
    pub(crate) collapsed: BTreeSet<String>,
    /// Findings the project dismissed (`utils::dismissed`).
    pub(crate) dismissed: BTreeSet<String>,
    /// List the dismissed findings instead (to restore them).
    pub(crate) dismissed_only: bool,
    /// The findings store generation the items were built from.
    findings_generation: u64,
    _poll: Task<()>,
}

impl EventEmitter<NavigateToDiagnostic> for ProblemsDrawer {}
impl EventEmitter<RunFindingAction> for ProblemsDrawer {}

impl ProblemsDrawer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search problems..."));
        cx.subscribe(&search_input, |this, input, event: &ui::input::InputEvent, cx| {
            if matches!(event, ui::input::InputEvent::Change) {
                let query = input.read(cx).value().to_string();
                if query != this.search_query {
                    this.search_query = query;
                    this.refresh_rows();
                    cx.notify();
                }
            }
        })
        .detach();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(FINDINGS_POLL).await;
            let alive = this.update(cx, |drawer, cx| {
                if pulsar_findings::generation() != drawer.findings_generation {
                    drawer.refresh_items();
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            diagnostics: Arc::new(Mutex::new(Vec::new())),
            sources: BTreeMap::new(),
            filtered_severity: None,
            filtered_source: None,
            search_query: String::new(),
            preview_inputs: HashMap::new(),
            diff_editors: HashMap::new(),
            search_input,
            project_root: None,
            tab: Category::Code,
            items: Vec::new(),
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.0)),
            scrollbar: ScrollbarState::default(),
            expanded: None,
            collapsed: BTreeSet::new(),
            dismissed: BTreeSet::new(),
            dismissed_only: false,
            findings_generation: 0,
            _poll: poll,
        };
        this.refresh_items();
        this
    }

    pub fn clear_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.sources.clear();
        self.filtered_source = None;
        self.rebuild();
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
        self.refresh_items();
    }

    /// Every item again from the code slots and the findings store.
    pub(crate) fn refresh_items(&mut self) {
        self.findings_generation = pulsar_findings::generation();
        let code: BTreeMap<String, Vec<Diagnostic>> = self
            .sources
            .iter()
            .map(|(name, slot)| (name.clone(), slot.current.clone()))
            .collect();
        self.items = items::build_items(
            &code,
            &pulsar_findings::snapshot(),
            self.project_root.as_deref(),
        );
        if self
            .expanded
            .as_ref()
            .is_some_and(|id| !self.items.iter().any(|item| &item.id == id))
        {
            self.expanded = None;
        }
        self.refresh_rows();
    }

    /// The current tab's rows again. The list keeps its scroll position
    /// (rows are spliced, not reset), so a republish does not jump.
    pub(crate) fn refresh_rows(&mut self) {
        let view = View {
            severity: self.filtered_severity.as_ref().map(items::severity_of),
            query: self.search_query.trim().to_lowercase(),
            dismissed_only: self.dismissed_only,
            source: self.filtered_source.clone(),
        };
        let rows = items::rows(&self.items, self.tab, &view, &self.dismissed, &self.collapsed);
        let old = self.list.item_count();
        self.rows = rows;
        self.list.splice(0..old, self.rows.len());
    }

    /// Replace everything `source` publishes (empty removes it). Returns
    /// whether anything changed.
    ///
    /// A server republishes the same set repeatedly. Treating each publish as
    /// a change reset the caches and notified the drawer (and, through the
    /// caller, the whole app) several times a second with nothing new to
    /// show -- each notify turning a ~0.5 ms display-only frame into a
    /// multi-millisecond full draw.
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

    /// The sources that currently have code diagnostics.
    pub fn source_names(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    /// The number of diagnostics of `source`.
    pub fn source_count(&self, source: &str) -> usize {
        self.sources
            .get(source)
            .map_or(0, |slot| slot.current.len())
    }

    /// The code diagnostic an item stands for.
    pub(crate) fn code_diagnostic(&self, item: &Item) -> Option<&Diagnostic> {
        let (source, index) = item.code.as_ref()?;
        self.sources.get(source)?.current.get(*index)
    }

    /// How many items, of every tab and source, have `severity` (dismissed
    /// findings not counted): the footer's counts.
    pub fn count_all(&self, severity: Severity) -> usize {
        self.items
            .iter()
            .filter(|item| item.severity == severity)
            .filter(|item| !(item.dismissible() && self.dismissed.contains(&item.id)))
            .count()
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

    /// The project the drawer is for: its paths are shown relative to it and
    /// its dismissed findings are loaded.
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.dismissed = project_root
            .as_deref()
            .map(crate::utils::dismissed::load)
            .unwrap_or_default();
        self.project_root = project_root;
        self.refresh_items();
        cx.notify();
    }

    /// Show `tab`.
    pub fn show_tab(&mut self, tab: Category, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.dismissed_only = false;
            self.refresh_rows();
            self.list.scroll_to(ListOffset::default());
        }
        cx.notify();
    }

    /// Open the row of `id` (closing the open one), or close it when open.
    pub(crate) fn toggle_expanded(&mut self, id: &str, cx: &mut Context<Self>) {
        self.expanded = match &self.expanded {
            Some(open) if open == id => None,
            _ => Some(id.to_string()),
        };
        cx.notify();
    }

    pub(crate) fn toggle_group(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_string());
        }
        self.refresh_rows();
        cx.notify();
    }

    pub(crate) fn toggle_dismissed_view(&mut self, cx: &mut Context<Self>) {
        self.dismissed_only = !self.dismissed_only;
        self.refresh_rows();
        cx.notify();
    }

    /// Dismiss the finding `id`, or restore it when dismissed; saved with
    /// the project.
    pub(crate) fn toggle_dismissed(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.dismissed.remove(id) {
            self.dismissed.insert(id.to_string());
        }
        if let Some(project) = self.project_root.as_deref() {
            // Ids no source publishes any more are dropped as the file is written.
            let live: BTreeSet<&str> = self.items.iter().map(|item| item.id.as_str()).collect();
            let kept: BTreeSet<String> = self
                .dismissed
                .iter()
                .filter(|id| live.contains(id.as_str()))
                .cloned()
                .collect();
            if let Err(error) = crate::utils::dismissed::save(project, &kept) {
                tracing::warn!("could not save the dismissed findings: {error}");
            }
        }
        if self.dismissed_only
            && !self
                .items
                .iter()
                .any(|item| item.category == self.tab && self.dismissed.contains(&item.id))
        {
            self.dismissed_only = false;
        }
        self.refresh_rows();
        cx.notify();
    }

    /// Carry out a finding's action: a file opens through the navigation
    /// event, the rest goes to the host.
    pub(crate) fn run_action(&mut self, action: &pulsar_findings::Action, cx: &mut Context<Self>) {
        match action {
            pulsar_findings::Action::OpenFile { path, line, column } => {
                cx.emit(NavigateToDiagnostic {
                    file_path: path.clone(),
                    line: *line,
                    column: *column,
                });
            }
            other => cx.emit(RunFindingAction(other.clone())),
        }
    }
}

impl Focusable for ProblemsDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProblemsDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::components::render_drawer(self, window, cx)
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

    /// The code diagnostics the Code tab lists now.
    fn shown(drawer: &ProblemsDrawer) -> Vec<Diagnostic> {
        drawer
            .rows
            .iter()
            .filter_map(|row| match row {
                crate::utils::items::Row::Item(ix) => drawer.code_diagnostic(&drawer.items[*ix]).cloned(),
                crate::utils::items::Row::Group { .. } => None,
            })
            .collect()
    }

    /// The Code tab's groups (one per file).
    fn groups(drawer: &ProblemsDrawer) -> Vec<String> {
        drawer
            .rows
            .iter()
            .filter_map(|row| match row {
                crate::utils::items::Row::Group { title, .. } => Some(title.clone()),
                crate::utils::items::Row::Item(_) => None,
            })
            .collect()
    }

    struct Host;

    impl gpui::Render for Host {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
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
            let files = groups(drawer);
            assert_eq!(files.len(), 2, "both files are listed");

            // The same set again is not a change.
            assert!(!drawer.set_source_diagnostics(
                "rust-analyzer",
                vec![problem("/p/a.rs", "in a"), problem("/p/b.rs", "in b")],
                cx
            ));

            // a.rs was fixed: the server's next complete set no longer has it.
            assert!(drawer.set_source_diagnostics(
                "rust-analyzer",
                vec![problem("/p/b.rs", "in b")],
                cx
            ));
            assert_eq!(drawer.total_count(), 1);
            assert_eq!(groups(drawer).len(), 1);

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
            drawer.set_source_diagnostics(
                "typescript compiler",
                vec![problem("/p/c.ts", "ts")],
                cx,
            );
            assert_eq!(
                drawer.source_names(),
                ["rust-analyzer", "typescript compiler"]
            );
            assert_eq!(drawer.total_count(), 2);

            // One source's republish does not touch the other's.
            drawer.set_source_diagnostics("rust-analyzer", Vec::new(), cx);
            assert_eq!(drawer.source_names(), ["typescript compiler"]);

            drawer.set_source_diagnostics("rust-analyzer", vec![problem("/p/a.rs", "ra")], cx);
            drawer.set_source_filter(Some("typescript compiler".into()), cx);
            let only_ts = shown(drawer);
            assert_eq!(only_ts.len(), 1);
            assert_eq!(only_ts[0].message, "ts");
            drawer.set_source_filter(None, cx);
            assert_eq!(shown(drawer).len(), 2);

            // A source that empties while it is the filter drops the filter.
            drawer.set_source_filter(Some("rust-analyzer".into()), cx);
            drawer.set_source_diagnostics("rust-analyzer", Vec::new(), cx);
            assert_eq!(shown(drawer).len(), 1);
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
            let listed = shown(drawer);
            let ra = listed.iter().find(|d| d.message == "ra").unwrap();
            assert_eq!(ra.hints.len(), 1);
            assert!(!ra.loading_actions);
        });
    }
}
