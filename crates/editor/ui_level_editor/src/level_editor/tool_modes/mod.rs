//! Level Editor Tool Modes
//!
//! Provides modular editing modes (e.g. Level Edit, Terrain) via a pluggable
//! registry pattern. Each mode encapsulates its identity, presentational widgets,
//! brush cursor rendering, status bar readouts, and pointer event handling.

pub mod dispatcher;
pub mod level_edit;
pub mod registry;
pub mod spline;
pub mod terrain;

use gpui::{App, Window};
use std::sync::Arc;
use std::sync::Mutex;

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::services::terrain_edit::TerrainEditApi;

pub use dispatcher::*;
pub use level_edit::*;
pub use registry::*;
pub use spline::*;
pub use terrain::*;

use crate::level_editor::state::LevelEditorState;

// ── Frames & Cursor ────────────────────────────────────────────────────────

/// Camera snapshot for mode raycasting and spatial math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFrame {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

impl Default for CameraFrame {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            fov: 60.0,
        }
    }
}

/// Viewport dimensions in logical/screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ViewportFrame {
    pub width: f32,
    pub height: f32,
}

/// Visual brush cursor ring or disk for spatial editing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushCursor {
    pub center: [f32; 3],
    pub radius: f32,
    pub color: [f32; 4],
}

// ── Pointer Events & Results ───────────────────────────────────────────────

/// Type of pointer action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerKind {
    Down,
    Up,
    Drag,
    Hover,
    Scroll { delta_y: f32 },
}

/// Normalized viewport coordinates and input state passed to active tool mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolPointerEvent {
    pub kind: PointerKind,
    pub button: Option<gpui::MouseButton>,
    pub norm_x: f32,
    pub norm_y: f32,
    pub holding_mods: gpui::Modifiers,
}

/// Result returned from [`ToolMode::on_pointer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPointerResult {
    /// Mode consumed the event; default editor interaction will not run.
    Consumed,
    /// Mode passed the event through; editor will perform default object pick/transform gizmo.
    PassThrough,
}

// ── Declarative Toolbar Widgets ────────────────────────────────────────────

/// Declarative toolbar widget that a mode requests the toolbar shell to render.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolWidget {
    Slider {
        id: &'static str,
        label_key: &'static str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
    },
    Segmented {
        id: &'static str,
        options: Vec<(&'static str, &'static str)>,
        selected: &'static str,
    },
    Toggle {
        id: &'static str,
        label_key: &'static str,
        on: bool,
    },
    /// A one-shot command button. Unlike the other widgets it carries no
    /// value: pressing it is the whole message, dispatched as
    /// [`ToolWidgetEdit::Invoke`].
    ///
    /// No icon field on purpose — `ui::IconName` is not `PartialEq`, and this
    /// enum's `PartialEq` is what `ToolbarSignature` uses to notice that a
    /// mode's controls changed.
    Action {
        id: &'static str,
        label_key: &'static str,
    },
    Divider,
    /// A section heading with no control of its own — groups the widgets
    /// that follow it under a label within one tab. Unlike [`PanelTab`],
    /// this doesn't split content into separate switchable pages; it is for
    /// a tab whose own content is long enough to want internal structure
    /// (e.g. Foliage's "Rendering" / "Wind" groups within one tab).
    Section { label_key: &'static str },
}

// ── Panel Tabs ──────────────────────────────────────────────────────────────

/// One page of a mode's dedicated left-hand panel
/// ([`ModeLayout::show_mode_panel`]).
///
/// A mode with a small, single-purpose control set doesn't need this — the
/// default [`ToolMode::panel_tabs`] wraps `toolbar_controls()` in one
/// unnamed tab. Override it once a mode's panel content is large enough to
/// want its own categories (Terrain's Sculpt vs. Foliage, each with many
/// more controls than would fit — or make sense — in a single flat list).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelTab {
    /// Stable identity for the tab, used only to remember which tab was
    /// selected across re-renders — never shown to the user.
    pub id: &'static str,
    /// i18n key for the tab's label.
    pub label_key: &'static str,
    pub widgets: Vec<ToolWidget>,
}

