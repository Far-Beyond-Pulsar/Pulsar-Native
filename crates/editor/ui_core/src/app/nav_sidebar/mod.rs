//! Unified left sidebar (Pulsar-Native#1000, prototype).
//!
//! With `editor.navigation.unified_sidebar` on, the centre tab strip is hidden
//! and a sidebar on the left lists the open editors and the project's folders:
//!
//! - **Rail.** By default only a thin column of icons shows, one per open
//!   editor, so the viewport keeps its width.
//! - **Drawer.** Hovering the rail opens the full sidebar *over* the editor
//!   (the viewport does not move). `editor.navigation.sidebar_pinned` keeps it
//!   open beside the editor instead.
//! - **Editors.** Tabs the user pinned come first, then groups the user made
//!   and named, then the rest grouped by editor kind. Groups collapse. A
//!   pinned or grouped file stays listed after its tab closes and reopens in
//!   one click. Rows drag like tabs: onto the editor to split it, or onto a
//!   group's header to move the tab there.
//! - **Content.** The project's `Content` folder tree (or the project folder).
//!   Choosing a folder lists its assets in the bottom file drawer, which in
//!   this mode leaves out its own tree; opening an asset opens its editor tab.
//!
//! Pins, the user's groups, collapsed groups and expanded folders are saved
//! with the project's layout. [`model`] holds that state and the ordering rules; this module
//! connects it to the dock, and [`render`] draws it.

pub(crate) mod model;
mod render;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use engine_state::settings::{global_config, ConfigValue, GlobalSettings, NS_EDITOR};
use gpui::{AppContext as _, Context, Entity, Subscription, Task, Window};
use ui::dock::{DockPlacement, DragPanel, PanelView, TabPanel};
use ui::input::{InputEvent, InputState};

use self::model::{HoverAction, HoverState, SidebarModel, SidebarTab, TabKey, HOVER_CLOSE_DELAY};
use super::PulsarApp;

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

/// Sidebar state kept in [`super::state::AppState`].
#[derive(Default)]
pub struct NavSidebarState {
    pub(crate) model: SidebarModel,
    pub(crate) hover: HoverState,
    close_task: Option<Task<()>>,
    /// The group whose name is being edited, and its text field.
    renaming: Option<(u32, Entity<InputState>)>,
    _rename_events: Option<Subscription>,
}

