# Settings catalog rebuild plan

## Goal

Rebuild the built-in `pulsar_settings` catalog around real, user-visible engine
and editor behavior. A setting belongs in the database only when it has all of
these:

1. A user-facing control with a validated default and a documented scope.
2. A runtime read path that applies the value to the behavior it names.
3. A write path that persists the value to the correct global or project store.
4. A clear fallback for missing, stale, or invalid persisted values.

Do not register placeholder/future settings. A setting is not complete merely
because a UI field can edit it or some code reads its key. Defaults must keep
current behavior, and every control path must use the same value and type.

## Scope and stores

- `editor.*` values are user preferences loaded through `GlobalSettings` and
  saved in the user config store.
- `project.*` values are loaded and saved through the active project's settings
  store; switching projects must switch these values.
- Plugin settings use a host-owned settings bridge. A plugin must not create a
  second `ConfigManager` singleton inside its DLL.
- Settings for subsystems without a live behavior path stay out of the catalog.

## Catalog to keep

These 12 entries already have real consumers. Keep them while correcting the
two data-flow problems called out below.

| Owner | Key | Default | Consumer / effect |
|---|---|---:|---|
| `editor.advanced` | `allow_unsafe_process` | `false` | `engine/src/steps/settings.rs`; controls unsafe blueprint process execution |
| `editor.debugger` | `pie_on_script_error` | `continue` | `ui_level_editor/.../game_viewport.rs`; controls pause-on-script-error behavior |
| `editor.radial_menu` | `enabled` | `true` | `ui_core/src/app/radial_menu.rs`; enables the Tab hold menu |
| `editor.radial_menu` | `hold_delay_ms` | `180` | Same; controls the hold threshold |
| `editor.radial_menu` | `items` | built-in action list | Same; parses and displays configured actions |
| `editor.source_control` | `auto_fetch` | `true` | `ui_git_manager`; controls background remote fetches |
| `editor.source_control` | `auto_fetch_interval_minutes` | `5` | Same; schedules fetch interval |
| `project.graphics` | `bloom_enabled` | `true` | `engine_backend/.../editor_postprocess.rs`; controls editor-camera bloom |
| `project.graphics` | `bloom_intensity` | `1.0` | Same; scales bloom intensity |
| `project.streaming` | `texture_stream_pool_mb` | `512` | Renderer and SceneDB bridge; sets residency budget |
| `project.streaming` | `virtual_texturing_enabled` | `false` | Renderer construction; opts into texture streaming |
| `project.streaming` | `virtual_texture_tile_size` | `128` | Renderer construction; sets virtual-texture tile size |

### Fix before expanding

- **Virtual-texture tile size is currently stored as a string** by its dropdown,
  while both runtime readers accept only an integer. The custom value is
  silently ignored and replaced with 128. Make the reader accept/validate the
  dropdown string (and integer legacy values), with one allowed-value set.
- **Unsafe-process fallback is unreachable in normal startup.** Schema
  registration supplies `false`, so the database read succeeds before the
  fallback to legacy `EngineSettings` is considered. Import an explicitly saved
  legacy value into the modern store before removing the legacy field and make
  the modern store the single source of truth.

## Add to the catalog

All defaults below preserve current behavior. Add each schema alongside its
reader/writer in the same implementation slice.

### Editor appearance — `editor.appearance`

| Key | Default / allowed values | Read/write behavior |
|---|---|---|
| `font_size` | `14`; integer 10–24 | Initialize `Theme::font_size`; zoom shortcuts and the appearance menu read and persist changes |
| `radius` | `6`; one of `0`, `4`, `6`, `8` | Initialize `Theme::radius`; appearance menu reads and persists changes |
| `scrollbar_show` | `scrolling`; `scrolling`, `hover`, `always` | Initialize `Theme::scrollbar_show`; appearance menu reads and persists changes |

Existing controls are in `ui_core/src/app/menu_actions.rs` and
`ui_common/src/menu/mod.rs`; startup currently ignores these user selections.
The appearance menu, zoom/reset shortcuts, and settings pane must update one
shared preference value so their displayed selection cannot drift.

### File drawer — `editor.file_manager`

| Key | Default / allowed values | Read/write behavior |
|---|---|---|
| `view_mode` | `grid`; `grid`, `list` | Initialize `FileManagerDrawer`; existing grid/list actions update the preference |
| `sort_by` | `name`; `name`, `modified`, `size`, `type` | Initialize existing `SortBy` behavior; settings pane changes re-sort immediately |
| `sort_order` | `ascending`; `ascending`, `descending` | Initialize existing `SortOrder` behavior; settings pane changes re-sort immediately |
| `show_hidden_files` | `false` | Initialize the drawer and persist the existing hidden-files toggle |

