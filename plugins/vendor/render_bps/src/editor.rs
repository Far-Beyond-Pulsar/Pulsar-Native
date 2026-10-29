//! The Render Graph editor: one document on the shared Graphy canvas.
//!
//! The document is a main graph and one graph per section. Each is a Graphy
//! `GraphDescription` owned by its `GraphCanvas` entity; there is no other
//! editable copy. Edits are `GraphCommand` batches the canvas applies and reports
//! as `CanvasEvent::Edited`, which is where undo, dirty state and the status line
//! hook in. `.rendergraph` text is read once at load ([`crate::document::decode`])
//! and written from the document at save ([`crate::document::encode`]); validation
//! and the preview read the same graphs through a [`DocRef`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::*;
use graphy::{Connection, ConnectionType, GraphCommand, GraphDescription, NodeInstance, Position};
use graphy_gpui::canvas::{system_fonts, CanvasEvent, GraphCanvas};
use graphy_gpui::{Action, GraphProfile as _, MenuItem, MenuTarget};
use graphy_render::Vec2;
use plugin_editor_api::{DocumentId, History, SharedHistory, UndoEntry, UndoService};
use serde_json::Value;
use ui::dock::{Panel, PanelEvent};
use ui::input::{InputEvent, InputState};
use ui::workspace::Workspace;
use uuid::Uuid;

use crate::collapse::collapse as document_collapse;
use crate::document::{self, DocRef, GraphRef, RenderDocument};
use crate::model;
use crate::preview::PipelinePreview;
use crate::profile::{RenderProfile, SectionNames};

/// Grid the canvas snaps nodes and comments to.
pub const GRID: f64 = 10.0;

/// Context menu entries this editor adds to Graphy's.
const MENU_COLLAPSE: &str = "render.collapse";
const MENU_EXPAND: &str = "render.expand";

pub(crate) fn snap(v: f64) -> f64 {
    (v / GRID).round() * GRID
}

/// One undoable edit of a graph: the change set to revert and the commands to
/// replay.
#[derive(Debug, Clone)]
pub struct Command {
    pub label: String,
    pub commands: Vec<GraphCommand>,
    pub changes: graphy::ChangeSet,
}

impl UndoEntry for Command {
    fn label(&self) -> String {
        self.label.clone()
    }
}

/// What the status line says.
#[derive(Clone, Debug)]
pub struct Status {
    pub is_error: bool,
    pub text: String,
}

/// One graph of the document: the main graph or a section.
pub struct GraphTab {
    /// `""` for the main graph, the section id otherwise.
    pub id: String,
    pub name: String,
    /// Node ids in authoring order (see [`GraphRef::order`]).
    pub order: Vec<String>,
    pub canvas: Entity<GraphCanvas>,
    pub history: SharedHistory<Command>,
}

/// A context menu requested on the canvas.
#[derive(Clone, Debug)]
pub struct CanvasMenu {
    pub at: Point<Pixels>,
    pub local: Vec2,
    pub target: MenuTarget,
    pub items: Vec<MenuItem>,
}

/// The node picker: opened by a right click on empty canvas, or by dropping a
/// wire there (then it offers nodes the wire can reach).
pub struct Palette {
    pub at: Point<Pixels>,
    pub graph_pos: Vec2,
    pub source: Option<graphy::PinRef>,
    pub entries: Vec<graphy_gpui::CatalogEntry>,
    pub focus_pending: bool,
}

pub struct RenderGraphEditor {
    pub(crate) file: PathBuf,
    pub(crate) load_error: Option<String>,
    pub(crate) loading: bool,
    format_version: u32,
    name: String,
    description: String,
    pub(crate) tabs: Vec<GraphTab>,
    /// Indices into `tabs` of the graphs shown as tabs; the main graph is always first.
    pub(crate) open: Vec<usize>,
    pub(crate) active: usize,
    pub(crate) profile: Option<Arc<RenderProfile>>,
    section_names: SectionNames,
    pub(crate) status: Option<Status>,
    pub(crate) undo_service: Option<UndoService>,
    pub(crate) workspace: Option<Entity<Workspace>>,
    pub(crate) preview: Option<Entity<PipelinePreview>>,
    pub(crate) focus_handle: FocusHandle,