// ── Status Bar Readout ─────────────────────────────────────────────────────

/// Mode-provided status text and tooltip for the editor status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReadout {
    pub text: String,
    pub tooltip: Option<String>,
}

// ── Mode Layout ─────────────────────────────────────────────────────────────

/// Declarative panel layout a tool mode wants the editor shell to show.
///
/// Pure data, like [`ToolWidget`]/[`StatusReadout`] — modes never touch dock
/// panel entities directly, keeping the trait object-safe and GPUI-agnostic
/// (§4.1's contract). The shell (`ui/panel.rs`'s `LevelEditorPanel::sync_mode_layout`)
/// reconciles the actual dock area to match whenever the active mode changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeLayout {
    /// Whether the right-hand dock (Hierarchy / Properties / World Settings)
    /// should be visible while this mode is active. `false` lets a mode that
    /// wants the whole window for itself (e.g. a full-viewport paint tool)
    /// ask for a cleaner canvas.
    pub show_right_dock: bool,
    /// Whether this mode wants its `toolbar_controls()` widgets rendered in a
    /// dedicated left-hand dock panel instead of the horizontal toolbar
    /// strip. Use this once a mode's control count would otherwise crowd the
    /// toolbar (see design doc's tool-modes-layout addendum) — the shell
    /// renders the *same* `toolbar_controls()` data either way, just
    /// vertically, and skips adding it to the toolbar so it never appears in
    /// both places at once.
    pub show_mode_panel: bool,
}

impl Default for ModeLayout {
    /// Today's behavior for every mode that doesn't override `layout()`:
    /// right dock visible, controls in the toolbar. Matches `LevelEditMode`
    /// exactly, so adding this method to the trait was a no-op for it.
    fn default() -> Self {
        Self {
            show_right_dock: true,
            show_mode_panel: false,
        }
    }
}

// ── Contributed Dock Panels ──────────────────────────────────────────────────

/// Which dock a mode-contributed panel ([`ModePanelDescriptor`]) joins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModePanelPlacement {
    /// Join the left-hand mode-tools dock, sharing its native tab strip with
    /// the [`ModeToolsPanel`](crate::level_editor::workspace::panels::mode_tools::ModeToolsPanel)
    /// tabs (Terrain's brush palette lands here).
    Left,
    /// Join the right-hand dock alongside Hierarchy / Properties / World
    /// Settings.
    Right,
}

/// One dock panel a tool mode wants the editor shell to add while it is active.
///
/// The declarative half of the "full GPUI in a mode" extension point — a
/// descriptor only says *what* panel exists and where it docks; the actual
/// GPUI view comes from [`ToolMode::build_panel`]. Modes that only need a
/// control strip keep using `layout()`/`panel_tabs()`/`toolbar_controls()`
/// and never return any of these (see the design doc's §11).
#[derive(Clone, Debug)]
pub struct ModePanelDescriptor {
    /// Stable identity for the panel. `build_panel` matches on it to decide
    /// what to construct, and the shell uses it to track which contributed
    /// panels are currently open so it can tear them down on mode switch.
    pub id: &'static str,
    /// i18n key for the panel's dock title.
    pub title_key: &'static str,
    /// Tab icon, if any. `Option<IconName>` rather than a bare icon so a
    /// panel that only wants a text tab needs no expensive lookups, and —
    /// because `IconName` is not `PartialEq` — this stays out of any
    /// signature struct the shell compares.
    pub icon: Option<ui::IconName>,
    /// Which dock the panel should be added to.
    pub placement: ModePanelPlacement,
}

// ── Mode Context ───────────────────────────────────────────────────────────