Consumer: `ui_file_manager/src/components/file_list/mod.rs` and
`ui_file_manager/src/utils/state.rs`. Keep folders first regardless of the
selected file sort. Persist user preferences globally, not in the project.

### Level editor viewport — `editor.viewport`

| Key | Default / range | Read/write behavior |
|---|---|---|
| `camera_move_speed` | `10.0`; 1–100 units/s | Initialize `InputState` and `EditorDomain`; camera speed buttons read/update/persist the same value |
| `location_snap` | `1.0`; 0.01–1000 units | Initialize `EditorDomain`; snap selector updates the renderer gizmo and persists |
| `rotation_snap` | `15.0`; 0.1–360 degrees | Same |
| `scale_snap` | `0.1`; 0.01–10 scale units | Same |

Consumers: `ui_level_editor/src/state/editor.rs`,
`ui_level_editor/src/ui/viewport/input_state.rs`,
`ui_level_editor/src/ui/toolbar/snap_controls.rs`, and
`engine_backend/src/subsystems/render/helio_renderer/interaction.rs`. Unify
camera speed clamping: `InputState` currently clamps to 1–100 while
`EditorDomain` permits 0.5–100. Apply loaded values to the input atomics and the
renderer snap atomics, not just to the Settings pane.

### Build feedback — `editor.build_notifications`

| Key | Default / range | Read/write behavior |
|---|---|---|
| `play_success_sound` | `true` | Build success callback decides whether to play the embedded success asset |
| `play_error_sound` | `true` | Build failure callback decides whether to play the embedded error asset |
| `volume` | `1.0`; 0–1 | Applied to the audio sink before playback |

Consumer: `ui_build/src/runner/audio.rs`. Defaults retain the existing success
and failure sounds. Read preferences at playback time so Settings changes do
not require restarting the editor.

### Code editor — `editor.code_editor`

| Key | Default / range | Read/write behavior |
|---|---|---|
| `tab_width` | `4`; integer 1–16 | Apply at every script editor `InputState` construction |
| `hard_tabs` | `false` | Apply at every script editor `InputState` construction |
| `line_numbers` | `true` | Apply at every script editor `InputState` construction |
| `minimap` | `true` | Apply at every script editor `InputState` construction |
| `soft_wrap` | `false` | Apply at every script editor `InputState` construction |

Consumer: `plugins/vendor/code_editor/src/script_editor/text_editor.rs` (three
editor construction paths). Before registering these entries, add a
host-owned settings read/write bridge in `plugin_editor_api` / `plugin_manager`
so the DLL reads the same database as the host. Remove all hard-coded duplicate
defaults from those paths. Do not add code-editor font size here; use the
global appearance font-size preference.

### Renderer device and quality

Add a dedicated **Renderer** section in Settings with **Device**, **Quality**,
and **Effects** pages. Keep machine/device choices in the global editor store;
keep authored image-quality choices in the active project store.

#### Device — `editor.renderer`

These values affect device/surface creation. Show the active backend, adapter,
and ray-query support as status; do not present detected capabilities as
editable settings.

| Key | Default / allowed values | Required consumer and behavior |
|---|---|---|
| `backend_preference` | `auto`; `auto`, plus backends available on this build/platform | Add backend selection to WGPUI `WgpuOptions` and instance creation. `auto` keeps current all-backend behavior. An explicit unavailable backend must report a clear startup error; do not silently claim it was selected. Restart required. |
| `gpu_preference` | `high_performance`; `high_performance`, `low_power`, `auto` | Feed adapter selection and replace the unconditional high-performance environment policy in `engine/src/gpu_policy.rs`. Log the effective adapter. Restart required. |
| `hardware_ray_queries` | `false` | When enabled, request wgpu's experimental `EXPERIMENTAL_RAY_QUERY` only on supporting native adapters, and pass the experimental-feature acknowledgement through WGPUI device creation. If unsupported, leave it disabled and report why. Restart required. |
| `max_frame_latency` | `2`; integer 1–4 | Wire WGPUI's existing `desired_maximum_frame_latency` through editor and game surfaces. Lower values favor input latency; higher values can improve throughput. Apply to newly created surfaces, and report the effective value. |

Ray queries here mean the hardware acceleration feature only. The current
renderer uses it as an optional path in screen-space reflections and radiance
cascades; Pulsar does **not** currently expose a full path-tracing renderer.
The editor's GPUI device does not currently request this experimental feature,
while the standalone game path requests it whenever the adapter reports
support. The new preference must make those paths consistent.

#### Quality and effects — `project.rendering`

These map to fields and builder methods already present in Helio's
`RendererConfig`. Register them only in the same change that applies them to
the editor viewport and game renderer.