    pub(crate) menu: Option<CanvasMenu>,
    pub(crate) palette: Option<Palette>,
    pub(crate) palette_input: Entity<InputState>,
    pub(crate) editing_comment: Option<usize>,
    pub(crate) comment_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for RenderGraphEditor {}

impl Focusable for RenderGraphEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for RenderGraphEditor {
    fn panel_name(&self) -> &'static str {
        "Render Graph Editor"
    }

    fn panel_file_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.file.clone())
    }

    fn tab_unsaved(&self, _cx: &App) -> bool {
        self.is_dirty()
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let title = self
            .file
            .file_stem()
            .map_or_else(|| "Render Graph".to_string(), |s| s.to_string_lossy().into_owned());
        div().text_sm().child(title).into_any_element()
    }
}

impl RenderGraphEditor {
    /// Opens the render graph at `file`. It loads in the background (the node
    /// registry waits for the pin probe), so the editor first shows that it is
    /// loading.
    pub fn open(file: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search nodes…"));
        let comment_input = cx.new(|cx| InputState::new(window, cx).placeholder("Comment text..."));
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &palette_input,
            window,
            |_this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &comment_input,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.finish_comment_editing(cx);
                }
            },
        ));
        let editor = Self {
            file: file.clone(),
            load_error: None,
            loading: true,
            format_version: helio_graph_asset::FORMAT_VERSION,
            name: String::new(),
            description: String::new(),
            tabs: Vec::new(),
            open: Vec::new(),
            active: 0,
            profile: None,
            section_names: Arc::new(RwLock::new(HashMap::new())),
            status: None,
            undo_service: None,
            workspace: None,
            preview: None,
            focus_handle: cx.focus_handle(),
            menu: None,
            palette: None,
            palette_input,
            editing_comment: None,
            comment_input,
            _subscriptions: subscriptions,
        };
        cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { load(&file) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.finish_load(loaded, window, cx));
        })
        .detach();
        editor
    }

    /// An editor that shows why the file could not be opened.
    pub fn with_error(file: PathBuf, error: String, cx: &mut Context<Self>, window: &mut Window) -> Self {
        let mut editor = Self::open(file, window, cx);
        editor.loading = false;
        editor.load_error = Some(error);
        editor
    }

    fn finish_load(
        &mut self,
        loaded: Result<RenderDocument, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        match loaded {
            Ok(doc) => self.install(doc, window, cx),
            Err(error) => {
                tracing::error!("Failed to open render graph {:?}: {error}", self.file);
                self.load_error = Some(error);
            }
        }
        cx.notify();
    }

    /// Takes the graphs of the decoded document: from here on the canvases own them.
    fn install(&mut self, doc: RenderDocument, window: &mut Window, cx: &mut Context<Self>) {
        let RenderDocument {
            format_version,
            name,
            description,
            main,
            main_order,
            sections,
        } = doc;
        self.format_version = format_version;
        self.name = name;
        self.description = description;
        *self.section_names.write().unwrap_or_else(|e| e.into_inner()) = sections
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let profile = Arc::new(RenderProfile::new(
            model::registry().clone(),
            self.section_names.clone(),
        ));
        self.profile = Some(profile.clone());

        let mut graphs = vec![(String::new(), self.display_name(), main, main_order)];
        graphs.extend(
            sections
                .into_iter()
                .map(|s| (s.id, s.name, s.graph, s.order)),
        );
        for (id, name, graph, order) in graphs {
            self.add_graph_tab(id, name, graph, order, window, cx);
        }
        self.open = vec![0];
        self.active = 0;
        if let Some(service) = self.undo_service.clone() {
            self.adopt_histories(&service);
        }
        self.initialize_workspace(window, cx);
    }

    /// Gives a graph of the document its canvas.
    fn add_graph_tab(
        &mut self,
        id: String,
        name: String,
        graph: GraphDescription,
        order: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profile.clone() else {
            return;
        };
        let canvas = cx.new(|cx| {
            GraphCanvas::new(graph, profile, system_fonts(), cx)
                .expect("the bundled canvas font parses")
        });
        let tab_id = id.clone();
        cx.subscribe_in(
            &canvas,
            window,
            move |this, _canvas, event: &CanvasEvent, window, cx| {
                this.on_canvas_event(&tab_id, event, window, cx)
            },
        )
        .detach();
        let history: SharedHistory<Command> =
            Arc::new(std::sync::Mutex::new(History::with_capacity(100)));
        if let Some(service) = &self.undo_service {
            let document = DocumentId::for_path(&self.file);
            service.adopt(&DocumentId::new(format!("{document}#{id}")), &history);
        }
        self.tabs.push(GraphTab {
            id,
            name,
            order,
            canvas,
            history,
        });
    }

    fn display_name(&self) -> String {
        if self.name.is_empty() {
            "Main".to_string()
        } else {
            self.name.clone()
        }
    }

    // ── The document ───────────────────────────────────────────────────────

    /// Reads the document: the main graph and each section, borrowed from their
    /// canvases. This is what saving, validation and the preview see.
    pub(crate) fn with_doc<R>(&self, cx: &App, f: impl FnOnce(&DocRef<'_>) -> R) -> R {
        let contained: Vec<Vec<Vec<String>>> = self
            .tabs
            .iter()
            .map(|tab| contained_nodes(tab.canvas.read(cx)))
            .collect();
        let mut refs = self.tabs.iter().zip(&contained).map(|(tab, contained)| GraphRef {
            id: &tab.id,
            name: &tab.name,
            graph: tab.canvas.read(cx).graph(),
            order: &tab.order,
            contained: Some(contained),
        });
        let main = refs.next().expect("a document has a main graph");
        let doc = DocRef {
            format_version: self.format_version,
            name: &self.name,
            description: &self.description,
            main,
            sections: refs.collect(),
        };
        f(&doc)
    }

    pub(crate) fn active_tab(&self) -> Option<&GraphTab> {
        self.tabs.get(self.active)
    }

    pub(crate) fn active_canvas(&self) -> Option<&Entity<GraphCanvas>> {
        self.active_tab().map(|tab| &tab.canvas)
    }

    pub(crate) fn tab_index(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// `(id, name, nodes inside)` of each section, in document order.
    pub fn sections(&self, cx: &App) -> Vec<(String, String, usize)> {
        self.tabs
            .iter()
            .skip(1)
            .map(|tab| {
                // Entry and exit are not passes.
                let nodes = tab
                    .canvas
                    .read(cx)
                    .graph()
                    .nodes
                    .values()
                    .filter(|n| !matches!(n.node_type.as_str(), document::ENTRY_TYPE | document::EXIT_TYPE))
                    .count();
                (tab.id.clone(), tab.name.clone(), nodes)
            })
            .collect()
    }

    pub fn is_dirty(&self) -> bool {
        self.tabs
            .iter()
            .any(|tab| plugin_editor_api::undo::lock(&tab.history).is_dirty())
    }

    // ── Tabs ───────────────────────────────────────────────────────────────

    /// Shows the section `id` as a tab and makes it the active graph.
    pub fn open_section(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.tab_index(id) else {
            return;
        };
        if !self.open.contains(&index) {
            self.open.push(index);
        }
        self.activate(index, cx);
    }

    pub(crate) fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.menu = None;
            self.palette = None;
            self.editing_comment = None;
            cx.notify();
        }
    }

    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 {
            return;
        }
        self.open.retain(|open| *open != index);
        if self.active == index {
            self.active = 0;
        }
        cx.notify();
    }

    // ── Undo ───────────────────────────────────────────────────────────────

    /// Gives every graph's history to the host, keyed by the graph of this file.
    pub fn set_undo_service(&mut self, service: UndoService) {
        self.adopt_histories(&service);
        self.undo_service = Some(service);
    }

    fn adopt_histories(&self, service: &UndoService) {
        let document = DocumentId::for_path(&self.file);
        for tab in &self.tabs {
            let graph = DocumentId::new(format!("{document}#{}", tab.id));
            service.adopt(&graph, &tab.history);
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (history, canvas) = (tab.history.clone(), tab.canvas.clone());
        let undone = plugin_editor_api::undo::lock(&history)
            .undo(|command| {
                canvas.update(cx, |canvas, cx| canvas.undo_changes(&command.changes, cx));
                true
            })
            .is_some();
        if undone {
            self.after_edit(cx);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (history, canvas) = (tab.history.clone(), tab.canvas.clone());
        let redone = plugin_editor_api::undo::lock(&history)
            .redo(|command| {
                canvas
                    .update(cx, |canvas, cx| canvas.redo_commands(&command.commands, cx))
                    .is_ok()
            })
            .is_some();
        if redone {
            self.after_edit(cx);
        }
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    // ── Canvas events ──────────────────────────────────────────────────────

    fn on_canvas_event(
        &mut self,
        tab_id: &str,
        event: &CanvasEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            CanvasEvent::Edited {
                label,
                commands,
                changes,
            } => {
                if let Some(index) = self.tab_index(tab_id) {
                    let mut history = plugin_editor_api::undo::lock(&self.tabs[index].history);
                    history.push(Command {
                        label: (*label).to_string(),
                        commands: commands.clone(),
                        changes: changes.clone(),
                    });
                    history.seal();
                }
                self.after_edit(cx);
            }
            CanvasEvent::Rejected(why) => tracing::warn!("graph edit rejected: {why}"),
            CanvasEvent::SelectionChanged => cx.notify(),
            CanvasEvent::ViewChanged => {}
            CanvasEvent::Request(action) => self.on_canvas_request(action.clone(), window, cx),
        }
    }

    fn on_canvas_request(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        match action {
            Action::ContextMenu { at, target, items } => {
                self.palette = None;
                let screen = canvas.read(cx).local_to_window(at);
                if let MenuTarget::Canvas { graph_pos } = target {
                    self.open_palette(screen, graph_pos, None, window, cx);
                    self.menu = None;
                } else {
                    let items = self.with_section_items(items, &target, cx);
                    self.menu = Some(CanvasMenu {
                        at: screen,
                        local: at,
                        target,
                        items,
                    });
                }
                cx.notify();
            }
            Action::OpenGraph(target) => {
                // The double-click lands while the canvas is updating.
                let editor = cx.weak_entity();
                let window_handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = cx.update_window(window_handle, |_, _, cx| {
                        let _ = editor.update(cx, |editor, cx| {
                            editor.open_section(&target.graph_id, cx)
                        });
                    });
                });
            }
            Action::CommentActivated(index) => self.start_comment_editing(index, window, cx),
            Action::ConnectionDropped { from, side, at } => {
                if side != graphy_render::PinSide::Output {
                    return;
                }
                let screen = canvas.read(cx).local_to_window(at);
                let graph_pos = canvas.read(cx).window_to_graph(screen);
                self.menu = None;
                self.open_palette(screen, graph_pos, Some(from), window, cx);
                cx.notify();
            }
            Action::ConnectionRefused(why) => tracing::debug!("connection refused: {why}"),
            _ => {}
        }
    }

    /// Runs the entry of the open context menu.
    pub(crate) fn run_menu_item(
        &mut self,
        menu: CanvasMenu,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        match id {
            MENU_COLLAPSE => {
                self.collapse_selection(&menu.target, window, cx);
                return;
            }
            MENU_EXPAND => {
                if let MenuTarget::Node(node) = &menu.target {
                    self.expand_section_node(node, cx);
                }
                return;
            }
            _ => {}
        }
        if let Some(canvas) = self.active_canvas().cloned() {
            canvas.update(cx, |canvas, cx| {
                canvas.run_menu_item(&menu.target, id, menu.local, cx)
            });
        }
        cx.notify();
    }

    // ── Sections ───────────────────────────────────────────────────────────

    /// The render graph entries of a node menu: collapse the selection into a
    /// section, or expand a section node.
    fn with_section_items(
        &self,
        mut items: Vec<MenuItem>,
        target: &MenuTarget,
        cx: &App,
    ) -> Vec<MenuItem> {
        let section_node = match target {
            MenuTarget::Node(id) => self
                .active_canvas()
                .and_then(|canvas| canvas.read(cx).graph().nodes.get(id))
                .is_some_and(|node| {
                    helio_graph_asset::sections::section_id(&node.node_type).is_some()
                }),
            _ => false,
        };
        let mut extra = Vec::new();
        if matches!(target, MenuTarget::Node(_) | MenuTarget::Selection) {
            extra.push(MenuItem::new(MENU_COLLAPSE, "Collapse to Section").separated());
        }
        if section_node {
            extra.push(MenuItem::new(MENU_EXPAND, "Expand Section"));
        }
        extra.append(&mut items);
        extra
    }

    /// Moves the selected nodes into a new section, placed by one node.
    fn collapse_selection(
        &mut self,
        target: &MenuTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let mut selected: Vec<String> = canvas
            .read(cx)
            .controller()
            .selection
            .nodes
            .iter()
            .cloned()
            .collect();
        if let MenuTarget::Node(id) = target {
            if !selected.contains(id) {
                selected = vec![id.clone()];
            }
        }
        let taken = self.tabs.iter().map(|tab| tab.id.clone()).collect();
        let collapsed = match document_collapse(canvas.read(cx).graph(), &selected, &taken) {
            Ok(collapsed) => collapsed,
            Err(error) => {
                self.status = Some(Status {
                    is_error: true,
                    text: error,
                });
                cx.notify();
                return;
            }
        };
        // The profile titles the placing node from the section's name.
        self.section_names
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(collapsed.id.clone(), collapsed.name.clone());
        let (commands, placed) = (collapsed.commands, collapsed.node.id);
        self.add_graph_tab(
            collapsed.id,
            collapsed.name,
            collapsed.graph,
            collapsed.order,
            window,
            cx,
        );
        let _ = canvas.update(cx, |canvas, cx| {
            canvas.apply("Collapse to section", &commands, cx)
        });
        canvas.update(cx, |canvas, cx| canvas.select_nodes([placed], cx));
        cx.notify();
    }

    /// Puts the nodes of the section that `node` places back in the active graph.
    /// The section stays in the Sections list, so undoing this still finds it.
    fn expand_section_node(&mut self, node: &str, cx: &mut Context<Self>) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let section = canvas
            .read(cx)
            .graph()
            .nodes
            .get(node)
            .and_then(|n| helio_graph_asset::sections::section_id(&n.node_type))
            .and_then(|id| self.tab_index(id))
            .map(|index| self.tabs[index].canvas.read(cx).graph().clone());
        let Some(section) = section else {
            return;
        };
        match crate::collapse::expand(canvas.read(cx).graph(), node, &section) {
            Ok(commands) => {
                let _ = canvas.update(cx, |canvas, cx| {
                    canvas.apply("Expand section", &commands, cx)
                });
            }
            Err(error) => {
                self.status = Some(Status {
                    is_error: true,
                    text: error,
                });
                cx.notify();
            }
        }
    }

    // ── Node picker ────────────────────────────────────────────────────────

    fn open_palette(
        &mut self,
        at: Point<Pixels>,
        graph_pos: Vec2,
        source: Option<graphy::PinRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(profile), Some(canvas)) = (self.profile.clone(), self.active_canvas().cloned())
        else {
            return;
        };
        let mut entries = profile.catalog();
        if let Some(source) = &source {
            // Only nodes the dragged wire can reach.
            let graph = canvas.read(cx).graph();
            let from_type = graph
                .nodes
                .get(&source.node)
                .and_then(|n| n.outputs.iter().find(|p| p.id == source.pin))
                .map(|p| p.data_type().clone());
            if let Some(from_type) = from_type {
                let resolver = profile.type_resolver();
                entries.retain(|entry| {
                    profile
                        .create_node(&entry.definition, "probe", Position::new(0.0, 0.0))
                        .is_some_and(|node| {
                            node.inputs.iter().any(|pin| {
                                resolver
                                    .compatibility(&from_type, pin.data_type())
                                    .is_connectable()
                            })
                        })
                });
            }
        }
        self.palette_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.palette = Some(Palette {
            at,
            graph_pos,
            source,
            entries,
            focus_pending: true,
        });
    }

    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette = None;
        cx.notify();
    }

    /// Adds the picked node where the picker was opened, wired to the dragged
    /// wire's source if there was one.
    pub(crate) fn place_picked(&mut self, definition: &str, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.take() else {
            return;
        };
        let at = Position::new(snap(f64::from(palette.graph_pos.x)), snap(f64::from(palette.graph_pos.y)));
        self.add_node(definition, Vec::new(), at, palette.source, cx);
        cx.notify();
    }

    /// Adds a node of the catalogue to the active graph, with `properties` set and
    /// its pins in line with them, wired from `source` when given. One edit.
    pub(crate) fn add_node(
        &mut self,
        definition: &str,
        properties: Vec<(String, String)>,
        at: Position,
        source: Option<graphy::PinRef>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let (profile, canvas) = (self.profile.clone()?, self.active_canvas().cloned()?);
        let id = Uuid::new_v4().to_string();
        let mut node = profile.create_node(definition, &id, at)?;
        for (key, value) in properties {
            node.properties.insert(key, Value::from(value));
        }
        let mut commands = vec![GraphCommand::AddNode(node.clone())];
        let graph = canvas.read(cx).graph();
        let mut after = graph.clone();
        if commands[0].apply(&mut after).is_ok() {
            commands.extend(profile.pin_commands(&after, &id));
        }
        if let Some(source) = source {
            // The pins may have changed with the properties.
            let node = after.nodes.get(&id).unwrap_or(&node);
            commands.extend(connect_commands(&profile, graph, &source, node));
        }
        canvas
            .update(cx, |canvas, cx| canvas.apply("Add node", &commands, cx))
            .ok()?;
        Some(id)
    }

    /// Adds a node at the centre of the active graph's view.
    pub fn add_node_at_center(
        &mut self,
        definition: &str,
        properties: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let canvas_ref = canvas.read(cx);
        let (pan, zoom) = canvas_ref.view();
        let vp = canvas_ref.viewport();
        let centre = Vec2::new(vp.x * 0.5 / zoom - pan.x, vp.y * 0.5 / zoom - pan.y);
        let at = Position::new(snap(f64::from(centre.x)), snap(f64::from(centre.y)));
        self.add_node(definition, properties, at, None, cx);
    }

    // ── Node parameters ────────────────────────────────────────────────────

    /// Sets one parameter of a node from editor text and brings the node's pins in
    /// line with it, as one edit. No-op when the value is unchanged.
    pub fn set_node_property(&mut self, node_id: &str, key: &str, value: String, cx: &mut Context<Self>) {
        let (Some(profile), Some(canvas)) = (self.profile.clone(), self.active_canvas().cloned())
        else {
            return;
        };
        let new = Value::from(value);
        let graph = canvas.read(cx).graph();
        let unchanged = graph.nodes.get(node_id).is_some_and(|n| {
            n.properties
                .get(key)
                .is_some_and(|old| model::property_text(old) == model::property_text(&new))
        });
        if unchanged {
            return;
        }
        let mut commands = vec![GraphCommand::SetProperty {
            node: node_id.to_string(),
            key: key.to_string(),
            value: Some(new),
        }];
        // The node sees its pins as they will be after the edit.
        let mut after = graph.clone();
        if commands[0].apply(&mut after).is_ok() {
            commands.extend(profile.pin_commands(&after, node_id));
        }
        let _ = canvas.update(cx, |canvas, cx| canvas.apply("Edit property", &commands, cx));
    }

    /// Arranges the active graph (the toolbar's Auto Layout).
    pub fn auto_layout(&mut self, cx: &mut Context<Self>) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let commands = model::auto_layout_graph(canvas.read(cx).graph());
        if !commands.is_empty() {
            let _ = canvas.update(cx, |canvas, cx| canvas.apply("Auto layout", &commands, cx));
        }
    }

    // ── Comments ───────────────────────────────────────────────────────────

    /// Adds a comment at the centre of the view, around the selected nodes if any.
    pub fn add_comment(&mut self, cx: &mut Context<Self>) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let canvas_ref = canvas.read(cx);
        let (pan, zoom) = canvas_ref.view();
        let vp = canvas_ref.viewport();
        let at = Vec2::new(vp.x * 0.5 / zoom - pan.x, vp.y * 0.5 / zoom - pan.y);
        let mut comment = graphy::core::GraphComment::new(
            "Comment",
            Position::new(snap(f64::from(at.x)), snap(f64::from(at.y))),
            (300.0, 200.0),
        );
        comment.id = Uuid::new_v4().to_string();
        comment.color = [0.2, 0.4, 0.8, 0.35];
        let selected: Vec<String> = canvas_ref.controller().selection.nodes.iter().cloned().collect();
        let rects: Vec<_> = selected
            .iter()
            .filter_map(|id| canvas_ref.scene().node(id))
            .map(|n| n.rect())
            .collect();
        if !rects.is_empty() {
            const PADDING: f32 = 30.0;
            let left = rects.iter().map(|r| r.min.x).fold(f32::INFINITY, f32::min) - PADDING;
            let top = rects.iter().map(|r| r.min.y).fold(f32::INFINITY, f32::min) - PADDING;
            let right = rects.iter().map(|r| r.max.x).fold(f32::NEG_INFINITY, f32::max) + PADDING;
            let bottom = rects.iter().map(|r| r.max.y).fold(f32::NEG_INFINITY, f32::max) + PADDING;
            comment.position = Position::new(snap(f64::from(left)), snap(f64::from(top)));
            comment.size = (
                snap(f64::from(right - left)).max(100.0),
                snap(f64::from(bottom - top)).max(50.0),
            );
            comment.contained_node_ids = selected;
        }
        let index = canvas_ref.graph().comments.len();
        let _ = canvas.update(cx, |canvas, cx| {
            canvas.apply("Add comment", &[GraphCommand::AddComment { index, comment }], cx)
        });
    }

    fn start_comment_editing(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(canvas) = self.active_canvas().cloned() else {
            return;
        };
        let Some(text) = canvas.read(cx).graph().comments.get(index).map(|c| c.text.clone()) else {
            return;
        };
        self.editing_comment = Some(index);
        self.comment_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        let handle = self.comment_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn finish_comment_editing(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.editing_comment.take() else {
            return;
        };
        let text = self.comment_input.read(cx).text().to_string();
        if let Some(canvas) = self.active_canvas().cloned() {
            let changed = canvas
                .read(cx)
                .graph()
                .comments
                .get(index)
                .is_some_and(|c| c.text != text);
            if changed {
                let _ = canvas.update(cx, |canvas, cx| {
                    canvas.apply(
                        "Edit comment",
                        &[GraphCommand::EditComment {
                            index,
                            text: Some(text),
                            color: None,
                            contained_node_ids: None,
                        }],
                        cx,
                    )
                });
            }
        }
        cx.notify();
    }

    // ── Checking, saving, preview ──────────────────────────────────────────

    /// What stops the document from building, one line each; and the asset that
    /// was checked. The asset is a projection for Helio, built from the document.
    fn check(&self, cx: &App) -> (Vec<String>, helio_graph_asset::GraphAsset) {
        let registry = model::registry();
        self.with_doc(cx, |doc| {
            let asset = doc.buildable_asset(registry);
            let mut problems = model::problems(&asset, registry);
            // Without the probe the stage nodes have no resource pins; say why.
            if let Some(reason) =
                engine_backend::subsystems::render::helio_renderer::pipelines::graph_pin_probe_error()
            {
                problems.insert(0, format!("node pins unavailable: {reason}"));
            }
            (problems, asset)
        })
    }

    /// The toolbar's Compile: check the graph and, if it is sound, run it in the preview.
    pub fn validate(&mut self, cx: &mut Context<Self>) {
        let (problems, asset) = self.check(cx);
        self.status = Some(if problems.is_empty() {
            Status {
                is_error: false,
                text: "✓ Graph is valid".to_string(),
            }
        } else {
            Status {
                is_error: true,
                text: format!("✗ {}", problems.join("; ")),
            }
        });
        if problems.is_empty() {
            self.apply_to_preview(asset, cx);
        }
        cx.notify();
    }

    fn apply_to_preview(&mut self, asset: helio_graph_asset::GraphAsset, cx: &mut Context<Self>) {
        if let Some(preview) = &self.preview {
            preview.update(cx, |preview, cx| preview.apply_asset(asset, cx));
        }
    }

    /// Writes the document to its file. The preview follows what is saved.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = &self.load_error {
            self.status = Some(Status {
                is_error: true,
                text: format!("Save blocked because the file failed to load: {error}"),
            });
            cx.notify();
            return;
        }
        if self.tabs.is_empty() {
            return;
        }
        let registry = model::registry();
        let (text, asset) = self.with_doc(cx, |doc| (document::encode(doc, registry), doc.buildable_asset(registry)));
        let written = (|| {
            if let Some(parent) = self.file.parent() {
                engine_fs::virtual_fs::create_dir_all(parent)
                    .map_err(|error| format!("Failed to create directory: {error}"))?;
            }
            engine_fs::virtual_fs::write_file_atomically(&self.file, text.as_bytes())
                .map_err(|error| format!("Failed to write {}: {error}", self.file.display()))
        })();
        match written {
            Ok(()) => {
                for tab in &self.tabs {
                    plugin_editor_api::undo::lock(&tab.history).mark_saved();
                }
                self.status = Some(Status {
                    is_error: false,
                    text: "Saved".to_string(),
                });
                self.apply_to_preview(asset, cx);
            }
            Err(error) => {
                tracing::error!("Render graph save failed: {error}");
                self.status = Some(Status {
                    is_error: true,
                    text: error,
                });
            }
        }
        cx.notify();
    }
}