/// Execution context provided to tool mode operations.
pub struct ToolModeContext<'a> {
    pub state: &'a mut LevelEditorState,
    pub gpu_engine: &'a Mutex<GpuRenderer>,
    /// Voxel terrain seam (design doc §5.4). `None` when the renderer has not
    /// initialized yet; modes must treat that as "terrain editing is
    /// unavailable" and pass the event through, not as an error.
    ///
    /// This is what keeps `TerrainMode` off raw `TerrainRuntimeHandle`s: it
    /// never sees the runtime, only this seam.
    pub terrain: Option<&'a TerrainEditApi>,
    pub camera: CameraFrame,
    pub viewport: ViewportFrame,
}

// ── ToolMode Trait ─────────────────────────────────────────────────────────

/// Contract implemented by all level editor tool modes.
pub trait ToolMode: Send + Sync {
    /// Unique identifier for this tool mode.
    fn id(&self) -> ToolModeId;

    /// Localization key for the mode's display label.
    fn label_key(&self) -> &'static str;

    /// Icon representing this mode in the toolbar and mode indicator.
    fn icon(&self) -> ui::IconName;

    /// Localization key for the mode's description tooltip.
    fn description_key(&self) -> &'static str;

    /// Called when switching into this mode.
    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext);

    /// Called when switching out of this mode.
    fn on_mode_exited(&mut self, ctx: &mut ToolModeContext);

    /// Optional 3D brush cursor ring/mesh to display in the viewport.
    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        None
    }

    /// Declarative widgets to display in the editor toolbar when active.
    fn toolbar_controls(&self, _ctx: &ToolModeContext) -> Vec<ToolWidget> {
        Vec::new()
    }

    /// Status readout to surface on the status bar when active.
    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        None
    }

    /// Panel layout this mode wants while active. See [`ModeLayout`].
    fn layout(&self) -> ModeLayout {
        ModeLayout::default()
    }

    /// Tabs for this mode's dedicated left-hand panel. Only consulted when
    /// [`ModeLayout::show_mode_panel`] is `true`; ignored otherwise (the
    /// toolbar strip always reads `toolbar_controls()` directly, never this).
    ///
    /// Default: one unlabeled tab wrapping `toolbar_controls()`, so a mode
    /// that turns on `show_mode_panel` without overriding this still gets a
    /// working (if unorganized) panel rather than an empty one.
    fn panel_tabs(&self, ctx: &ToolModeContext) -> Vec<PanelTab> {
        vec![PanelTab {
            id: "default",
            label_key: "LevelEditor.ModeTools.DefaultTab",
            widgets: self.toolbar_controls(ctx),
        }]
    }

    /// Dock panels this mode contributes to the level editor while active.
    ///
    /// Default: none. Modes that want their own panels (not just control
    /// strips) override this together with [`Self::build_panel`] — see
    /// [`ModePanelDescriptor`] and the design doc's §11. The shell reconciles
    /// the dock area to match this set whenever the active mode changes
    /// (`ui/panel.rs`'s `LevelEditorPanel::sync_mode_layout`), so this is
    /// consulted only on mode switches, never per frame.
    fn contributes_panels(&self) -> Vec<ModePanelDescriptor> {
        Vec::new()
    }

    /// Build the GPUI view for one panel this mode contributes.
    ///
    /// This is the deliberate exception to the "modes are GPUI-agnostic"
    /// rule from §4.1: the core trait stays pure (identity, `layout()`,
    /// `toolbar_controls()`, `on_pointer`), but a mode that opts into real
    /// panels implements this to construct arbitrary `ui::dock::PanelView`s.
    /// It receives the shared editor state so a panel can frame-pump the same
    /// `Arc<RwLock<LevelEditorState>>` every other panel does.
    ///
    /// Return `None` for descriptors this mode doesn't recognize (the shell
    /// silently skips them). Only called for ids returned by
    /// [`Self::contributes_panels`], and only while the mode is active.
    fn build_panel(
        &self,
        _state: Arc<parking_lot::RwLock<LevelEditorState>>,
        _panel: &ModePanelDescriptor,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Box<dyn ui::dock::PanelView>> {
        None
    }

    /// Handle pointer events occurring within the viewport.
    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult;

    /// Helper for boxing clones in the trait registry.
    fn clone_box(&self) -> Box<dyn ToolMode>;
}