| Key | Default / allowed values | Required consumer and behavior |
|---|---|---|
| `render_scale` | `0.75`; 0.25–1.0 | Apply through `with_render_scale` when TSR is off; allow live updates through `Renderer::set_render_scale`. |
| `tsr_quality` | `off`; `off`, `performance`, `balanced`, `quality`, `native` | `off` preserves today's `None` TSR path. Other values map to Helio `TsrQuality` and its recommended internal scale, through renderer construction and `set_tsr_quality`. Disable the manual scale control while a TSR preset owns the scale. Define precedence against the voxel renderer's scene-specific quality before exposing the setting. |
| `shadow_quality` | `medium`; `low`, `medium`, `high`, `ultra` | Map directly to Helio `ShadowQuality`; apply at graph creation or add a supported live setter. |
| `shadow_atlas_size` | `1024`; `512`, `1024`, `2048`, `4096` | Apply to the existing `RendererConfig.shadow_atlas_size`; show a VRAM impact hint because memory use grows with atlas area. |
| `screen_space_reflections` | `false` | Apply through `with_ssr`. This is the actual reflection-pass toggle. |
| `planar_reflections` | `false` | Apply through `with_planar_reflections`; explain that it rerenders the scene for authored reflection planes. |
| `hdr_output_mode` | `ldr`; `ldr`, `hdr10`, `scrgb` | Apply `with_hdr_output_mode` and choose a compatible surface format using Helio's `select_hdr_surface_format`. If the display/surface cannot support a requested mode, keep LDR active and show the reason. Do not offer raw passthrough as a normal-user option. |
| `render_mode` | `deferred`; `deferred`, `forward_opaque`, `forward_only` | Advanced setting mapped to Helio's existing render modes. Validate required feature/pass compatibility and make renderer recreation explicit. |

The effect and graph settings above need a reliable renderer-reconfigure path:
today most are read only while the render graph is constructed. Prefer a
single controlled graph rebuild on project setting changes over adding fields
to the database that do not affect an already-running viewport. TSR and render
scale already have live renderer setters; use them for those two.

`screen_space_reflections` can use hardware ray queries when both the device
preference is enabled and the selected adapter supports them. Add a separate
advanced `ray_query_reflections` project option only if users need to choose
between the current Hi-Z-only SSR path and its hardware-accelerated path; its
fallback must remain SSR and the UI must show when ray queries are unavailable.
Do not label either option “full ray tracing.”

#### Renderer follow-on candidates

Helio also has real options for GI volume radius, environment reflections,
foliage density, portals, perf overlays, and XR. Audit each with its actual
render-graph cost, target (editor viewport vs. game), and update/rebuild
semantics before adding it. In particular, do not expose `environment
reflections` as an ordinary performance switch: the current renderer relies
on it as the base specular contribution, and disabling it makes metallic
materials black. Foliage density and XR belong in specialized workflows, not
the first general Renderer page.

## Explicitly out of this catalog

The removed accessibility, AI, animation, audio-mix, build, gameplay, input,
network, packaging, physics, plugin, project-info, scripting, VR, window, and
world schemas had no live settings-database consumers. Do not restore their
placeholder toggles. Revisit an owner only alongside an implemented system,
its Settings UI, persistence scope, and runtime behavior.

Also leave legacy TOML fields such as `performance_level`, `debug_logging`,
`experimental_features`, `max_viewport_fps`, project autosave/backups, and
legacy editor line-number/wrap fields out until a real behavior uses them.
`active_theme` remains owned by the theme system; do not duplicate theme
selection until that system exposes a selected-theme read/write contract.

## Implementation order

1. Fix the two current-catalog correctness issues (tile-size type and unsafe
   process migration); keep the 12 current entries live.
2. Add appearance preferences and make existing appearance/zoom controls
   persist through them.
3. Add file-drawer preferences and persist both Settings-pane edits and drawer
   controls.
4. Add viewport preferences; reconcile duplicated camera speed state and apply
   persisted snap values to each renderer.
5. Add build-notification sound preferences.
6. Add renderer device/surface options in WGPUI and startup, then project
   rendering options in Helio's graph construction and reconfiguration path.
7. Add the plugin settings bridge, then add the five code-editor preferences.
8. Remove dead fields from the legacy TOML `EngineSettings` struct only after
   their migration or retirement behavior is implemented.

## Completion gate

For every registered key, verify: default is visible; editing it persists to
the correct store; reloading restores it; the named behavior changes
immediately or at the next documented initialization boundary; invalid or
missing values fall back safely; and no duplicate hard-coded reader bypasses
the setting. Search all registered keys for actual consumers before accepting
the final catalog.