/// Reads and decodes the file at `file`.
fn load(file: &std::path::Path) -> Result<RenderDocument, String> {
    let bytes = engine_fs::virtual_fs::read_file(file)
        .map_err(|error| format!("Failed to read file: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|error| format!("Render graph is not valid UTF-8: {error}"))?;
    document::decode(&text, model::registry())
}

/// What each comment contains, by comment index, from the laid-out geometry.
fn contained_nodes(canvas: &GraphCanvas) -> Vec<Vec<String>> {
    let scene = canvas.scene();
    canvas
        .graph()
        .comments
        .iter()
        .zip(&scene.comments)
        .map(|(_, placed)| {
            scene
                .nodes
                .iter()
                .filter(|n| {
                    let r = n.rect();
                    r.min.x >= placed.rect.min.x
                        && r.min.y >= placed.rect.min.y
                        && r.max.x <= placed.rect.max.x
                        && r.max.y <= placed.rect.max.y
                })
                .map(|n| n.id.clone())
                .collect()
        })
        .collect()
}

/// Commands wiring `source` (an output pin) to the first compatible input of the
/// freshly added `node`.
fn connect_commands(
    profile: &RenderProfile,
    graph: &GraphDescription,
    source: &graphy::PinRef,
    node: &NodeInstance,
) -> Vec<GraphCommand> {
    let Some(src_pin) = graph
        .nodes
        .get(&source.node)
        .and_then(|n| n.outputs.iter().find(|p| p.id == source.pin))
    else {
        return Vec::new();
    };
    let resolver = profile.type_resolver();
    let Some(target) = node.inputs.iter().find(|p| {
        resolver
            .compatibility(src_pin.data_type(), p.data_type())
            .is_connectable()
    }) else {
        return Vec::new();
    };
    let exec = src_pin.data_type().is_execution();
    let mut commands = Vec::new();
    // An execution output (or a reroute's) drives one wire.
    let single_output = exec
        || graph
            .nodes
            .get(&source.node)
            .is_some_and(|n| n.is_reroute());
    if single_output {
        commands.extend(
            graph
                .connections
                .iter()
                .filter(|c| c.source_node == source.node && c.source_pin == source.pin)
                .cloned()
                .map(GraphCommand::Disconnect),
        );
    }
    commands.push(GraphCommand::Connect(Connection::new(
        &source.node,
        &source.pin,
        &node.id,
        &target.id,
        if exec {
            ConnectionType::Execution
        } else {
            ConnectionType::Data
        },
    )));
    commands
}

