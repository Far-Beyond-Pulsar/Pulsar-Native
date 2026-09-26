//! Tool Mode Registry
//!
//! Owns registered tool modes and the currently active mode ID.
//! Provides lookup, switching, and extensibility seams.

use super::{level_edit::LevelEditMode, ToolMode, ToolModeContext};

// ── ToolModeId ─────────────────────────────────────────────────────────────

/// Strongly-typed identifier for a tool mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ToolModeId(pub &'static str);

impl ToolModeId {
    pub const LEVEL_EDIT: Self = Self("level_edit");
    /// Milestone 5's extensibility-demo mode (`tool_modes::spline::SplineMode`).
    /// Registered via [`register_tool_modes`], never added to
    /// [`ToolModeRegistry::builtin`].
    pub const SPLINE: Self = Self("spline");
    pub const VOXEL_SCULPT: Self = Self("voxel_sculpt");
}

// ── ToolModeRegistry ───────────────────────────────────────────────────────

/// Registry containing all available editor tool modes.
pub struct ToolModeRegistry {
    modes: Vec<Box<dyn ToolMode>>,
    selected: ToolModeId,
}

impl ToolModeRegistry {
    /// Constructs registry with the built-in Level Edit mode.
    pub fn builtin() -> Self {
        let mut registry = Self {
            modes: Vec::new(),
            selected: ToolModeId::LEVEL_EDIT,
        };
        registry.register(Box::new(LevelEditMode::default()));
        registry
    }

    /// Registers a tool mode, replacing any existing registration with matching ID.
    pub fn register(&mut self, mode: Box<dyn ToolMode>) {
        if let Some(pos) = self.modes.iter().position(|m| m.id() == mode.id()) {
            self.modes[pos] = mode;
        } else {
            self.modes.push(mode);
        }
    }

    /// Returns a reference to the active tool mode.
    pub fn selected(&self) -> &dyn ToolMode {
        self.modes
            .iter()
            .find(|m| m.id() == self.selected)
            .map(|m| m.as_ref())
            .unwrap_or_else(|| self.modes[0].as_ref())
    }

    /// Returns a mutable reference to the active tool mode.
    pub fn selected_mut(&mut self) -> &mut dyn ToolMode {
        let selected_id = self.selected;
        if let Some(pos) = self.modes.iter().position(|m| m.id() == selected_id) {
            self.modes[pos].as_mut()
        } else {
            self.modes[0].as_mut()
        }
    }

    /// Returns the ID of the active tool mode.
    pub fn selected_id(&self) -> ToolModeId {
        self.selected
    }

    /// Switches the active tool mode.
    ///
    /// If `ctx` is provided, invokes `on_mode_exited` on the previous mode and
    /// `on_mode_entered` on the newly selected mode.
    pub fn select(&mut self, id: ToolModeId, mut ctx: Option<&mut ToolModeContext>) {
        if self.selected == id {
            return;
        }

        let old_id = self.selected;
        let old_idx = self.modes.iter().position(|m| m.id() == old_id);
        let new_idx = self.modes.iter().position(|m| m.id() == id);

        if let Some(ref mut c) = ctx {
            if let Some(idx) = old_idx {
                self.modes[idx].on_mode_exited(c);
            }
        }

        self.selected = id;

        if let Some(ref mut c) = ctx {
            if let Some(idx) = new_idx {
                self.modes[idx].on_mode_entered(c);
            }
        }
    }

    /// Returns a slice of all registered modes.
    pub fn modes(&self) -> &[Box<dyn ToolMode>] {
        &self.modes
    }

    /// Temporarily swaps the active mode with a placeholder to allow dispatching with `&mut LevelEditorState`.
    pub fn swap_selected(
        &mut self,
        mut placeholder: Box<dyn ToolMode>,
    ) -> (Box<dyn ToolMode>, usize) {
        if let Some(idx) = self.modes.iter().position(|m| m.id() == self.selected) {
            std::mem::swap(&mut self.modes[idx], &mut placeholder);
            (placeholder, idx)
        } else {
            (placeholder, 0)
        }
    }

    /// Restores a swapped mode back to its registry index.
    pub fn restore_swapped(&mut self, idx: usize, mut mode: Box<dyn ToolMode>) {
        if idx < self.modes.len() {
            std::mem::swap(&mut self.modes[idx], &mut mode);
        } else {
            self.modes.push(mode);
        }
    }
}

impl Clone for ToolModeRegistry {
    fn clone(&self) -> Self {
        Self {
            modes: self.modes.iter().map(|m| m.clone_box()).collect(),
            selected: self.selected,
        }
    }
}

impl Default for ToolModeRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

// ── Registration surface for external modes ───────────────────────────────

/// Registers every non-builtin tool mode. This is the design doc's §9 open
/// question ("Registration surface for future external modes"), resolved by
/// Milestone 5 (issue #714): a plain function, called once at editor
/// construction right after [`ToolModeRegistry::builtin`], that adds modes
/// purely through [`ToolModeRegistry::register`].
///
/// `builtin()` itself never grows a third entry -- that it doesn't is the
/// actual point of this milestone. A future out-of-tree/plugin mode would
/// hook in here (or from an equivalent function in its own crate called from
/// this one) rather than by editing `builtin()`.
pub fn register_tool_modes(registry: &mut ToolModeRegistry) {
    registry.register(Box::new(super::spline::SplineMode::default()));
    registry.register(Box::new(super::voxel_sculpt::VoxelSculptMode::default()));
}