impl NavSidebarState {
    /// The text field of the group being renamed, when it is `id`.
    pub(crate) fn rename_field(&self, id: u32) -> Option<&Entity<InputState>> {
        self.renaming
            .as_ref()
            .filter(|(renaming, _)| *renaming == id)
            .map(|(_, field)| field)
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

    pub(crate) fn sidebar_hover(&mut self, action: HoverAction, cx: &mut Context<Self>) {
        match action {
            HoverAction::None => return,
            HoverAction::KeepOpen => self.state.nav_sidebar.close_task = None,
            HoverAction::CloseLater => {
                self.state.nav_sidebar.close_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_CLOSE_DELAY).await;
                    _ = this.update(cx, |app, cx| {
                        if app.state.nav_sidebar.hover.close_if_unhovered() {
                            cx.notify();
                        }
                    });
                }));
            }
        }
        cx.notify();
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
        if !pinned_open() {
            self.state.nav_sidebar.hover.close();
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

    pub(crate) fn toggle_sidebar_pin(&mut self, key: &TabKey, cx: &mut Context<Self>) {
        self.state.nav_sidebar.model.toggle_pin(key);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    /// A tab drag of the open tab at `index`, as its tab strip would start.
    pub(crate) fn sidebar_tab_drag(&self, index: usize, cx: &gpui::App) -> Option<DragPanel> {
        let open = self.center_tab_list(cx).into_iter().nth(index)?;
        TabPanel::tab_drag(&open.tabs, open.local_ix, cx)
    }

    /// A tab dropped on a section's header: into that user group, back to its
    /// editor-kind group, or pinned.
    pub(crate) fn drop_on_sidebar_section(
        &mut self,
        section: &model::SectionId,
        drag: &DragPanel,
        cx: &mut Context<Self>,
    ) {
        let panel = drag.panel();
        let file = panel.panel_file_path(cx);
        let key = TabKey::of(panel.panel_name(cx), file.as_deref());
        let model = &mut self.state.nav_sidebar.model;
        match section {
            model::SectionId::Custom(id) => model.move_to_group(&key, Some(*id)),
            model::SectionId::Group(_) => model.move_to_group(&key, None),
            model::SectionId::Pinned => {
                if !model.is_pinned(&key) {
                    model.toggle_pin(&key);
                }
            }
        }
        self.schedule_layout_save(cx);
        cx.notify();
    }

    /// Put a tab in a user group, or with `None` back in its editor-kind group.
    pub(crate) fn move_sidebar_tab(
        &mut self,
        key: &TabKey,
        to: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.state.nav_sidebar.model.move_to_group(key, to);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    /// Make a group, holding `first` if given, and start naming it.
    pub(crate) fn new_sidebar_group(
        &mut self,
        first: Option<TabKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = &mut self.state.nav_sidebar.model;
        let id = model.create_group(model.next_group_name());
        if let Some(key) = first {
            model.move_to_group(&key, Some(id));
        }
        self.schedule_layout_save(cx);
        self.start_sidebar_group_rename(id, window, cx);
    }

    /// Edit a group's name in place; Enter or leaving the field keeps it.
    pub(crate) fn start_sidebar_group_rename(
        &mut self,
        id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .state
            .nav_sidebar
            .model
            .group(id)
            .map(|g| g.name.clone())
        else {
            return;
        };
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name));
        field.update(cx, |field, cx| field.focus(window, cx));
        self.state.nav_sidebar._rename_events =
            Some(cx.subscribe(&field, |app, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    app.finish_sidebar_group_rename(cx);
                }
            }));
        self.state.nav_sidebar.renaming = Some((id, field));
        cx.notify();
    }

    fn finish_sidebar_group_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, field)) = self.state.nav_sidebar.renaming.take() else {
            return;
        };
        self.state.nav_sidebar._rename_events = None;
        let name = field.read(cx).value();
        self.state.nav_sidebar.model.rename_group(id, &name);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    /// Remove a user group; its tabs go back to their editor-kind groups.
    pub(crate) fn delete_sidebar_group(&mut self, id: u32, cx: &mut Context<Self>) {
        if self
            .state
            .nav_sidebar
            .renaming
            .as_ref()
            .is_some_and(|(renaming, _)| *renaming == id)
        {
            self.state.nav_sidebar.renaming = None;
            self.state.nav_sidebar._rename_events = None;
        }
        self.state.nav_sidebar.model.delete_group(id);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn toggle_sidebar_section(&mut self, id: &model::SectionId, cx: &mut Context<Self>) {
        self.state.nav_sidebar.model.toggle_section(id);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn toggle_sidebar_folder(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.state.nav_sidebar.model.toggle_folder(path);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    /// List a folder's assets in the bottom drawer.
    pub(crate) fn show_sidebar_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.state
            .file_manager_drawer
            .update(cx, |drawer, cx| drawer.show_folder(path, cx));
        self.state.drawer_open = true;
        cx.notify();
    }

    pub(crate) fn set_sidebar_pinned_open(&mut self, pinned: bool, cx: &mut Context<Self>) {
        persist_setting("sidebar_pinned", pinned);
        if !pinned {
            self.state.nav_sidebar.hover.close();
        }
        cx.notify();
    }

    pub(crate) fn on_toggle_unified_sidebar(
        &mut self,
        _: &crate::actions::ToggleUnifiedSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        persist_setting("unified_sidebar", !enabled());
        self.state.nav_sidebar.hover.close();
        self.sync_sidebar_mode(window, cx);
        cx.notify();
    }
}
