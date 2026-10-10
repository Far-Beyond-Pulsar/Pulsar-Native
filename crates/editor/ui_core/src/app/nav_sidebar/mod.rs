//! Unified left sidebar (Pulsar-Native#1000, prototype).
//!
//! With `editor.navigation.unified_sidebar` on, the centre tab strip is hidden
//! and a sidebar on the left lists the open editors and the project's content:
//!
//! - **Rail.** By default only a thin column of icons shows, one per open
//!   editor, so the viewport keeps its width.
//! - **Drawer.** Hovering the rail opens the full sidebar *over* the editor
//!   (the viewport does not move). `editor.navigation.sidebar_pinned` keeps it
//!   open beside the editor instead.
//! - **Panes.** The drawer stacks its sections as panes, like VS Code's side
//!   bar ([`panes`]): each has a header that collapses it and a body that
//!   scrolls on its own with a scrollbar, and the border between two open
//!   panes drags to share the height. Another section is one more
//!   [`panes::PaneKind`].
//! - **Editors.** Tabs the user pinned come first, then groups the user made
//!   and named, then the rest grouped by editor kind. Groups collapse. A
//!   pinned or grouped file stays listed after its tab closes and reopens in
//!   one click. Rows drag like tabs: onto the editor to split it, or onto a
//!   group's header to move the tab there.
//! - **Content.** The project's `Content` folder tree (or the project folder),
//!   with the files in each expanded folder (#1139). Double-clicking a file,
//!   or a folder-based asset such as a blueprint class, opens it in its
//!   editor; double-clicking any other folder lists it in the bottom file
//!   drawer. The icon at a hovered row's end does the same for one click:
//!   "Reveal in file drawer" lists a file's folder with the file selected,
//!   "Open in file drawer" lists a folder. The right-click menu has both, and
//!   renames, deletes, cuts, copies and pastes, through the file drawer's
//!   operations and clipboard.
//!
//! The sidebar is its own entity, [`NavSidebar`]: hovering, expanding a
//! folder, selecting a row or renaming notify the sidebar, not [`PulsarApp`].
//! Only what changes the editor (choosing or closing a tab, opening a file,
//! showing the file drawer) goes through [`PulsarApp`]. The hover drawer is a
//! second, small view ([`NavSidebarOverlay`]) drawn over the editor.
//!
//! The sidebar is drawn as a `.cached()` view, so redrawing the editor replays
//! it rather than rebuilding it.
//!
//! Pins, the user's groups, collapsed groups, expanded folders and the panes'
//! sizes and collapsed state are saved with the project's layout. [`model`] holds that state and the ordering
//! rules; this module connects it to the dock and the file drawer, [`render`]
//! draws the editors and [`content`] the content tree.

mod content;
mod content_list;
pub(crate) mod model;
pub(crate) mod panes;
mod render;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use engine_state::settings::{global_config, ConfigValue, GlobalSettings, NS_EDITOR};
use gpui::{AppContext as _, Context, Entity, Subscription, Task, WeakEntity, Window};
use ui::dock::{DockPlacement, DragPanel, PanelView, TabPanel};
use ui::input::{InputEvent, InputState};
use ui_file_manager::FileManagerDrawer;

use self::model::{HoverAction, HoverState, SidebarModel, SidebarTab, TabKey, HOVER_CLOSE_DELAY};
use self::panes::PaneKind;
use super::PulsarApp;

pub(crate) use self::render::NavSidebarOverlay;

const OWNER: &str = "navigation";

fn setting(key: &str) -> bool {
    global_config()
        .get(NS_EDITOR, OWNER, key)
        .ok()
        .and_then(|value| value.as_bool().ok())
        .unwrap_or(false)
}

fn persist_setting(key: &str, value: bool) {
    let Some(handle) = global_config().owner_handle(NS_EDITOR, OWNER) else {
        return;
    };
    if let Err(error) = handle.set(key, ConfigValue::Bool(value)) {
        tracing::warn!(%error, key, "could not update the navigation setting");
        return;
    }
    if let Err(error) = GlobalSettings::new().save_owner_keys(OWNER, &[key]) {
        tracing::warn!(%error, key, "could not save the navigation setting");
    }
}

/// The sidebar replaces the tab strip.
pub(crate) fn enabled() -> bool {
    setting("unified_sidebar")
}

