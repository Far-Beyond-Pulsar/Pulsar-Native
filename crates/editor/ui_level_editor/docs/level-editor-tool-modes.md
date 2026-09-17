# Level Editor — Tool Modes, Voxel Terrain & Foliage (Design)

Status: DRAFT
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
- Scene mutations: `SceneCommand` + `execute_command` (`core/commands.rs`), snapshot-based
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

Modes are **data-in/data-out strategies**. They never build GPUI elements directly and never
hold `Context<V>`; the GPUI shell owns rendering. This keeps the trait object-safe and lets
future modes (including plugin modes) stay UI-agnostic.

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
2. **Voxel seam** — `TerrainEditApi` in `engine_backend`; thread the active planet's
   `TerrainRuntimeHandle` in. Planet hit-test + sculpt (raise/lower/flatten/paint) with
   brush cursor; per-stroke undo via terrain snapshots.
3. **Flat voxel terrain** — `TerrainShape` axis / `EditShape::Box` + volume creation,
   or (if (b) chosen) `pulsar_flat_terrain`; editor gains "create flat world" in Terrain
   mode; same brush pipeline.
4. **Foliage painting** — `FoliageBrush` stamping foliage components; density/slope
   controls; save-path coordination.
5. **Extensibility demo** — one more mode (e.g. Decals or a test "Spline") via
   `registry.register(...)` to prove the contract, then document it as the template.
   Plugin/workspace-external contribution point can follow later.

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
- **Registration surface for future external modes:** defer until the milestone-5 demo
   proves the contract; likely a `fn register_tool_modes(registry)` hook at editor boot.