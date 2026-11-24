//! The GPUI leaf resolver: claims live panels, rebuilds restorable panels and
//! creates file editors.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Entity, EntityId, Window};
use plugin_manager::{PluginManagerError, RestoreOutcome};
use ui::dock::{LazyPanel, PanelState, PanelView, TabPanel};

use super::super::dormant::{DormantRoot, PendingDormant};
use super::super::owned;

use super::*;

/// The GPUI [`LeafResolver`]: claims live panels, rebuilds restorable panels
/// through the restorer registry, creates file editors.
pub(super) struct Resolver<'a> {
    pub(super) project_root: PathBuf,
    /// Whether live panels may be claimed by name. Pop-outs never claim them:
    /// a live panel sits in the main window's dock and is placed by that root.
    pub(super) claim_live: bool,
    pub(super) snapshot: &'a LiveSnapshot,
    pub(super) claims: &'a mut Claims,
    /// Where each claimed owned panel sits now, so commit can take it out.
    pub(super) homes: &'a mut HashMap<EntityId, Entity<TabPanel>>,
    pub(super) dormant: &'a mut Vec<PendingDormant>,
    /// Which saved root this resolves, for the tabs it defers.
    pub(super) root: DormantRoot,
    /// Files some saved tab names as its owner. Such an editor is built now so
    /// its inner panels can be claimed.
    pub(super) owners: &'a HashSet<String>,
    pub(super) window: &'a mut Window,
    pub(super) cx: &'a mut App,
}

/// Build one sleeping tab from its saved state. Nothing live is claimed: a
/// sleeping tab is never an owned panel or a live singleton.
pub(super) fn build_deferred(
    state: &PanelState,
    project_root: &std::path::Path,
    window: &mut Window,
    cx: &mut App,
) -> Result<Arc<dyn PanelView>, Unresolved> {
    let snapshot = LiveSnapshot {
        panels: Vec::new(),
        anchor_panels: HashSet::new(),
    };
    let (mut claims, mut homes, mut dormant) = (Claims::default(), HashMap::new(), Vec::new());
    let owners = HashSet::new();
    let mut resolver = Resolver {
        project_root: project_root.to_path_buf(),
        claim_live: false,
        snapshot: &snapshot,
        claims: &mut claims,
        homes: &mut homes,
        dormant: &mut dormant,
        root: DormantRoot::Center,
        owners: &owners,
        window,
        cx,
    };
    resolve_in_order(&mut resolver, state)
}

/// How a sleeping tab rebuilds its panel when it is displayed.
pub(crate) fn lazy_builder(project_root: PathBuf) -> ui::dock::LazyBuilder {
    // A tab that was never built still shows its file type's icon.
    ui::dock::set_icon_resolver(|path| {
        let manager = plugin_manager::global()?;
        let manager = manager.read();
        manager
            .get_file_type_for_path(path)
            .map(|ft| ft.icon.clone())
    });
    Arc::new(move |state, window, cx| build_deferred(state, &project_root, window, cx).ok())
}