/// The sidebar stays open beside the editor rather than opening on hover.
pub(crate) fn pinned_open() -> bool {
    setting("sidebar_pinned")
}

/// The left sidebar: its state, and the actions its rows take.
pub struct NavSidebar {
    app: WeakEntity<PulsarApp>,
    drawer: Entity<FileManagerDrawer>,
    pub(crate) model: SidebarModel,
    pub(crate) hover: HoverState,
    close_task: Option<Task<()>>,
    /// The group whose name is being edited, and its text field.
    renaming: Option<(u32, Entity<InputState>)>,
    _rename_events: Option<Subscription>,
    /// The content-tree row last clicked.
    selected: Option<PathBuf>,
    /// The content-tree file or folder being renamed, and its text field.
    renaming_path: Option<(PathBuf, Entity<InputState>)>,
    _path_rename_events: Option<Subscription>,
    /// Each pane's scrolling and the height its body last had.
    pane_views: std::collections::HashMap<PaneKind, PaneView>,
    /// The pane border being dragged.
    border_drag: Option<BorderDrag>,
    /// The content tree's rows, rebuilt when `content_key` changes.
    content_list: std::rc::Rc<content_list::ContentList>,
    /// The top row of the content tree, which is not part of the list.
    content_root_row: Option<model::FolderRow>,
    /// `(expanded folders, drawer content)` revisions `content_list` was built at.
    content_key: Option<(u64, u64)>,
    /// The editors pane's rows, set each time the drawer is drawn.
    editor_list: std::rc::Rc<render::EditorList>,
    /// How many times it has rendered, for tests that check it replays.
    #[cfg(test)]
    pub(crate) renders: usize,
}

/// What a pane keeps between renders.
pub(crate) struct PaneView {
    pub scroll: gpui::UniformListScrollHandle,
    pub scrollbar: ui::scroll::ScrollbarState,
    /// The body's height when it was last drawn, for dragging a border.
    pub height: std::rc::Rc<std::cell::Cell<f32>>,
}

/// A pane border being dragged: the two open panes it moves between, and
/// what they were when the drag started.
struct BorderDrag {
    panes: (usize, usize),
    start_y: gpui::Pixels,
    heights: (f32, f32),
    weights: (f32, f32),
}

impl NavSidebar {
    pub(crate) fn new(app: WeakEntity<PulsarApp>, drawer: Entity<FileManagerDrawer>) -> Self {
        Self {
            app,
            drawer,
            model: SidebarModel::default(),
            hover: HoverState::default(),
            close_task: None,
            renaming: None,
            _rename_events: None,
            selected: None,
            renaming_path: None,
            _path_rename_events: None,
            pane_views: PaneKind::ALL
                .into_iter()
                .map(|kind| {
                    let view = PaneView {
                        scroll: gpui::UniformListScrollHandle::new(),
                        scrollbar: Default::default(),
                        height: Default::default(),
                    };
                    (kind, view)
                })
                .collect(),
            border_drag: None,
            content_list: Default::default(),
            content_root_row: None,
            content_key: None,
            editor_list: Default::default(),
            #[cfg(test)]
            renders: 0,
        }
    }

    /// Redraw when the project's folders or files change on disk, so the
    /// content tree stays current without anything else notifying the sidebar.
    pub(crate) fn observe_drawer(&mut self, cx: &mut Context<Self>) {
        let drawer = self.drawer.clone();
        cx.observe(&drawer, |sidebar, drawer, cx| {
            let revision = drawer.read(cx).content_revision();
            if sidebar.content_key.map(|(_, drawer)| drawer) != Some(revision) {
                cx.notify();
            }
        })
        .detach();
    }

    /// The text field of the group being renamed, when it is `id`.
    pub(crate) fn rename_field(&self, id: u32) -> Option<&Entity<InputState>> {
        self.renaming
            .as_ref()
            .filter(|(renaming, _)| *renaming == id)
            .map(|(_, field)| field)
    }

    /// Ask the editor to save its layout soon, which includes this sidebar.
    fn save_layout(&self, cx: &mut Context<Self>) {
        _ = self
            .app
            .update(cx, |app, cx| app.schedule_layout_save(cx));
    }

