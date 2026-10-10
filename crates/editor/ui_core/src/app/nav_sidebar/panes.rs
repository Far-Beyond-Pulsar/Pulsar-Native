//! The sidebar's stack of panes, like VS Code's side bar: each pane has a
//! header that collapses it and a body that scrolls on its own, and the
//! border between two open panes drags to share the height between them.
//!
//! Panes are listed here, not built into the layout, so another kind is one
//! more [`PaneKind`] and its body.

use serde::{Deserialize, Serialize};

/// What a pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    /// The open editors, grouped.
    Editors,
    /// The project's content folder tree.
    Content,
}

impl PaneKind {
    /// Every kind, in the order a new project lists them.
    pub const ALL: [PaneKind; 2] = [PaneKind::Editors, PaneKind::Content];

    pub fn title(self) -> &'static str {
        match self {
            PaneKind::Editors => "Editors",
            PaneKind::Content => "Content",
        }
    }

    /// The share of the height a new project gives it.
    fn default_weight(self) -> f32 {
        match self {
            PaneKind::Editors => 2.,
            PaneKind::Content => 3.,
        }
    }
}

/// One pane of the stack.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub kind: PaneKind,
    /// Only the header shows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Its share of the height the open panes split, relative to the others.
    pub weight: f32,
}

/// The smallest body an open pane keeps when its border is dragged, in
/// pixels.
pub const MIN_BODY: f32 = 48.;

/// The panes, top to bottom.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneStack {
    panes: Vec<Pane>,
}

impl Default for PaneStack {
    fn default() -> Self {
        Self {
            panes: PaneKind::ALL
                .iter()
                .map(|&kind| Pane {
                    kind,
                    collapsed: false,
                    weight: kind.default_weight(),
                })
                .collect(),
        }
    }
}

impl PaneStack {
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn is_collapsed(&self, kind: PaneKind) -> bool {
        self.panes
            .iter()
            .any(|pane| pane.kind == kind && pane.collapsed)
    }

    pub fn toggle(&mut self, kind: PaneKind) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.kind == kind) {
            pane.collapsed = !pane.collapsed;
        }
    }

    /// The open panes the border under pane `index` moves between: the
    /// nearest open pane at or above it and the nearest below, as in VS Code.
    /// `None` when either side has no open pane, so there is nothing to drag.
    pub fn border_panes(&self, index: usize) -> Option<(usize, usize)> {
        let above = (0..=index.min(self.panes.len().checked_sub(1)?))
            .rev()
            .find(|&i| !self.panes[i].collapsed)?;
        let below = (index + 1..self.panes.len()).find(|&i| !self.panes[i].collapsed)?;
        Some((above, below))
    }

    /// Drag the border between open panes `above` and `below` by `delta`
    /// pixels, given the heights their bodies had when the drag started and
    /// their weights then. Neither body shrinks below [`MIN_BODY`], and the
    /// two keep the height they had together, so the other panes don't move.
    pub fn resize(
        &mut self,
        (above, below): (usize, usize),
        start_heights: (f32, f32),
        start_weights: (f32, f32),
        delta: f32,
    ) {
        let (height_above, height_below) = start_heights;
        let total = height_above + height_below;
        if total <= 0. {
            return;
        }
        let min = MIN_BODY.min(total / 2.);
        let new_above = (height_above + delta).clamp(min, total - min);
        let weight = start_weights.0 + start_weights.1;
        self.panes[above].weight = weight * new_above / total;
        self.panes[below].weight = weight - self.panes[above].weight;
    }

    /// Bring back a saved stack: its panes in its order, without unknown
    /// repeats, followed by any kind it didn't have yet.
    pub fn restored(saved: PaneStack) -> Self {
        let mut panes: Vec<Pane> = Vec::new();
        for pane in saved.panes {
            if !panes.iter().any(|known| known.kind == pane.kind) {
                let weight = if pane.weight.is_finite() && pane.weight > 0. {
                    pane.weight
                } else {
                    pane.kind.default_weight()
                };
                panes.push(Pane { weight, ..pane });
            }
        }
        for kind in PaneKind::ALL {
            if !panes.iter().any(|pane| pane.kind == kind) {
                panes.push(Pane {
                    kind,
                    collapsed: false,
                    weight: kind.default_weight(),
                });
            }
        }
        Self { panes }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_border_moves_height_between_its_two_panes_only() {
        let mut stack = PaneStack::default();
        assert_eq!(stack.border_panes(0), Some((0, 1)));
        let weights = (stack.panes[0].weight, stack.panes[1].weight);
        // 200 px over 300 px, dragged down 100 px.
        stack.resize((0, 1), (200., 300.), weights, 100.);
        let (a, b) = (stack.panes[0].weight, stack.panes[1].weight);
        assert!(
            (a + b - (weights.0 + weights.1)).abs() < 1e-4,
            "the pair keeps its share"
        );
        assert!((a / (a + b) - 0.6).abs() < 1e-4, "300 of 500 px");

        // Dragging past the other body leaves it its minimum.
        stack.resize((0, 1), (200., 300.), weights, 1000.);
        let b = stack.panes[1].weight / (stack.panes[0].weight + stack.panes[1].weight);
        assert!((b * 500. - MIN_BODY).abs() < 1e-3);
    }

    #[test]
    fn collapsed_panes_have_no_border_and_keep_their_share() {
        let mut stack = PaneStack::default();
        stack.toggle(PaneKind::Editors);
        assert!(stack.is_collapsed(PaneKind::Editors));
        assert_eq!(stack.border_panes(0), None, "nothing open above");
        stack.toggle(PaneKind::Editors);
        assert_eq!(stack.border_panes(0), Some((0, 1)));
        assert_eq!(stack.panes[0].weight, PaneKind::Editors.default_weight());
    }

    #[test]
    fn a_saved_stack_keeps_its_order_and_gains_new_kinds() {
        let saved: PaneStack = serde_json::from_str(
            r#"[{"kind":"content","weight":4.0},{"kind":"content","weight":1.0}]"#,
        )
        .unwrap();
        let stack = PaneStack::restored(saved);
        let kinds: Vec<_> = stack.panes().iter().map(|pane| pane.kind).collect();
        assert_eq!(kinds, [PaneKind::Content, PaneKind::Editors]);
        assert_eq!(stack.panes()[0].weight, 4.);

        let json = serde_json::to_string(&stack).unwrap();
        assert_eq!(
            PaneStack::restored(serde_json::from_str(&json).unwrap()),
            stack
        );
    }
}
