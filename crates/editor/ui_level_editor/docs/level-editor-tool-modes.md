# Level Editor — Tool Modes, Voxel Terrain & Foliage (Design)

Status: IMPLEMENTED (Milestones 1-7 done, tracked by epic #709). The
extensibility contract this doc describes (§4.2) is proven out by Milestone 5
(issue #714); see [`adding-a-tool-mode.md`](./adding-a-tool-mode.md) for the
practical "how to add a mode" walkthrough, including two real gaps this doc's
original sketch didn't anticipate (§9 below, and the `ToolModeDispatcher`
limitation documented in that file's §5). Milestone 6 (§10) extends the
extensibility contract from toolbar widgets to whole dock panels: a mode can
now hide the right dock or claim a left-hand tools panel of its own instead of
crowding every control into the horizontal toolbar strip. Milestone 7 (§11)
extends it one level further — from *declared* panels the shell renders to
*real GPUI panels a mode builds itself*, with the mode receiving the full
shared editor state.
Scope: `crates/editor/ui_level_editor` (+ seams into `engine_backend`, `pulsar_terrain`,
`helio-component` foliage stack, and a new flat voxel terrain runtime).

## 1. Goals

1. Give the level editor distinct, user-switchable **tool modes** via a dropdown in the
   top-left of the toolbar:
   - **Level Edit** — the default: select/move/rotate/scale meshes, lights, etc. (current behavior).
   - **Terrain** — voxel terrain creation & sculpting (raise / lower / flatten / paint)
     plus **foliage painting**.
2. Keep the codebase clean: one mode per focused module, shared infrastructure reused,
   no "one big match" sprawl, and no new single-struct mega-state.
3. Make the system **modularly extendable**: future modes (spline, pathing, partition,
   zone paint, …) plug in without touching core editor code — a **trait-object registry**.
4. Terrain is **voxel-based** throughout. Planetary voxels already exist (`pulsar_terrain`);
   flat landscapes must also be voxels (not heightmaps).

## 2. Orthogonal axes (do not conflate)

| Axis | Type | Owner | Values |
|---|---|---|---|
| Runtime state | `EditorMode` | `SceneDomain` (`state/scene.rs:25`) | `Edit`, `Play` |
| Authoring context | `ToolMode` (**new**) | `EditorDomain` / `ToolModeDomain` | `LevelEdit`, `Terrain`, … |

- `EditorMode` is play/stop. `ToolMode` is which editing toolkit is active.
- Switching tool mode while in `Play` is ignored (or allowed but rendered inert).
- `TransformTool` (`Select/Move/Rotate/Scale`) remains the *sub-tool* of **LevelEdit** —
  mode-scoped, not editor-global.

## 3. Current architecture (grounding)

- `LevelEditorState` = `Arc<parking_lot::RwLock<...>>` of domains (`state/mod.rs:54`):
  `scene`, `editor`, `overlays`, `hierarchy`, `build`, `play`.
- `EditorDomain` holds `current_tool: TransformTool`, camera mode, rendering toggles
  (`state/editor.rs:131`).
- Toolbar: `ToolbarPanel::render` builds an `h_flex` of component structs
  (`ui/toolbar/mod.rs`), cached by `ToolbarView` via `ToolbarSignature`
  (`ui/toolbar/view.rs:41`). **Every toolbar state read must be added to `ToolbarSignature`**
  or the cached view renders stale (documented at the top of `view.rs`).
- Dropdowns use `Button::new(..).popup_menu(..).menu_with_check(..)` via
  `ui::popup_menu::PopupMenuExt` (`ui/toolbar/time_scale_dropdown.rs`).
- Viewport left-click: mailbox `PendingPointerEvent::LeftClick { norm_x, norm_y }`
  → render thread `handle_left_click` (`helio_renderer/renderer.rs:448`)
  → `SceneInteraction::pick` / `try_start_drag` / `update_drag` (`interaction.rs:69/92/135`).
  Gizmo mode is mapped `TransformTool -> (SceneGizmoType, GizmoMode)` in
  `ui/panel.rs:601..625`, pushed via `queue_gizmo_mode`.
- Scene mutations: `SceneCommand` + `execute_command` (`core/commands/`: `types.rs`,
  `executor.rs`), snapshot-based
  undo (`SceneDomain::undo_stack`, `state/scene.rs`).
- Terrain: `pulsar_terrain` = authoritative **spherical planetary voxels**.
  `EditOp { shape: EditShape::Sphere { center_cell, radius_cells }, mode:
  EditMode::{Union,Subtract,Replace,Paint}, material }` (`pulsar_terrain/src/edit.rs`).
  Runtime reached through `TerrainRuntimeHandle` (stored on `World` as
  `PlanetTerrainComponent`, `helio-component/.../planet_terrain_component`).
- Foliage: GPU grass stack `helio-pass-foliage-place` / `-gbuffer`; placement is driven by
  SceneDB foliage entity components (`FoliageTypeComponent`, `FoliageLayerComponent`).

## 4. Tool-Mode system

### 4.1 The `ToolMode` trait (the registry contract)

Modes are **data-in/data-out strategies**. None of the *core* trait methods
(`identity`, `layout()`, `toolbar_controls()`, `panel_tabs()`, `on_pointer`)
ever build GPUI elements directly or hold `Context<V>`; the GPUI shell owns
rendering. This keeps the core contract object-safe and lets future modes
(including plugin modes) stay UI-agnostic. The one deliberate exception is
`build_panel` (§11): a mode that opts into real dock panels builds arbitrary
`ui::dock::PanelView`s there and receives the shared editor state — full GPUI
behind an opt-in method, so `LevelEditMode`/`SplineMode` never touch it.

```rust
// level_editor/tool_modes/mod.rs
pub trait ToolMode: Send + Sync {
    fn id(&self) -> ToolModeId;

    // ── Identity / presentational ──────────────────────────────────────
    fn label_key(&self) -> &'static str;      // i18n key, e.g. "LevelEditor.ToolMode.Terrain"
    fn icon(&self) -> ui::IconName;           // dropdown + mode indicator
    fn description_key(&self) -> &'static str; // tooltip

    // ── Per-frame (GPUI shell calls, mode mutates ctx hooks) ──────────
    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext);
    fn on_mode_exited(&mut self, ctx: &mut ToolModeContext);

    /// Called once per viewport frame; return the brush/hover ring to draw.
    fn brush_cursor(&self, ctx: &ToolModeContext) -> Option<BrushCursor> { None }

    /// Data the shell renders as mode-specific toolbar widgets (sliders,
    /// small dropdowns, toggles). Shell knows how to draw each `ToolWidget`.
    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> { Vec::new() }

    /// Status-bar readout (e.g. terrain mode shows brush radius + material).
    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> { None }

    // ── Pointer routing ────────────────────────────────────────────────
    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult;
}
```

```rust
/// Everything a mode needs to do its job, handed over the wire each call.
pub struct ToolModeContext<'a> {
    pub state: &'a mut LevelEditorState,       // caller holds the write lock
    pub gpu_engine: &'a Mutex<GpuRenderer>,
    pub terrain: Option<&'a TerrainEditApi>,   // voxel seam (see §5.4)
    pub camera: CameraFrame,                   // pos, yaw, pitch, fov, basis
    pub viewport: ViewportFrame,               // bounds, size for ray math
}

/// Normalized viewport coords + button/modifier state (mode-agnostic).
pub struct ToolPointerEvent {
    pub kind: PointerKind,            // Down/Up/Drag/Hover/Scroll
    pub button: Option<MouseButton>,
    pub norm_x: f32, pub norm_y: f32,
    pub holding_mods: Modifiers,
}

pub enum ToolPointerResult {
    /// Mode consumed the event (e.g. painted terrain). Shell does nothing else.
    Consumed,
    /// Fall through to default scene interaction (pick + gizmo + selection).
    PassThrough,
}

pub struct BrushCursor { pub center: Vec3, pub radius: f32, pub color: [f32; 4] }

/// One declarative toolbar widget a mode wants the shell to draw.
pub enum ToolWidget {
    Slider { id: &'static str, label_key: &'static str, value: f32, min: f32, max: f32, step: f32 },
    Segmented { id: &'static str, options: Vec<(&'static str, &'static str)> /* label_key, value_key */ },
    Toggle   { id: &'static str, label_key: &'static str, on: bool },
    Divider,
}
```

The shell writes widget changes back through explicit mode setters via a small
`ToolModeEdit` enum dispatched by `ToolModeDispatcher` (single switch point, §4.3).

### 4.2 The registry

```rust
// level_editor/tool_modes/registry.rs
pub struct ToolModeRegistry {
    modes: Vec<Box<dyn ToolMode>>,
    selected: ToolModeId,
}

impl ToolModeRegistry {
    pub fn builtin() -> Self;                 // LevelEdit, Terrain
    pub fn register(&mut self, mode: Box<dyn ToolMode>);       // extensibility seam
    pub fn selected(&self) -> &dyn ToolMode;
    pub fn select(&mut self, id: ToolModeId, ctx: &mut ToolModeContext);
}
```

- `ToolModeId` is a private `String`/`&'static str` wrapper (compare by `id()`), so new
  modes need no enum change. Builtin ids: `"level_edit"`, `"terrain"`.
- `ToolModeRegistry` lives **inside** `EditorDomain` (`state/editor.rs`) so selection
  persists naturally and `ToolbarSignature` stays derivable from it.
- Registration point is `LevelEditorState::new()` (builtins) — future external modes
  register at editor construction.

### 4.3 State shape (`LevelEditorState`)

```rust
pub struct EditorDomain {
    // ...existing...
    pub tool_mode_registry: ToolModeRegistry,  // owns selected mode + all modes
    pub terrain: TerrainDomain,                // brush settings (§5), not the terrain data
}
```

Per-mode sub-state (brush settings, active planet, etc.) lives in one domain struct
(`TerrainDomain`) so modes remain stateless apart from their config. Terrain *data*
never lives in editor state — it stays in the voxel runtimes (see §5).

### 4.4 Toolbar integration

- **Dropdown (top-left).** `ui/toolbar/tool_mode_dropdown.rs` becomes the first child in
  `ToolbarPanel::render` before `PlaybackControls`. Uses `popup_menu` + `menu_with_check`
  per registered mode (icon + label). Selecting a mode: write lock state →
  `registry.select(id)` → (if `ToolModeContext` side effects needed, defer them to a
  queued `ToolModeEdit`).
- **`ToolbarSignature`** (the one-slot constraint): add `tool_mode: ToolModeId` and the
  `TerrainDomain` fields surfaced by `toolbar_controls`. `ToolbarSignature::of` must read
  them, and the scrubber in `ToolbarView` must bump a control value whenever `toolbar_controls`
  change. Document that adding a widget ⇒ update the signature (extend the existing
  "If you add a state read…" note in `view.rs`).
- **`ModeIndicator`** (`ui/toolbar/mode_indicator.rs`): keep the Play/Edit pill, and add the
  tool-mode icon + label beside it (or fold: `Playing · Terrain` / `Editing · Level`).
- **i18n**: new `locales/*.toml` keys `LevelEditor.ToolMode.{LevelEdit,Terrain}` + widget
  label/description keys.

### 4.5 Viewport input routing

Single dispatch point in `ui/viewport/mod.rs` (the `on_mouse_down(Left)` handler currently
at `viewport/mod.rs:925`):

```
left button event
  ├─ right/middle captured (camera)   → existing camera path (unchanged)
  ├─ overlay drag active              → existing overlay path (unchanged)
  └─ otherwise                        → dispatch_active_mode_pointer(Event)
       │
       │ state.editor.tool_mode_registry.selected().on_pointer(&event, &mut ctx)
       ├─ Consumed   → done (no mailbox push)
       └─ PassThrough → existing mailbox `PendingPointerEvent::LeftClick/LeftRelease`
                        path → renderer pick + gizmo (LevelEdit behavior, byte-for-byte)
```

- Modes that need the renderer (brush ring, planet hit test, edit ops) get `ctx.terrain`
  (the `TerrainEditApi`) and `ctx.gpu_engine`; all math is done from
  `CameraFrame` + `ViewportFrame` constructed by the shell around `engine.editor_camera_state()`.
- Hover during `Consumed`-class modes still routes to `handle_mouse_move` for cursor-first
  feedback; modes can opt into raw deltas via `event.kind == Drag` when they need an actual
  drag carriage return.

## 5. Terrain authoring

### 5.1 Voxel recap (planetary, exists)

`pulsar_terrain`: canonical planet-cell addressing, `EditOp`/`EditShape` mutation log,
streaming LOD pages, persistent snapshots (`TerrainStore`). Editing a planet =
pushing `EditOp`s with cells in planet space. Brush math for a planet hit: ray → sphere
intersection → `PlanetPosition::from_lod0_cell` (planetary radius from `PlanetDefinition`).

### 5.2 Flat voxel terrain (new — voxel based)

Add a flat variant on the **same voxel model** (density field, `CellWord`, material,
PageKey-style paging) so editing math, brushes, materials, and persistence are shared —
never a disjoint heightmap system.

Recommended shape: a **finite voxel volume**, e.g. `FlatTerrain { cell_size_m, extent:
(i16,i16,i16), origin }`, exposed through the same `TerrainEditApi` so `EditShape`-style
ops (sphere/slab/box brushes) work on both. Two options to weigh:

- **(a) Reuse `pulsar_terrain` runtime with a rectangular "planet".** Planet cells are
  axis-aligned page bricks over an extent; an `EditShape::Box` maps a flat-world volume
  into cell space. Largest reuse of hierarchy/LOD/persistence; a cube "planet" is also a
  natural boundary for testing. Cost: some "sphere"-specific code paths need generalization
  (cell→sphere direction math; creator/streamer).
- **(b) A sibling `pulsar_flat_terrain` crate** mirroring the minimal core
  (edit log → pages → GPU page upload → snapshot). Cleaner separation from planet math,
  at the cost of duplicating the voxel core.

**Recommendation: (a)** first — generalize `pulsar_terrain` by adding a `TerrainShape`
axis (`Planet` | `Volume`) to addressing/`EditShape` (add `EditShape::Box`), then revisit
(b) only if planet LOD coupling proves expensive. Either way, the **editor front-end is
identical** because both sit behind `TerrainEditApi`.

### 5.3 Brush model

`TerrainDomain`:

```rust
pub struct TerrainDomain {
    pub sculpt: SculptBrush {
        pub mode: EditMode,          // Raise=Union, Lower=Subtract, Flatten=Replace
        pub radius_m: f32,           // brush radius in meters (~1..64)
        pub falloff: f32,            // 0=hard .. 1=soft
        pub strength: f32,           // per-stamp depth (meters), for Push/Pull feel
        pub material: MaterialId,
    },
    pub foliage: FoliageBrush {
        pub type_id: String,          // FoliageTypeComponent / registered mesh
        pub density: f32,             // blades/m² override for stamped region
        pub radius_m: f32,
        pub slope_limit: (f32, f32),  // degrees band this type grows on
    },
    pub target: TerrainTarget,        // which planet/volume is being edited
    pub active_stroke: Option<Stroke>,// coalesced brush-op batch for undo (started on Down, closed on Up)
}
```

- **Sculpt stamp** (per pointer `Drag`): ray→terrain hit → convert to
  cells (planet or volume) → build `EditShape` sized by `radius_m` + falloff → one
  `EditOp` (Union/Subtract/Replace) with `material`. Coalesce stamp-to-stamp distance
  (do not emit an op if the brush center moved less than ~radius/4) so drags stay cheap.
- **Flatten** = `Replace` ops anchored to the height sampled at stroke start (stroke-height
  capture, standard sculpt behavior).
- **Paint** = `EditMode::Paint` ops carrying `material` (existing voxel material channel).
- **Brush cursor** = mode returns `BrushCursor`; the shell draws a debug ring
  (renderer already has `debug_line`/`debug_torus` via `SceneInteraction::draw_gizmo`).

### 5.4 Terrain edit seam (the integration gap)

The editor does not currently hold a `TerrainRuntimeHandle`. Add a narrow
`engine_backend` service (or thread a handle from `PlanetTerrainComponent` into
`SceneDomain`) exposing:

```rust
// engine_backend::services::terrain_edit  (new)
pub struct TerrainEditApi {
    active: Option<(PlanetId | VolumeId, TerrainRuntimeHandle)>,
}
impl TerrainEditApi {
    pub fn hit_terrain(&self, ray: Ray3) -> Option<TerrainHit>; // planet or volume, cell, normal, material
    pub fn apply_edit(&self, target: TerrainTarget, op: EditOp) -> Result<(), TerrainEditError>;
    pub fn create_planet(&self, def: PlanetDefinition) -> Result<PlanetId, _>;
    pub fn create_volume(&self, def: VolumeDefinition) -> Result<VolumeId, _>;
}
```

- Pointer handlers (mode + viewport) go through this — never through raw runtime handles.
- Render-thread safety follows the existing mailbox pattern for anything cross-thread.

### 5.5 Undo & persistence

- **Scene objects** (foliage stamps) already flow through `SceneCommand`/`execute_command`
  (snapshot undo) — free.
- **Voxel edits** cannot use `SceneHistorySnapshot` (it snapshots the scene DB, not the
  voxel store). Design: mirror the command checkpoint pattern with a **terrain grant
  checkpoint**:
  - `Stroke` = one undo unit (Down…Up). On stroke-start, capture
    `TerrainSnapshot` (terrain already can snapshot); on stroke-end inside
    `execute_command`-shaped flow, commit a compacted delta to a stacked
    `TerrainUndoDomain` (bounded capacity like `MAX_UNDO_HISTORY`), and pair it with the
    scene checkpoint when the same action also touches scene objects.
  - Reuse `TerrainStore` snapshot encoding; keep editor undo bounded.
- Persistence stays where it is: `TerrainStore` for voxel data, `.level` for objects.
  Save path must now also flush pending terrain snapshots (`save_to_file_with_editor_camera`
  currently only writes scene objects — extend to coordinate with `TerrainEditApi`).

## 6. Foliage painting

Foliage rendering already keys off SceneDB foliage entities (types/layers). Painting =
**stamping authoring data**, not per-blade placement:

- `FoliageBrush` paints a `FoliageTypeComponent` + `FoliageLayerComponent` region into the
  scene at the terrain hit (scene `AddObject` with a foliage component set) via
  `SceneCommand` → undo-tracked automatically.
- Density/slope limits set in the brush flow into the stamped component; the GPU passes
  keep deciding which blades actually exist per 8 m tile.
- Terrain mode owns brush and target; no separate "foliage mode" needed initially (it is a
  `FoliageBrush` sub-tab of Terrain). If it grows (multi-type palettes, masks), it becomes
  its own `ToolMode` via the registry — that is the extensibility path we are reserving.

## 7. File layout (target)

```
crates/editor/ui_level_editor/src/level_editor/
  state/
    editor.rs        + tool_mode_registry: ToolModeRegistry, terrain: TerrainDomain
    terrain.rs       TerrainDomain, SculptBrush, FoliageBrush, Stroke
  tool_modes/                      ← new
    mod.rs           ToolMode trait, ToolModeContext, ToolPointerEvent,
                     ToolPointerResult, BrushCursor, ToolWidget, StatusReadout
    registry.rs      ToolModeRegistry, ToolModeId
    dispatcher.rs    ToolModeDispatcher — the ONE switch point on_pointer/toolbar/draw,
                     write-back of ToolWidget edits (mode-scoped works stay clean)
    level_edit.rs    LevelEditMode  (exactly today's pick+gizmo behavior)
    terrain/mod.rs   TerrainMode
    terrain/sculpt.rs brush math → EditOp/EditShape (planet + volume)
    terrain/foliage.rs stamp → SceneCommand FoliageType/Layer objects
    terrain/ray.rs     camera/viewport → terrain hit (planet & volume)
  ui/toolbar/
    tool_mode_dropdown.rs          ← first toolbar child
    mode_indicator.rs              + tool-mode icon/label
  ui/viewport/mod.rs               dispatch_active_mode_pointer(...)
  ui/status_bar_view.rs            + mode status readout
engine_backend/src/services/terrain_edit.rs      TerrainEditApi (new seam)
pulsar_terrain/src/…               TerrainShape axis + EditShape::Box (flat-voxel option (a))
```

## 8. Milestones

1. **Shell & registry** — ✅ **Done.** `ToolMode` trait, `ToolModeRegistry`, `ToolModeId`,
   `TerrainDomain` + stub state, top-left dropdown, `ToolbarSignature` + `ModeIndicator`
   updates, status bar readout. `LevelEdit` is registered and is byte-for-byte today's
   behavior; a placeholder `Terrain` mode exists with brush UI but `PassThrough` no-ops.
   Viewport pointer dispatch (§4.5) is wired at the `on_mouse_down(Left)` handler in
   `ui/viewport/mod.rs`, giving the active mode first refusal before the standard pick/gizmo
   mailbox path; `LevelEditMode::on_pointer` always returns `PassThrough` so behavior is
   unchanged. i18n keys added for all 16 locales. No behavior change (ship-safe).
2. **Voxel seam** — ✅ **Done** (issue #711). `TerrainEditApi` in `engine_backend`
   (`services/terrain_edit.rs`); a narrow scene-authored activation bridge
   (`sync_scene_planets`) re-supplies the planet runtime the in-flight SceneDB
   nativization work disconnected. Planet hit-test + sculpt (raise/lower/flatten/paint)
   with brush cursor; per-stroke undo via `TerrainUndoDomain`; terrain persists to a
   `.terrain` sidecar.
3. **Flat voxel terrain** — ✅ **Done** (issue #712). Approach (a): a `TerrainShape` axis
   (`Planet` | `Volume`) and `EditShape::Box` in `pulsar_terrain` (`body.rs`'s
   `TerrainBodyDefinition`), so flat `VolumeDefinition`/`FlatTerrain` worlds share the
   exact sculpt/undo/sidecar pipeline planets use — no shape branching above
   `pulsar_terrain`/`TerrainEditApi`. "Create Flat World" toolbar action in Terrain mode.
4. **Foliage painting** — ✅ **Done** (issue #713). `FoliageBrush` (scaffolded since
   Milestone 1) wired to a real paint stamp: a "Paint Foliage" toggle sub-mode of Terrain
   stamps `FoliageComponent`-carrying scene objects (not the doc's original
   `FoliageTypeComponent`/`FoliageLayerComponent` sketch — those are GPU-internal, derived
   automatically from one `FoliageComponent`) via the ordinary `SceneCommand`/scene-undo
   path, kept independent of `TerrainUndoDomain`.
5. **Extensibility demo** — ✅ **Done** (issue #714). Added `SplineMode`
   (`tool_modes/spline.rs`): click to place points in the viewport, Shift-click to
   clear, point count + total length surfaced on the toolbar and status bar. Registered
   purely via a new `register_tool_modes(&mut ToolModeRegistry)` hook
   (`tool_modes/registry.rs`), called from `EditorDomain::default()` right after
   `ToolModeRegistry::builtin()` — `builtin()`'s own two entries, `ToolModeDispatcher`,
   `ToolModeContext`, and `LevelEditMode`/`TerrainMode`'s logic were not touched. See
   [`adding-a-tool-mode.md`](./adding-a-tool-mode.md) for the worked-example writeup,
   including the state-placement rule (mode-internal field vs. new `EditorDomain`
   sub-struct), the `ToolbarSignature`/`StatusBarSignature` staleness gotcha, and a real
   limitation this milestone surfaced: `ToolWidgetEdit` write-back
   (`ToolModeDispatcher::dispatch_widget_edit[_with_terrain]`) is a hardcoded switch over
   `TerrainDomain`-specific widget ids, not actually generic over `ToolMode`
   implementors as §4.1 implies — a mode's interactive toolbar widgets need a (small,
   additive) dispatcher edit to do anything; `SplineMode` avoids this entirely by doing
   all of its real work through the already-generic `on_pointer`. A true out-of-tree
   plugin/workspace-external contribution point is still open (§9).

## 9. Open questions

- **Flat terrain scope:** finite-volume `(i16,i16,i16)` cell world is the recommendation;
   confirm extent budget (GB of voxels needed before LOD streaming kicks in) before
   building, so `EditShape::Box` correctness is scoped.
- **Planet/volume interop:** can a level contain both, and can foliage paint onto both?
   (Design assumes yes, both behind `TerrainEditApi`.)
- **Undo pairing:** should one stroke atomically bundle voxel + foliage + object edits?
   (Recommendation: yes — a stroke is the undo unit.)
- **`ToolModeContext` locking:** the shell holds the state write lock during
   `on_pointer`. Confirm no mode needs to re-enter that lock (design says no; enforce by
   passing `state` directly).
- **Registration surface for future external modes:** ✅ **Resolved by Milestone 5**
   (issue #714) for in-tree modes: `fn register_tool_modes(registry: &mut ToolModeRegistry)`
   in `tool_modes/registry.rs`, called from `EditorDomain::default()` right after
   `ToolModeRegistry::builtin()`. What remains genuinely open is a true *out-of-tree*
   (plugin/workspace-external, i.e. a different crate or a dynamically loaded module)
   contribution point — `register_tool_modes` today is a plain function call compiled
   into this crate, not a discovery mechanism. Also newly surfaced by Milestone 5 (not
   previously called out in this doc): `ToolWidgetEdit` write-back
   (`ToolModeDispatcher::dispatch_widget_edit[_with_terrain]`) is hardcoded per widget id
   rather than generic over `ToolMode` implementors, so a registered mode's interactive
   toolbar controls (`Toggle`/`Action`/editable `Slider`/`Segmented`) do not work out of
   the box the way `on_pointer` does — see `adding-a-tool-mode.md` §5.
6. **Mode-defined panels & layout** — ✅ **Done.** `ModeLayout` added to the `ToolMode`
   trait (`show_right_dock`, `show_mode_panel`); `TerrainMode` opts into its own left-hand
   `ModeToolsPanel`, decluttering the toolbar of its now-considerable sculpt+foliage
   control count. `LevelEditorPanel::sync_mode_layout` reconciles the dock area on mode
   switch, mirroring `sync_game_tab`'s guarded-signature shape. See §10.
7. **Mode-owned GPUI dock panels** — ✅ **Done.** The bundled Milestone 7 hardening of
   the mode-panel system: `ModePanelDescriptor`/`ModePanelPlacement` plus the
   `contributes_panels()`/`build_panel()` trait pair let a mode build *real* dock
   panels (full GPUI + the shared state Arc), the deliberate exception to §4.1.
   Terrain's brush palette (`tool_modes/terrain/{layout,panels}.rs`) is the worked
   example; `sync_mode_layout` folds left contributions into the mode-tools tab strip
   and right contributions into the Properties/World tab group (rebuilt only when the
   contributed set changes, §11.3). See §11.

## 10. Mode-defined panels & layout (Milestone 6)

> **Superseded in part.** The generic `ModeToolsPanel` ("Tools" tab), `panel_tabs()` and
> `PanelTab`, `ModeLayout::show_mode_panel`, `WidgetLayout::Panel` and `ToolWidget::Section`
> described in 10.1-10.4 were removed: modes now contribute real panels through
> `contributes_panels()` + `build_panel()` (section 11), and `toolbar_controls()` only feeds
> the horizontal toolbar strip. `ModeLayout` is just `{ show_right_dock }`. The text below is
> kept as history.

**Problem this addendum solves**: §4.1's `toolbar_controls()` puts every mode-specific
control into one horizontal strip. That's fine for a handful of widgets (Level Edit has
none, Spline has two read-only chips) but not for Terrain, whose sculpt *and* foliage
controls together would otherwise crowd the toolbar. The fix generalizes the same idea
one level up: a mode can declare a whole **panel layout**, not just a widget list.

### 10.1 `ModeLayout` (the new part of the registry contract)

```rust
// level_editor/tool_modes/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeLayout {
    /// Show the right-hand dock (Hierarchy / Properties / World Settings)?
    pub show_right_dock: bool,
    /// Render this mode's `toolbar_controls()` in a dedicated left-hand dock
    /// panel instead of the horizontal toolbar strip?
    pub show_mode_panel: bool,
}
```

Added to the `ToolMode` trait as `fn layout(&self) -> ModeLayout`, defaulting to
`{ show_right_dock: true, show_mode_panel: false }` — today's behavior for every mode
that doesn't override it (`LevelEditMode`, `SplineMode`). `TerrainMode` overrides it to
`{ show_right_dock: true, show_mode_panel: true }`: picking objects and checking World
Settings while sculpting is still a normal workflow, so the right dock stays, but the
sculpt/foliage controls move to their own panel.

Pure data, same contract as `ToolWidget`/`StatusReadout` — a mode never touches a dock
panel entity directly, only describes what it wants.

### 10.2 Rendering: one widget-to-element mapping, two layouts

`ui/mode_widgets.rs` (new, promoted out of `ui/toolbar/` since it is no longer
toolbar-exclusive) holds the single function that turns a `Vec<ToolWidget>` into GPUI
elements, parameterized by `WidgetLayout::{Toolbar, Panel}` (horizontal row vs. vertical
column — same widget variants, same dispatcher calls, different flex axis and divider
orientation). Both call sites — the toolbar (`ui/toolbar/mod.rs`) and the new dock panel
(`workspace/panels/mode_tools.rs`) — read the *same* `toolbar_controls()` data through
this one function, so a mode's controls can never drift between the two renderings. The
toolbar skips rendering a mode's widgets entirely when `layout().show_mode_panel` is
true, so they never appear in both places at once.

### 10.3 `ModeToolsPanel`: one generic left-hand panel, not one per mode

`workspace/panels/mode_tools.rs`'s `ModeToolsPanel` is a single `ui::dock::Panel` that
renders *whichever* mode is currently active's widgets — it never names `TerrainMode` or
any other concrete mode. It is a self-refreshing panel like `HierarchyPanelWrapper`/
`PropertiesPanelWrapper` (a frame pump comparing `(ToolModeId, Vec<ToolWidget>)` each
tick), so a mode that changes its own widget values (e.g. dragging the brush radius) or
switches mode entirely both invalidate it correctly with no coupling to any specific
mode's state shape.

### 10.4 Dock reconciliation: `LevelEditorPanel::sync_mode_layout`

`ui/panel.rs` already had exactly this shape of problem once — `sync_game_tab` opens/
closes the Play-In-Editor tab based on state, guarded by a cheap signature check so the
several-times-a-second render loop doesn't touch the dock area for nothing.
`sync_mode_layout` follows the identical pattern, guarded on the active `ToolModeId`:

- Right dock: `dock_area.toggle_dock(DockPlacement::Right, ..)` when its current
  open/closed state disagrees with `layout.show_right_dock`. (§11) additionally
  rebuilds the right dock *only* when the set of mode-contributed right panels
  changes — tracked in `LevelEditorPanel::mode_right_panels` — so the common
  case (no contributions) leaves the Properties/World Settings tab group and
  its cached section entities untouched.
- Left "mode tools" dock: rebuilt wholesale on a mode switch via `set_left_dock`,
  with each `PanelTab` the active mode returns becoming its own real dock panel
  (`ModeToolsPanel`) in one native tab strip — and any panel the mode
  contributes to the left dock (§11) joining that same strip. That is a full
  `set_left_dock` every time a mode with `show_mode_panel: true` becomes
  active, which is fine precisely because it only runs on a mode switch (the
  expensive rebuild `PropertiesPanelWrapper`'s caching exists to avoid does not
  apply to a fresh, throwaway hot-swap panel).

### 10.5 File layout (target, extends §7)

```
ui/
  mode_widgets.rs                  ← NEW: shared ToolWidget → element rendering
  toolbar/
    mod.rs                         (renders mode widgets via mode_widgets, Toolbar layout)
    tool_mode_dropdown.rs
    build/                         ← build family: split-button + target dropdowns + cargo progress
      build_core/                  ← split-button core (split per concern)
        mod.rs
        quick.rs
        pipeline.rs
        crash.rs
      build_dropdowns/             ← config + 290-target platform dropdowns
        mod.rs
        platform.rs                ← label/icon tables
        platform_options.rs        ← platform_menu (all target submenus)
        config.rs                  ← build-config button
        render.rs                  ← h_flex assembly
      cargo_progress.rs
  world_settings/
    panel/                         ← WorldSettingsPanelImpl (renders under the dock wrapper)
      mod.rs
      sections.rs
      fields.rs
workspace/
  panels/                          ← was a single panels.rs, split one file per panel
    mod.rs
    hierarchy.rs
    properties.rs
    viewport.rs
    world_settings.rs
    mode_tools.rs                  ← NEW: ModeToolsPanel (Panel layout)
ui/panel.rs                        + sync_mode_layout, mode_tools_panel/applied_mode_layout fields
```

## 11. Mode-owned GPUI dock panels (Milestone 7)

**Problem this addendum solves**: §10 gave a mode a *declared* panel whose
content is still restricted to `toolbar_controls()`-shaped widgets
(`ToolWidget`). That covers sliders, segmented pickers, toggles — but not a
real palette UI with its own layout, gaps, and headers. The line between "a
mode is UI-agnostic data" (§4.1) and "the shell owns all rendering" breaks
the moment a mode wants a panel that is not a flat widget list. §11 draws
the line again, one step further out: **a mode may build real GPUI dock
panels, as an explicit opt-in, receiving the shared editor state.**

### 11.1 The extension point

Two trait methods, both defaulted to inert (`tool_modes/mod.rs`):

```rust
pub enum ModePanelPlacement { Left, Right }

pub struct ModePanelDescriptor {
    pub id: &'static str,        // stable identity; build_panel matches on it
    pub title_key: &'static str, // i18n key for the dock tab title
    pub icon: Option<ui::IconName>,
    pub placement: ModePanelPlacement,
}

fn contributes_panels(&self) -> Vec<ModePanelDescriptor> { Vec::new() }

fn build_panel(
    &self,
    state: Arc<parking_lot::RwLock<LevelEditorState>>, // full editor state, same Arc
                                                        // every other panel holds
    panel: &ModePanelDescriptor,
    window: &mut Window,
    cx: &mut App,
) -> Option<Box<dyn ui::dock::PanelView>> { None }
```

- `contributes_panels` is the *declarative half*: what panels exist and where
  they dock. `build_panel` is the *constructive half*: the actual GPUI view.
  Keeping the two apart means the shell can reconcile the dock area from pure
  descriptors (a `Vec` of `&str` ids + placements) without touching gpui, and
  only calls `build_panel` for the active mode, on mode switch.
- `build_panel` is the deliberate §4.1 exception: it is the one place a mode
  builds GPUI. It gets the entire `LevelEditorState` (not a thin context) so
  the panel can frame-pump the same `Arc<RwLock<...>>` every other editor
  panel does and stay honest about arbitrary state writes. Modes that never
  override it (`LevelEditMode`, `SplineMode`, and any plugin mode that wants
  to stay pure) inherit no gpui dependency — the exception is opt-in.
- Because `ui::IconName` is `Clone + Debug` but *not* `PartialEq`, the
  descriptor deliberately keeps the icon out of any signature struct the
  shell compares; `Option<IconName>` lives on the descriptor only.

### 11.2 Two files per panel-having mode

The worked example `TerrainMode` is a real mode (688-line `tool_modes/terrain/mod.rs`
at Milestone 6), so it keeps its own concerns out of the mode file entirely:

```
tool_modes/terrain/
  mod.rs        controller: on_pointer, layout(), panel_tabs(), status() — no GPUI
  layout.rs     NEW: published manifest — panel ids + contributed_panels()
  panels.rs     NEW: the GPUI views (TerrainPalettePanel) — no state logic
```

`layout.rs` reads like a manifest (stable id, title key, icon, placement);
`panels.rs` is where full GPUI lives (segmented pickers, stepper sliders,
headers, a frame pump). `TerrainMode::contributes_panels` returns
`layout::contributed_panels()` and `TerrainMode::build_panel` matches on
`layout::TERRAIN_PALETTE` then constructs `panels::TerrainPalettePanel` via
`cx.new(...)`, boxing the `Entity<T>` as `Box<dyn ui::dock::PanelView>`. The
palette panel follows the crate's ordinary panel conventions: `panel_boilerplate!`
for `Focusable`+`EventEmitter<PanelEvent>`, an explicit
`impl EventEmitter<ui::dock::PanelEvent>`, a localized `title()`, a frame
pump (`start_pump`) diffing a small `PaletteSignature` copied out of the
domain so it only `cx.notify()`s when something it paints actually changed —
identical in spirit to `HierarchyPanelWrapper`/`PropertiesPanelWrapper`.

### 11.3 Dock reconciliation

`LevelEditorPanel::sync_mode_layout` (guarded on the active `ToolModeId`,
same shape as `sync_game_tab`) now:

- clones the selected mode via `clone_box()` so the shared-state read lock is
  dropped before any gpui construction;
- iterates `contributes_panels()`, calling `build_panel` for each; `&mut
  Context<Self>` satisfies `build_panel`'s `&mut App` parameter via `DerefMut`;
  returned views are `Arc::<dyn PanelView>::from(Box<..>)`;
- **Left** contributions join the left dock's native tab strip alongside the
  `ModeToolsPanel` pages (one reused `DockItem::tabs`), which is rebuilt
  wholesale on the switch anyway;
- **Right** contributions are folded into the bottom Properties/World
  Settings tab group (surfaced first, so the mode's own panel activates);
  because the right dock holds `PropertiesPanelWrapper`'s cached sections, it
  is only rebuilt when the contributed right-panel id set changes
  (`mode_right_panels` field), toggling teardown and re-add symmetrically;
- builds nothing for modes that return an empty manifest — the default, so
  `LevelEditMode`/`SplineMode` behavior is byte-for-byte unchanged.

Left panels need no teardown tracking (the left dock is replaced wholesale on
each switch); right panels do, which is what `mode_right_panels: Vec<&'static str>`
on `LevelEditorPanel` is for.

### 11.4 i18n

Every new title/label goes into `locales/en.yml` (the fallback locale; the
crate's `rust_i18n!` config resolves the rest). Terrain's palette added
`LevelEditor.TerrainPalette.Title` and
`LevelEditor.TerrainPalette.NoTarget`; label renderers take dynamic
`&'static str` keys through `t!` exactly like `ui/mode_widgets.rs` already does.
## 12. Terrain panels rebuilt: Sculpt + Foliage Sets

Terrain's control content moved from flat `ToolWidget` lists to two real panels
built on §11 (`tool_modes/terrain/panels/`): `SculptPanel` (Tool → Brush →
Material) and `FoliageSetsPanel` (Brush → Sets → Inspector), sharing
`panels/widgets.rs`. `layout.rs` lists both (`terrain.sculpt`,
`terrain.foliage`, docked left). A small declarative **World** tab remains for
the create-flat-world action, which needs the renderer's terrain seam. Each
panel has an *active tool* switch — both brushes exist at once and exactly one
fires on a viewport click (`TerrainDomain::paint_foliage`).

### 12.1 Foliage Sets model (`state/foliage_sets.rs`)

`FoliageSetLibrary` → `FoliageSet { name, enabled, expanded, members }` →
`FoliageMember { mesh, enabled, placement: MemberPlacement }`. Placement is
per member: density (per 100 m²), scale range, random yaw, align-to-normal,
ground offset. A brush stamp paints **every enabled member of every enabled
set** by that member's own rules (`paintable_members`). Pure editor config:
no scene database, renderer, or GPUI types.

### 12.2 Painting pipeline (three files, one seam each)

- `scatter.rs` — pure, seeded, unit-tested: library + brush radius/density →
  `InstanceSpec`s in the brush's tangent plane (capped at
  `MAX_INSTANCES_PER_STAMP`).
- `author.rs` — the **only** file that touches the scene: projects each
  instance onto terrain via `TerrainEditApi::hit_terrain`, then creates
  content (today: one `StaticMeshComponent` object per instance under a
  `Foliage · <set>` folder, one undo step per stamp). Swapping in
  `pulsar_scenedb` or the renderer's mesh-foliage path changes this file only.
- `TerrainMode::stamp_foliage` glues them with coalescing (`should_stamp`).

### 12.3 Known gaps

- No erase tool yet (needs instance identity in the authoring backend).
- Slope limits are not in the model: `hit_terrain` returns the radial normal,
  not the sculpted surface normal.
- Sets are not persisted with the project yet.
- Legacy grass-brush fields on `FoliageBrush` (density/slope/wind…) are unused
  by the panels; remove with the scene-database migration.

### 12.4 Revision: one Terrain panel, Unreal-style

The separate Sculpt panel was folded into a single `TerrainPanel`
(`panels/terrain.rs`, id `terrain.panel`) with in-panel tabs **Manage**
(terrain actors: create flat world, pick the edit target, inspect), **Sculpt**
(Raise/Lower/Flatten tool grid + brush options) and **Paint** (Paint tool +
brush options + 15-slot material palette). Foliage stays its own panel — it is
a content-placement tool, not a terrain property. Both follow the Unreal
Landscape/Foliage panel vocabulary via `panels/widgets.rs`: an icon-over-label
**tool grid** with the active tool highlighted, collapsible sections, inset
`− value +` boxes, and a search box + set list for foliage. Choosing a tool or
material both selects *and activates* it (`TerrainDomain::activate_*`), so the
old "active tool" switch is gone. Foliage gained an **Erase** tool
(`author::erase_foliage`: only removes objects parented under `Foliage · …`
set folders, never hand-placed content) with its own erase density.

Manage reads the terrain seam through `EditorDomain::terrain_api`, which
`TerrainMode::on_mode_entered` fills from `ToolModeContext::terrain` (panels
built by `build_panel` only receive the shared state).
Material swatches are generated colours: `pulsar_terrain` has material ids but
no authored names or colours yet.