    fn apply_hover(&mut self, action: HoverAction, cx: &mut Context<Self>) {
        match action {
            HoverAction::None => return,
            HoverAction::KeepOpen => self.close_task = None,
            HoverAction::CloseLater => {
                self.close_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_CLOSE_DELAY).await;
                    _ = this.update(cx, |sidebar, cx| {
                        if sidebar.hover.close_if_unhovered() {
                            cx.notify();
                        }
                    });
                }));
            }
        }
        cx.notify();
    }

    pub(crate) fn hover_rail(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let action = self.hover.set_rail(hovered);
        self.apply_hover(action, cx);
    }

    pub(crate) fn hover_drawer(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let action = self.hover.set_drawer(hovered);
        self.apply_hover(action, cx);
    }

    pub(crate) fn pane_view(&self, kind: PaneKind) -> &PaneView {
        &self.pane_views[&kind]
    }

    /// Collapse or expand a pane.
    pub(crate) fn toggle_pane(&mut self, kind: PaneKind, cx: &mut Context<Self>) {
        self.model.panes.toggle(kind);
        self.save_layout(cx);
        cx.notify();
    }

    /// Start dragging the border between open panes `above` and `below`.
    pub(crate) fn start_border_drag(
        &mut self,
        (above, below): (usize, usize),
        y: gpui::Pixels,
        cx: &mut Context<Self>,
    ) {
        let panes = self.model.panes.panes();
        let height = |index: usize| self.pane_views[&panes[index].kind].height.get();
        self.border_drag = Some(BorderDrag {
            panes: (above, below),
            start_y: y,
            heights: (height(above), height(below)),
            weights: (panes[above].weight, panes[below].weight),
        });
        // The pointer may leave the hover drawer while dragging.
        self.hold_open(true, cx);
        cx.notify();
    }

    pub(crate) fn drag_border(&mut self, y: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &self.border_drag else {
            return;
        };
        let delta = f32::from(y - drag.start_y);
        self.model
            .panes
            .resize(drag.panes, drag.heights, drag.weights, delta);
        cx.notify();
    }

    pub(crate) fn end_border_drag(&mut self, cx: &mut Context<Self>) {
        if self.border_drag.take().is_some() {
            self.hold_open(false, cx);
            self.save_layout(cx);
            cx.notify();
        }
    }

    pub(crate) fn is_dragging_border(&self, index: usize) -> bool {
        self.border_drag
            .as_ref()
            .is_some_and(|drag| drag.panes.0 <= index && index < drag.panes.1)
    }

    /// Keep the hover drawer open while a menu opened from it is up.
    pub(crate) fn hold_open(&mut self, held: bool, cx: &mut Context<Self>) {
        let action = self.hover.set_held(held);
        self.apply_hover(action, cx);
    }

    /// Close the hover drawer, unless it is kept open.
    pub(crate) fn close_hover(&mut self, cx: &mut Context<Self>) {
        if !pinned_open() && self.hover.is_open() {
            self.hover.close();
            cx.notify();
        }
    }

    /// Show the tab a row stands for, reopening a closed pinned file.
    pub(crate) fn activate_tab(
        &mut self,
        tab: &SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = tab.clone();
        _ = self
            .app
            .update(cx, |app, cx| app.activate_sidebar_tab(&tab, window, cx));
        self.close_hover(cx);
    }

    /// Close an open tab. The last tab stays, as it does in the tab strip.
    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        _ = self
            .app
            .update(cx, |app, cx| app.close_sidebar_tab(index, window, cx));
    }

    pub(crate) fn toggle_pin(&mut self, key: &TabKey, cx: &mut Context<Self>) {
        self.model.toggle_pin(key);
        self.save_layout(cx);
        cx.notify();
    }

    /// A tab dropped on a section's header: into that user group, back to its
    /// editor-kind group, or pinned.
    pub(crate) fn drop_on_section(
        &mut self,
        section: &model::SectionId,
        drag: &DragPanel,
        cx: &mut Context<Self>,
    ) {
        let panel = drag.panel();
        let file = panel.panel_file_path(cx);
        let key = TabKey::of(panel.panel_name(cx), file.as_deref());
        match section {
            model::SectionId::Custom(id) => self.model.move_to_group(&key, Some(*id)),
            model::SectionId::Group(_) => self.model.move_to_group(&key, None),
            model::SectionId::Pinned => {
                if !self.model.is_pinned(&key) {
                    self.model.toggle_pin(&key);
                }
            }
        }
        self.save_layout(cx);
        cx.notify();
    }

    /// Put a tab in a user group, or with `None` back in its editor-kind group.
    pub(crate) fn move_tab(&mut self, key: &TabKey, to: Option<u32>, cx: &mut Context<Self>) {
        self.model.move_to_group(key, to);
        self.save_layout(cx);
        cx.notify();
    }

    /// Make a group, holding `first` if given, and start naming it.
    pub(crate) fn new_group(
        &mut self,
        first: Option<TabKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.model.create_group(self.model.next_group_name());
        if let Some(key) = first {
            self.model.move_to_group(&key, Some(id));
        }
        self.save_layout(cx);
        self.start_group_rename(id, window, cx);
    }

    /// Edit a group's name in place; Enter or leaving the field keeps it.
    pub(crate) fn start_group_rename(
        &mut self,
        id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.model.group(id).map(|g| g.name.clone()) else {
            return;
        };
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name));
        field.update(cx, |field, cx| field.focus(window, cx));
        self._rename_events = Some(cx.subscribe(&field, |sidebar, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                sidebar.finish_group_rename(cx);
            }
        }));
        self.renaming = Some((id, field));
        cx.notify();
    }

    fn finish_group_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, field)) = self.renaming.take() else {
            return;
        };
        self._rename_events = None;
        let name = field.read(cx).value();
        self.model.rename_group(id, &name);
        self.save_layout(cx);
        cx.notify();
    }

    /// Remove a user group; its tabs go back to their editor-kind groups.
    pub(crate) fn delete_group(&mut self, id: u32, cx: &mut Context<Self>) {
        if self
            .renaming
            .as_ref()
            .is_some_and(|(renaming, _)| *renaming == id)
        {
            self.renaming = None;
            self._rename_events = None;
        }
        self.model.delete_group(id);
        self.save_layout(cx);
        cx.notify();
    }

    pub(crate) fn toggle_section(&mut self, id: &model::SectionId, cx: &mut Context<Self>) {
        self.model.toggle_section(id);
        self.save_layout(cx);
        cx.notify();
    }

    pub(crate) fn set_pinned_open(&mut self, pinned: bool, cx: &mut Context<Self>) {
        persist_setting("sidebar_pinned", pinned);
        if !pinned {
            self.hover.close();
        }
        cx.notify();
        // The sidebar's width changed, so the editor beside it moves.
        _ = self.app.update(cx, |_, cx| cx.notify());
    }

    pub(crate) fn toggle_file_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        _ = self
            .app
            .update(cx, |app, cx| app.toggle_drawer(window, cx));
    }

    // ---- Content tree --------------------------------------------------

    /// The content-tree row last clicked.
    pub(crate) fn selected_path(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub(crate) fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.model.toggle_folder(path);
        self.save_layout(cx);
        cx.notify();
    }

    /// Clicking a folder selects it and expands or collapses it.
    pub(crate) fn click_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.model.toggle_folder(&path);
        self.selected = Some(path);
        self.save_layout(cx);
        cx.notify();
    }

    pub(crate) fn select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&path) {
            self.selected = Some(path);
            cx.notify();
        }
    }

    /// Open a file in its editor.
    pub(crate) fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        _ = self
            .app
            .update(cx, |app, cx| app.open_path(path, window, cx));
        self.close_hover(cx);
        cx.notify();
    }

    /// List a folder's assets in the bottom file drawer.
    pub(crate) fn open_in_drawer(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.drawer
            .update(cx, |drawer, cx| drawer.show_folder(folder, cx));
        self.show_file_drawer(cx);
    }

    /// List a file's folder in the bottom file drawer, with the file selected.
    pub(crate) fn reveal_in_drawer(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.drawer.update(cx, |drawer, cx| drawer.reveal(path, cx));
        self.show_file_drawer(cx);
    }

    fn show_file_drawer(&mut self, cx: &mut Context<Self>) {
        _ = self.app.update(cx, |app, cx| {
            app.state.drawer_open = true;
            cx.notify();
        });
        self.close_hover(cx);
    }

    /// Whether the file drawer's clipboard holds something to paste.
    pub(crate) fn can_paste(&self, cx: &gpui::App) -> bool {
        self.drawer.read(cx).has_clipboard()
    }

    /// Cut or copy a file or folder, onto the file drawer's clipboard.
    pub(crate) fn put_on_clipboard(&mut self, path: PathBuf, cut: bool, cx: &mut Context<Self>) {
        self.drawer
            .update(cx, |drawer, _| drawer.set_clipboard(vec![path], cut));
    }

    /// Paste the clipboard into `folder`, and show it there.
    pub(crate) fn paste_into(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let pasted = self
            .drawer
            .update(cx, |drawer, cx| drawer.paste_into(&folder, cx));
        if !self.model.folder_expanded(&folder) {
            self.model.toggle_folder(&folder);
            self.save_layout(cx);
        }
        if let Some(first) = pasted.into_iter().next() {
            self.selected = Some(first);
        }
        cx.notify();
    }

    pub(crate) fn delete_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.drawer
            .update(cx, |drawer, cx| drawer.delete_paths(std::slice::from_ref(&path), cx));
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.starts_with(&path))
        {
            self.selected = None;
        }
        cx.notify();
    }

    /// The text field of the content-tree row being renamed, when it is `path`.
    pub(crate) fn path_rename_field(&self, path: &Path) -> Option<&Entity<InputState>> {
        self.renaming_path
            .as_ref()
            .filter(|(renaming, _)| renaming == path)
            .map(|(_, field)| field)
    }

    /// Rename a file or folder in place; Enter or leaving the field applies it.
    pub(crate) fn start_path_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name));
        field.update(cx, |field, cx| field.focus(window, cx));
        self._path_rename_events =
            Some(cx.subscribe(&field, |sidebar, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    sidebar.finish_path_rename(cx);
                }
            }));
        self.selected = Some(path.clone());
        self.renaming_path = Some((path, field));
        cx.notify();
    }

    pub(crate) fn finish_path_rename(&mut self, cx: &mut Context<Self>) {
        let Some((old, field)) = self.renaming_path.take() else {
            return;
        };
        self._path_rename_events = None;
        let name = field.read(cx).value().to_string();
        let renamed = self
            .drawer
            .update(cx, |drawer, cx| drawer.rename_path(&old, &name, cx));
        match renamed {
            Ok(new) => {
                self.model.folder_moved(&old, &new);
                if let Some(selected) = &self.selected {
                    if let Ok(rest) = selected.strip_prefix(&old) {
                        self.selected = Some(new.join(rest));
                    }
                }
                self.save_layout(cx);
            }
            Err(error) => tracing::warn!(%error, ?old, "could not rename"),
        }
        cx.notify();
    }
}

