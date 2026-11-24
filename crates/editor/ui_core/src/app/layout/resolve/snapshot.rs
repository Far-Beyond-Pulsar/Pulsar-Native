//! The read-only picture of the live dock a restore resolves against, and the
//! set of live panels a plan has claimed.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{App, Entity, EntityId};
use ui::dock::{DockArea, DockPlacement, PanelView, TabPanel};

/// A live panel available for reuse, and where it lives now.
#[derive(Clone)]
pub(in crate::app::layout) struct LivePanel {
    pub panel: Arc<dyn PanelView>,
    pub placement: DockPlacement,
}

impl LivePanel {
    pub(super) fn id(&self) -> EntityId {
        self.panel.view().entity_id()
    }
}

/// Read-only picture of the live dock, taken before anything is resolved.
pub(in crate::app::layout) struct LiveSnapshot {
    /// Everything the app created itself, with where it sits.
    pub panels: Vec<LivePanel>,
    /// Entity ids of the panels in the anchor tab group (the level editor's).
    pub anchor_panels: HashSet<EntityId>,
}

impl LiveSnapshot {
    pub fn capture(dock_area: &Entity<DockArea>, anchor: &Entity<TabPanel>, cx: &App) -> Self {
        let panels = dock_area
            .read(cx)
            .tab_panels(cx)
            .into_iter()
            .flat_map(|(placement, tabs)| {
                tabs.read(cx)
                    .all_panels()
                    .into_iter()
                    .map(move |panel| LivePanel { panel, placement })
            })
            .collect();
        let anchor_panels = anchor
            .read(cx)
            .all_panels()
            .iter()
            .map(|p| p.view().entity_id())
            .collect();
        Self {
            panels,
            anchor_panels,
        }
    }
}

/// Which live panels have been claimed by a plan so far.
#[derive(Default)]
pub(super) struct Claims(pub(super) HashSet<EntityId>);

impl Claims {
    pub(super) fn contains(&self, id: EntityId) -> bool {
        self.0.contains(&id)
    }
}