impl LeafSources for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    fn owned(&mut self, panel_name: &str, owner_file: &str, nth: usize) -> OwnedStep<Self::Leaf> {
        if !owned::owner_exists(owner_file) {
            tracing::info!("layout: the owner {owner_file} of a {panel_name} tab is gone");
            return OwnedStep::OwnerGone;
        }
        // Looked up fresh each time: editors created earlier in this same
        // resolve pass have built their docks by now.
        let live = owned::live_owned(self.cx);
        match owned::find(&live, panel_name, owner_file, nth) {
            Some(found) if !self.claims.contains(found.id()) => {
                self.claims.0.insert(found.id());
                self.homes.insert(found.id(), found.tab_panel.clone());
                OwnedStep::Found(found.panel.clone())
            }
            Some(_) => {
                tracing::debug!("layout: the {panel_name} #{nth} of {owner_file} is already used");
                OwnedStep::Missing
            }
            None => {
                tracing::info!(
                    "layout: no live {panel_name} #{nth} of {owner_file}; keeping its tab dormant"
                );
                OwnedStep::Missing
            }
        }
    }

    fn singleton(&mut self, panel_name: &str) -> Option<Self::Leaf> {
        if !self.claim_live {
            return None;
        }
        let live = self.snapshot.panels.iter().find(|live| {
            !self.claims.contains(live.id()) && live.panel.panel_name(self.cx) == panel_name
        })?;
        self.claims.0.insert(live.id());
        Some(live.panel.clone())
    }

    fn restore(&mut self, kind: &str, data: &serde_json::Value) -> RestoreStep<Self::Leaf> {
        match plugin_manager::panel_restorers().restore(kind, data, self.window, self.cx) {
            RestoreOutcome::Restored(panel) => RestoreStep::Restored(panel),
            RestoreOutcome::Declined => {
                tracing::info!("layout: restorer '{kind}' could not rebuild a tab");
                RestoreStep::Declined
            }
            RestoreOutcome::UnknownKind => {
                tracing::info!("layout: no restorer for '{kind}'; keeping its tab dormant");
                RestoreStep::Unregistered
            }
        }
    }

    fn open_file(&mut self, file: &str) -> FileStep<Self::Leaf> {
        let path = PathBuf::from(file);
        if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
            tracing::info!("layout: skipping {} (no longer exists)", path.display());
            return FileStep::Gone;
        }
        // No plugin manager means nothing is registered to edit the file; keep
        // the tab for a session that has one rather than dropping it.
        let Some(manager) = plugin_manager::global() else {
            return FileStep::NoEditor;
        };
        let mut manager = manager.write();
        manager.set_project_root(Some(self.project_root.clone()));
        crate::app::refresh_plugin_editor_settings(&mut manager);
        match manager.create_editor_for_file(&path, self.window, self.cx) {
            Ok(panel) => FileStep::Opened(panel),
            Err(error) if no_editor_registered(&error) => {
                tracing::info!(
                    "layout: no editor for {} ({error}); keeping its tab dormant",
                    path.display()
                );
                FileStep::NoEditor
            }
            Err(error) => {
                tracing::warn!("layout: could not reopen {}: {error}", path.display());
                FileStep::Failed
            }
        }
    }
}

impl LeafResolver for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    fn resolve_leaf(&mut self, leaf: &PanelState) -> Result<Self::Leaf, Unresolved> {
        let result = resolve_in_order(self, leaf);
        let outcome = match &result {
            Ok(panel) => format!("panel {}", panel.panel_name(self.cx)),
            Err(why) => format!("{why:?}"),
        };
        tracing::info!(
            leaf = %leaf.panel_name,
            file = ?leaf.file(),
            owner = ?leaf.owner(),
            claim_live = self.claim_live,
            "layout: resolved a saved tab -> {outcome}"
        );
        result
    }

    fn shelve(&mut self, dormant: PendingDormant) {
        self.dormant.push(dormant);
    }

    fn sleeping(&mut self, leaf: &PanelState, active: bool) -> Option<Self::Leaf> {
        // Only the displayed tab of the centre group wakes at startup (it has
        // the initial focus); every other tab is restored asleep and wakes when
        // it is displayed. Pop-outs build everything: their window opens at
        // commit.
        let eligible = self.claim_live
            && !(active && self.root == DormantRoot::Center)
            && leaf.owner().is_none()
            && (leaf.file().is_some() || leaf.restore_key().is_some())
            && !leaf.file().is_some_and(|file| self.owners.contains(file))
            // A live panel of this name would be claimed, not built.
            && !self
                .snapshot
                .panels
                .iter()
                .any(|live| live.panel.panel_name(self.cx) == leaf.panel_name);
        eligible.then(|| {
            LazyPanel::sleeping(
                leaf.clone(),
                lazy_builder(self.project_root.clone()),
                self.cx,
            )
        })
    }
}

/// Whether opening a file failed because nothing is registered for it (its
/// plugin is not loaded), as opposed to an editor that exists and failed.
fn no_editor_registered(error: &PluginManagerError) -> bool {
    matches!(
        error,
        PluginManagerError::NoFileTypeForPath { .. }
            | PluginManagerError::NoEditorForFileType { .. }
            | PluginManagerError::EditorNotFound { .. }
            | PluginManagerError::PluginNotFound { .. }
    )
}