/// An open editor tab and where it lives.
struct OpenTab {
    tabs: Entity<TabPanel>,
    local_ix: usize,
    panel: Arc<dyn PanelView>,
}

impl PulsarApp {
    /// Every editor tab in the centre area, in order, across splits.
    fn center_tab_list(&self, cx: &gpui::App) -> Vec<OpenTab> {
        self.state
            .dock_area
            .read(cx)
            .tab_panels(cx)
            .into_iter()
            .filter(|(placement, _)| *placement == DockPlacement::Center)
            .flat_map(|(_, tabs)| {
                tabs.read(cx)
                    .all_panels()
                    .into_iter()
                    .enumerate()
                    .map(move |(local_ix, panel)| OpenTab {
                        tabs: tabs.clone(),
                        local_ix,
                        panel,
                    })
            })
            .collect()
    }

    /// The open editors as the sidebar lists them.
    pub(crate) fn sidebar_tabs(&self, cx: &gpui::App) -> Vec<SidebarTab> {
        self.center_tab_list(cx)
            .into_iter()
            .enumerate()
            .map(|(index, open)| {
                let panel = &open.panel;
                let kind = panel.panel_name(cx).to_string();
                let file = panel.panel_file_path(cx);
                SidebarTab {
                    key: TabKey::of(&kind, file.as_deref()),
                    index: Some(index),
                    title: panel
                        .tab_name(cx)
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| kind.clone()),
                    icon: panel.tab_icon(cx),
                    active: open.tabs.read(cx).active_tab_index() == Some(open.local_ix),
                    unsaved: panel.tab_unsaved(cx),
                    kind,
                }
            })
            .collect()
    }

    /// `editor_area` (the dock and its overlays) with the sidebar beside it, or
    /// unchanged while the sidebar is off.
    pub(crate) fn with_nav_sidebar(&self, editor_area: gpui::Div) -> gpui::AnyElement {
        use gpui::{InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _};
        if !enabled() {
            return editor_area.into_any_element();
        }
        let width = if pinned_open() {
            render::DRAWER_WIDTH
        } else {
            render::RAIL_WIDTH
        };
        ui::h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_stretch()
            // Cached, so redrawing the editor replays the sidebar instead of
            // rebuilding it.
            .child(
                gpui::AnyView::from(self.state.nav_sidebar.clone()).cached(
                    gpui::StyleRefinement::default()
                        .w(gpui::px(width))
                        .h_full()
                        .flex_none(),
                ),
            )
            .child(
                editor_area
                    .h_full()
                    .min_w_0()
                    .debug_selector(|| "nav-sidebar-editor-area".into())
                    // After the editor, so the hover drawer lies on top of it.
                    .child(self.state.nav_overlay.clone()),
            )
            .into_any_element()
    }

    /// Show the tab a sidebar row stands for, reopening a closed pinned file.
    pub(crate) fn activate_sidebar_tab(
        &mut self,
        tab: &SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match tab.index {
            Some(index) => {
                if let Some(open) = self.center_tab_list(cx).into_iter().nth(index) {
                    open.tabs.update(cx, |tabs, cx| {
                        tabs.set_active_tab(open.local_ix, window, cx)
                    });
                }
            }
            None => {
                if let TabKey::File(path) = &tab.key {
                    self.open_path(path.clone(), window, cx);
                }
            }
        }
        self.refresh_open_editor_snapshot(cx);
        cx.notify();
    }

    /// Close an open tab from the sidebar. The last tab stays, as it does in
    /// the tab strip.
    pub(crate) fn close_sidebar_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = self.center_tab_list(cx);
        if list.len() <= 1 {
            return;
        }
        let Some(open) = list.into_iter().nth(index) else {
            return;
        };
        if !open.panel.closable(cx) {
            return;
        }
        open.tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(open.panel.clone(), window, cx)
        });
        self.refresh_open_editor_snapshot(cx);
        cx.notify();
    }

    /// A tab drag of the open tab at `index`, as its tab strip would start.
    #[cfg(test)]
    pub(crate) fn sidebar_tab_drag(&self, index: usize, cx: &gpui::App) -> Option<DragPanel> {
        let open = self.center_tab_list(cx).into_iter().nth(index)?;
        TabPanel::tab_drag(&open.tabs, open.local_ix, cx)
    }

    /// Match the dock and the file drawer to the setting: no tab strip, and no
    /// second folder tree in the drawer, while the sidebar is on.
    pub(crate) fn sync_sidebar_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = enabled();
        let stale: Vec<Entity<TabPanel>> = self
            .state
            .dock_area
            .read(cx)
            .tab_panels(cx)
            .into_iter()
            .filter(|(placement, tabs)| {
                *placement == DockPlacement::Center && tabs.read(cx).tab_bar_hidden() != on
            })
            .map(|(_, tabs)| tabs)
            .collect();
        let drawer = self.state.file_manager_drawer.clone();
        let drawer_stale = drawer.read(cx).folder_tree_hidden() != on;
        if stale.is_empty() && !drawer_stale {
            return;
        }
        // Applied after this frame: the entities are not updated mid-render.
        cx.defer_in(window, move |_, _, cx| {
            for tabs in stale {
                tabs.update(cx, |tabs, cx| tabs.set_tab_bar_hidden(on, cx));
            }
            drawer.update(cx, |drawer, cx| drawer.set_folder_tree_hidden(on, cx));
        });
    }

    pub(crate) fn on_toggle_unified_sidebar(
        &mut self,
        _: &crate::actions::ToggleUnifiedSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        persist_setting("unified_sidebar", !enabled());
        self.state.nav_sidebar.update(cx, |sidebar, cx| {
            sidebar.hover.close();
            cx.notify();
        });
        self.sync_sidebar_mode(window, cx);
        cx.notify();
    }
}
