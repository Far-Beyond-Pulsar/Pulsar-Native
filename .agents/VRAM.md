# Video memory: the VRAM tracker and the VRAM Map

Only GPU memory. Process memory (heap, address space, the residency cache) is
`.agents/MEMORY.md` and the Memory Explorer.

## The tracker (`engine_backend::services::vram`)

`vram::snapshot(usage)` returns a `VramSnapshot`, read when a tool asks; nothing
runs per frame.

- **Allocated**: every engine device's allocator report (wgpu): each driver memory
  block and, inside it, each allocation by label, offset and size (`VramDevice`,
  `VramBlock`, `VramAllocation`). Helio, SceneDB, the plugins and the UI all
  allocate through it, so the total is complete.
- **Owner**: stamped on each allocation when it is made, so nothing depends on an
  owner remembering to report.
  - The engine's wgpu (our fork, branch `helio/v30.0.1`) has *allocation owners*
    (`wgpu::wgt::allocation_owner`): a thread-local stack of owner scopes, and every
    buffer, texture and acceleration structure the Vulkan and DX12 backends
    allocate while one is open is named `owner<US>label` in the allocator report.
    `scope` nests (`Helio/shadow`); `root_scope` starts a new path, for a library
    that owns what it allocates whoever called it.
  - **SceneDB** opens `SceneDB` around every buffer and texture it creates
    (`pulsar_scenedb::gpu::vram::owner`, through a hook `request_engine_device`
    installs, so SceneDB itself builds against any wgpu).
  - **Helio** (`helio_core::vram`): building a graph node opens `Helio/<node kind>`,
    and each pass remembers the owner it was built under (`RenderGraph::add_pass`)
    and runs `prepare` and `execute` in it; a scene derivation runs in
    `Helio/<derivation>`; the graph's targets are `Helio/graph targets`; everything
    else the renderer does (its own targets, uploads, the profiler, building the
    renderer) is `Helio`. Owner names are interned once: opening a scope on a frame
    allocates nothing.
  - The renderer host gives each plugin's host part `Renderer host/<name>`; the game
    window's own target is `Game window`.
  - What is made outside every scope is `Unscoped` (the map's toolbar and the game's
    `GPU_MEMORY` line give its total and labels): the places still to give an owner.
    On the Bistro level that is 0.5 MB, all of it wgpu's own internal buffers.
- **Used** (with `usage`): `VramClaim`s, an owner's word that `used` of the
  `capacity` bytes of the allocations labelled `X` hold live data (`None` when it
  cannot tell). `VramSnapshot::name_use` adds up the claims on a label; a claim's
  owner also names an unscoped allocation of its label.
  - SceneDB reports its own pools: every var-len pool a `SceneGpuStore` creates
    joins `pulsar_scenedb::gpu::vram_use` (a weak list), with capacity and live
    elements from the pool's free list.
  - A plugin submits a `VramProviderRegistration { origin, owner, claims }` from its
    root (`pulsar_plugins::registration!`; the engine names no kind). `claims` runs
    about once a second while a tool reads, off the UI and frame threads.

Not reporting use yet (they show allocated, use "not reported"): SceneDB's row
buffers (growable and dirty-tracked columns: capacity is known, live rows are not
reported), Helio's pass buffers, the texture store. Each is one claim in its owner.

## The VRAM Map (`crates/editor/ui_vram_map`)

A window of its own (Tools menu and status bar, "VRAM Map"). A squarified treemap
(Bruls, Huizing and van Wijk) of the snapshot, refreshed every second:

- **By owner**: device, the owner path (`Helio` › `shadow`), label, then each
  allocation when a label has several. "Free in driver blocks" is the memory the
  process holds that no allocation covers.
- **By block**: each driver memory block, its allocations and free gaps in address
  order as a strip treemap (Bederson, Shneiderman and Wattenberg: reading order,
  near-square boxes): the physical layout, fragmentation visible.
- A box's area is its allocated bytes; its hue is its owner path's first part. A
  leaf's full colour, from the bottom up, is the share its owner reports live; the
  pale rest is allocated and unused. A box with no report is drawn in its full
  colour; the hover line says "use not reported".
- Wheel zooms around the pointer, drag pans, double-click frames a box's group,
  Fit resets. "Usage tracking" off reads only the allocator reports.

The built game logs the same snapshot with `PULSAR_FRAME_STATS` (`GPU_MEMORY`
lines: by owner, the largest by owner and label, and the unscoped labels;
`.agents/GAMEPLAY.md`, round 5).
