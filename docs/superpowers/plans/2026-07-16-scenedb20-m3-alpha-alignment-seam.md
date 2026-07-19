# SceneDB 2.0 — M3-α Implementation Plan (wgpu-30 Alignment, SceneDbBinding Seam, Remaining C5 Buffers)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align `pulsar_scenedb::gpu` with the Helio v4 lineage (upstream wgpu 30), land every remaining SceneDB-side deliverable the M3 passes need (instance-info column, texture store, meshlet buffer, expected-generation harvest column, Test 13 instrumentation), vendor Helio without `[patch]`, and build the `helio-scenedb` binding seam — per M3 design Rev 2 §1.1 (`docs/superpowers/specs/2026-07-16-scenedb20-m3-helio-inversion-design.md`).

**Architecture:** SceneDB's `gpu` feature moves to its own crates.io `wgpu = "30"` dep (coexisting with the workspace fork — distinct sources never unify; empirically verified in the design review). Helio (v4 lineage) is vendored as a **standalone nested-workspace submodule** at `crates/renderer/helio` — NOT a workspace member, NOT patched; only its new `helio-scenedb` crate path-deps `pulsar_scenedb`. Every new buffer follows the shipped M2 machinery patterns exactly (mirrored column ← `write_transform` twin; asset store ← `ClusterBuffer` twin).

**Tech Stack:** Rust 2021; wgpu 30.0 + naga 30 (crates.io) for the gpu feature; the fork remains for the rest of the workspace (M4 gate).

## Global Constraints

- **C0:** `cargo check -p pulsar_scenedb --no-default-features` green; CI no-Helio-edge guard green; the vendored submodule is NEVER added to root `Cargo.toml` members/patches (design Rev 2 finding 1 — a `[patch]` would recompile every legacy consumer against v4 and break the editor).
- **C5:** InstanceInfo = 8 B `{mesh_index u32@0, flags u32@4}`; MeshletEntry = 32 B (layout in Task 6 — lands as a CONTRACTS C5 amendment in the same commit); material row **GATED on R8** (Task 12 does not execute until the Rev 2.4 amendment is approved).
- **Change protocol:** contract amendments in the same commit as the code they sanction, citing the design; spec-side items go on the Rev 2.4 punch list only.
- **wgpu-30 API forms are UNKNOWN until Task 1 discovers them** — later tasks must use whatever forms Task 1's migration establishes (read the migrated code, not the old fork idioms). The 6-suite matrix is the oracle.
- **Two workspaces:** Pulsar-Native tasks run from repo root; Helio-side tasks (9–11) run from `crates/renderer/helio` (`cd` there for cargo commands). GPU suites sequential, `--test-threads=1`, both workspaces.
- **Windows:** author `.rs`/`.toml`/`.wgsl` ONLY via Write/Edit tools (BOM hazard).
- **Commits:** `type(scenedb): …` + trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Submodule-internal commits use the same convention on the `scenedb20-m3` branch and are pushed to the Helio remote; Pulsar-Native then commits the updated gitlink.
- **Test commands (post-Task-1 canonical):** core `cargo test -p pulsar_scenedb --lib --tests`; gpu-lib `cargo test -p pulsar_scenedb --features gpu --lib`; GPU targets `--test gpu_store|gpu_harvest|gpu_assets -- --test-threads=1`, `--test gpu_layout`, `--features gpu --doc`; guard `--no-default-features`.

## File Structure

```
Pulsar-Native/
  crates/core/pulsar_scenedb/Cargo.toml       # wgpu="30" own dep; naga="30"          [T1]
  crates/core/pulsar_scenedb/src/gpu/*         # wgpu-30 API drift                     [T1]
  crates/core/pulsar_reflection/Cargo.toml     # gpui-ce/ui behind `prims-gpui`        [T2]
  .gitmodules + crates/renderer/helio          # submodule, v4-lineage branch          [T3]
  crates/core/pulsar_scenedb/src/spatial.rs    # with_transform gains InstanceInfo col [T4]
  crates/core/pulsar_scenedb/src/gpu/scene_store.rs  # instance-info mirror machinery  [T4]
  crates/core/pulsar_scenedb/src/gpu/assets.rs # TextureStore, MeshletBuffer, VERTEX
                                               # usage, write counters                 [T5,T6,T7]
  crates/core/pulsar_scenedb/src/gpu/harvest.rs# expected-gen aligned columns          [T8]
  docs/superpowers/specs/CONTRACTS.md          # C5: InstanceInfo + MeshletEntry rows  [T4,T6]
  tests/gpu_store.rs, gpu_assets.rs, gpu_harvest.rs, gpu_layout.rs  # per task
crates/renderer/helio/ (nested workspace, branch scenedb20-m3)
  crates/helio-scenedb/                        # SceneDbBinding + WGSL + smoke         [T9,T10]
  crates/helio-scenedb/tests/binding_layout.rs # Helio-side Test 3 harness (NEW infra) [T11]
```

---

### Task 1: wgpu-30 migration of the `gpu` feature

**Files:**
- Modify: `crates/core/pulsar_scenedb/Cargo.toml`
- Modify: `crates/core/pulsar_scenedb/src/gpu/*.rs` + `tests/gpu_*.rs` (API drift only — zero semantic changes)
- Modify: `.github/workflows/ci.yml` (one added guard)

**Interfaces:**
- Produces: the crate's gpu feature on `wgpu = { version = "30", optional = true }` (own dep — NOT `workspace = true`; the workspace entry stays on the fork for everyone else) and `naga = { version = "30", features = ["wgsl-in"] }` dev-dep (crates.io, replaces the git-pinned fork naga — delete the lockstep comment). Every later task uses the API forms this task establishes.

- [ ] **Step 1: Swap the deps.** In `[dependencies]`: `wgpu = { version = "30", optional = true }` (replacing `workspace = true, optional = true`; keep the C0 comment block, amend its last paragraph: the gpu feature now uses upstream wgpu 30 per the M3 lineage decision — the fork remains the workspace dep for legacy consumers until the M4 gate). In `[dev-dependencies]`: `naga = { version = "30", features = ["wgsl-in"] }`.
- [ ] **Step 2: Compile and migrate.** `cargo check -p pulsar_scenedb --features gpu` — fix every error mechanically, preserving semantics exactly. Known drift candidates from fork-28 → upstream 30 (verify against rustc suggestions, do not guess): `Instance::new` argument form; `PollType`/poll-result shape (the fork's `PollType::wait_indefinitely()` convenience may not exist upstream — use the upstream equivalent of wait-idle); `request_adapter`/`request_device` signatures/return types; buffer-mapping callback forms; `naga::proc::Layouter`/`to_ctx` drift in gpu_layout tests. Record every change in the report (form → form).
- [ ] **Step 3: Full matrix (the oracle).** All of: `--no-default-features` check; core `--lib --tests` (132); `--features gpu --lib` (84+); `gpu_store` (23), `gpu_harvest` (8), `gpu_assets` (17) each `--test-threads=1`; `gpu_layout` (5); `--features gpu --doc` (6). ALL green — any behavioral difference is a STOP-and-report, not a workaround.
- [ ] **Step 4: CI guard addition.** After the existing no-Helio-edge step: a step `SceneDB resolves outside the workspace (submodule-build precondition)` that creates a scratch crate in the runner temp dir with `pulsar_scenedb = { path = "<abs>", features = ["gpu"] }` and runs `cargo metadata` on it (asserts no reliance on root `[patch]` tables — the vendored Helio build resolves this way).
- [ ] **Step 5: Commit** — `feat(scenedb): migrate gpu feature to upstream wgpu 30 (M3-a T1, lineage decision)`

---

### Task 2: Cut the `pulsar_reflection → gpui-ce/ui` edge

**Files:**
- Modify: `crates/core/pulsar_reflection/Cargo.toml` (+ its `src/prims/` gating), consumers' Cargo.tomls as discovered.

**Interfaces:**
- Produces: `gpui-ce`/`ui` as optional deps behind a `prims-gpui` feature (in `default` for now — flip below); consumers that need the gpui prim registrations declare it explicitly; `pulsar_scenedb`'s tree becomes fork-wgpu-free.

This is the **prims-helio playbook from the holistic audit, applied to the gpui edge** (that remediation commit — `a6c9fc75`, pulsar_reflection/Cargo.toml — is the template; read it first). Steps: (1) make `gpui-ce`/`ui` optional, gate the gpui prim module(s) under `prims-gpui = ["dep:gpui-ce", "dep:ui"]`; (2) unlike prims-helio (no users), gpui prims likely HAVE users — grep consumers (the 14 from the audit) for gpui prim types/registrations; add `features = ["prims-gpui"]` to exactly those consumers, keep `prims-gpui` OUT of default only if every user is explicit — if the editor stack's usage is diffuse, keep it IN default and instead have `pulsar_scenedb` opt out via `default-features = false, features = [<the non-gpui set>]`… **NO** — the audit proved per-edge default-features-false is defeated by feature unification. The ONLY working shape is: `prims-gpui` NOT in default + explicit features on every consumer that needs it. Verify each affected consumer compiles individually (audit-remediation report shows the command pattern). (3) Prove the cut: `cargo tree -p pulsar_scenedb --features gpu | grep -iE 'gpui|wgpui'` → empty; the no-Helio guard stays green. (4) Full scenedb matrix + compile checks on every touched consumer.
- [ ] **Commit** — `fix(scenedb): cut transitive gpui-ce/fork-wgpu edge from reflection (M3-a T2, audit follow-up)`

---

### Task 3: Vendor Helio as a standalone submodule (NO patch)

**Files:**
- Helio repo: branch `scenedb20-m3` created from the v4 lineage tip and pushed; the existing docs-only `scenedb20` branch rebased onto the same tip (CONTRACTS mirror refreshed from Pulsar-Native's current CONTRACTS.md incl. all audit amendments).
- Pulsar-Native: `.gitmodules` + gitlink at `crates/renderer/helio`. Root `Cargo.toml` UNTOUCHED.

Steps: (1) in `C:\Users\Sepehr\Desktop\Dev\Helio`: fetch; `git branch scenedb20-m3 origin/v4` (the v4 tip — record the exact SHA in the report); push it; rebase `scenedb20` onto it (docs-only — trivial; re-copy CONTRACTS.md from Pulsar-Native, commit, push). (2) in Pulsar-Native: `git submodule add -b scenedb20-m3 https://github.com/Far-Beyond-Pulsar/Helio.git crates/renderer/helio`. (3) Prove standalone health: from `crates/renderer/helio`, `cargo metadata --format-version 1 > NUL` exit 0 and `cargo check -p helio-core` compiles (wgpu 30 from crates.io — first build is heavy, allow 15+ min). (4) Prove isolation: root `cargo check -p pulsar_scenedb --no-default-features` unchanged; `git diff Cargo.toml` empty. (5) Commit Pulsar-Native (`.gitmodules` + gitlink) — `feat(scenedb): vendor Helio v4 lineage as standalone submodule (M3-a T3, no patch — design Rev 2 §6)`.

---

### Task 4: Instance-info mirrored column

**Files:**
- Modify: `src/page.rs` (Pod for the new type — no; the type lives gpu-adjacent but the COLUMN is core: define `InstanceInfo` in `src/spatial.rs` with `unsafe impl Pod`), `src/spatial.rs`, `src/gpu/scene_store.rs`, `docs/superpowers/specs/CONTRACTS.md` (C5 row), `tests/gpu_store.rs`, `tests/gpu_layout.rs`.

**Interfaces:**
- Produces (mirror `write_transform`'s machinery EXACTLY — same file, adjacent code):

```rust
// spatial.rs (graphics-free core — plain Pod data)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InstanceInfo {
    pub mesh_index: u32, // @0 — MeshRegistry index (LOD 0 entry per R9)
    pub flags: u32,      // @4 — bit 0 reserved: near-clip CPU twin (M3-β); rest 0
}
const _: () = assert!(std::mem::size_of::<InstanceInfo>() == 8);
unsafe impl crate::page::Pod for InstanceInfo {}

pub const INSTANCE_INFO_COLUMN: usize = SPATIAL_COLUMNS + 1; // 7
// with_transform: columns become [6×f32, [f32;16] @6, InstanceInfo @7]
// (stride 24+64+8 = 96 user bytes ≤ 128 C2 ✓); register_token_column::<InstanceInfo>(7).
```

- `SceneGpuStore` gains: `instance_infos: SceneBuffer<InstanceInfo>` (row capacity, same as transforms); per-cell `dirty_infos: DirtyMask` in `CellGpuState`; `pub fn write_instance_info(&self, id: CellId, cell: &mut CellStorage, handle: Handle, info: InstanceInfo, _sim: &impl SimulateWitness) -> bool` — body identical to `write_transform` minus the generation stamp (the stamp stays transform-only: one stamping path, documented); `compact_all` marks moved destinations in `dirty_infos` too; `sync_all` syncs the info column region (via `column_for::<InstanceInfo>()`); `rebuild` bulk-writes it; `register_cell` warm-up mark_range; `instance_info_buffer()` accessor.
- CONTRACTS C5 amendment (same commit): "Instance info (SceneDB-owned, amendment M3-α): 8 B per row — mesh_index u32@0 (LOD-0 MeshRegistry entry, R9), flags u32@4 (bit 0 near-clip twin, rest reserved 0). Row-indexed beside instance transforms."
- Tests: gpu_store — write_instance_info→boundary→readback byte-exact; compaction move carries the info (mirror `compaction_move_is_resynced` shape); Test 14 multicell extended to assert info-region byte identity. gpu_layout — WGSL `struct InstanceInfo { mesh_index: u32, flags: u32 }` size 8, offsets [0,4].
- [ ] TDD steps per the house pattern; full matrix; commit `feat(scenedb): instance-info mirrored column — cull's token->mesh link (M3-a T4, C5 amendment)`

---

### Task 5: TextureStore (SceneDB-owned textures + bindless slot table)

**Files:** `src/gpu/assets.rs` (+mod.rs re-exports `TextureStore, TextureError`), `tests/gpu_assets.rs`.

**Interfaces:**

```rust
pub const MAX_TEXTURE_SLOTS: u32 = 16384; // spec §10 G4 / recon ceiling
pub enum TextureError { SlotsExhausted, SlotVacant, SlotOutOfRange }
pub struct TextureStore { /* textures: Vec<Option<wgpu::Texture>>, free: Vec<u32>, next: u32, upload_count: u64 */ }
impl TextureStore {
    pub fn new(max_slots: u32) -> Self;               // asserts ≤ MAX_TEXTURE_SLOTS
    /// Owns the wgpu::Texture (C0/§10 G4 — Test 13: textures survive renderer teardown).
    /// Caller retains source data for device-loss re-registration (Test 14; store is residency only).
    pub fn register(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
                    desc: &wgpu::TextureDescriptor, data: &[u8]) -> Result<u32, TextureError>;
    pub fn unregister(&mut self, slot: u32) -> Result<(), TextureError>; // frees slot; slot ids recycle (LIFO)
    pub fn texture(&self, slot: u32) -> Option<&wgpu::Texture>;          // Helio builds VIEWS from these
    pub fn slot_count(&self) -> u32;
    #[doc(hidden)] pub fn upload_count(&self) -> u64;  // Test 13 instrumentation
}
```
Upload via `queue.write_texture` (mip 0, layout derived from desc — document single-mip M3-α scope; mip chains ride to the asset pipeline). Tests: register two textures → slots 0,1; `texture(slot)` present; unregister → slot recycled LIFO; exhaustion at a small max → `SlotsExhausted`; readback one texture's bytes via a copy-to-buffer (reuse `readback` after `copy_texture_to_buffer` — add a small `readback_texture` helper) — byte-exact vs source.
- [ ] TDD; matrix; commit `feat(scenedb): TextureStore — SceneDB-owned textures + bindless slot table (M3-a T5, spec §10 G4)`

---

### Task 6: MeshletBuffer (32 B, C5 amendment) + GeometryArena `VERTEX` usage

**Files:** `src/gpu/assets.rs` (+re-exports `MeshletBuffer, MeshletEntry, MeshletError`), CONTRACTS.md, `tests/gpu_assets.rs`, `tests/gpu_layout.rs`.

**MeshletEntry layout (this IS the C5 amendment — spec §19 gives size+contents only; layout defined here, R12 on the punch list):**

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshletEntry {
    pub sphere_x: f32,      // @0  bounding sphere center
    pub sphere_y: f32,      // @4
    pub sphere_z: f32,      // @8
    pub sphere_radius: f32, // @12
    pub cone_packed: u32,   // @16 i8x3 axis (snorm) | i8 cutoff (snorm sin φ) — §17.2 backface test
    pub data_offset: u32,   // @20 element offset into the geometry index buffer (meshlet-local indices base)
    pub counts_packed: u32, // @24 vertex_count u8 | triangle_count u8 << 8 | reserved u16 << 16 (=0)
    pub reserved: u32,      // @28 must be 0
}                            // = 32 bytes (spec §19)
const _: () = assert!(std::mem::size_of::<MeshletEntry>() == 32);
```
`MeshletBuffer` mirrors `ClusterBuffer` exactly (append with validation: `sphere_radius > 0.0` in the NaN-rejecting `!(r > 0.0)` REJECTS form, `reserved == 0`, counts nonzero and ≤ 255 each; `BufferFull`; bulk `rebuild`; `nodes()`-style `entries()`; upload counter from Task 7's pattern — see note there). CONTRACTS C5 gains the row (cite design Rev 2 §2 + R12). Same commit: `GeometryArena::new` vertex buffer usage gains `| wgpu::BufferUsages::VERTEX` (design: classic vertex fetch default; one line + doc note). Tests: mirror the ClusterBuffer suite (offsets, byte-exact readback, each validation arm, corruption-heal rebuild) + gpu_layout WGSL row (size 32, scalar offsets).
- [ ] TDD; matrix; commit `feat(scenedb): MeshletBuffer (C5 32B amendment) + VERTEX usage on geometry (M3-a T6)`

---

### Task 7: Asset-store write counters (Test 13 instrumentation)

**Files:** `src/gpu/assets.rs`, `src/gpu/grid.rs` (cell-metadata write counter), `tests/gpu_assets.rs`.

One shared pattern: each store gains a `u64` upload counter incremented at every `write_buffer`/`write_texture` call site (`#[doc(hidden)] pub fn upload_count(&self) -> u64` on `GeometryArena` (both buffers, one counter), `MeshRegistry`, `ClusterBuffer`, `MeshletBuffer`; `TextureStore` has it from T5; `StreamingGrid::write_cell_metadata` counts calls via a `&mut self`… it takes `&self`? check — as shipped it's `&self`; make the counter `AtomicU64` there). Doc on each: "Test 13 instrumentation: the teardown gate asserts these do not move across the renderer drop/rebind window." Tests: one test per store asserting the counter increments on upload and NOT on rejected registrations.
- [ ] TDD; matrix; commit `test(scenedb): asset-store upload counters — Test 13 instrumentation (M3-a T7)`

---

### Task 8: Expected-generation harvest column (§3.1 — Test 2's data path)

**Files:** `src/gpu/harvest.rs`, `tests/gpu_harvest.rs`.

**Interfaces:**
- `HarvestStaging` gains `pub traditional_gens: Vec<u32>, pub vg_gens: Vec<u32>, pub hlod_gens: Vec<u32>` — positionally aligned with their token arrays (C4 "aligned across columns"); `clear()` clears them (capacity-preserving).
- `harvest_cell`: plain path — for each valid token also push `gens[..] = registry_generations[slot_column[local_row] as usize]` (bind `let regs = cell.storage().registry().generations(); let col0 = cell.storage().slot_column();` before the loop). DEI path — after `compress_tokens`, iterate the NEW remap segment: `for &ri in &staging.remap[remap_start..] { dest_gens.push(regs[col0[ri as usize] as usize]); }` (remap holds LOCAL run indices — exactly what col0 needs).
- Invariant (doc + debug_assert): `dest.len() == dest_gens.len()` after every cell.
- Tests: (1) alignment — for every emitted token, independently recompute the expected gen from the cell and assert equality, both plain and DEI paths (extend the existing routing + DEI tests rather than duplicating setups); (2) staleness detectability — free_deferred a harvested handle, force-complete + boundary (gen bumps), re-harvest: the OLD run's gens no longer match live `generations()` for that slot while the NEW run's do (this is Test 2's CPU-side half; the shader half is M3-β); (3) zero-alloc warm-up extended to the gens Vecs.
- [ ] TDD; matrix (gpu_harvest grows to ~11); commit `feat(scenedb): expected-generation aligned harvest column (M3-a T8, design §3.1 — Test 2 data path)`

---

### Task 9: `helio-scenedb` seam crate (in the submodule)

**Files (all inside `crates/renderer/helio`, branch `scenedb20-m3`):**
- Create: `crates/helio-scenedb/Cargo.toml` (+ add to Helio workspace members), `src/lib.rs`, `src/wgsl.rs` (the shared WGSL declarations as `pub const SCENE_BINDINGS_WGSL: &str`), smoke test.

**Interfaces:**

```toml
[dependencies]
pulsar_scenedb = { path = "../../../../core/pulsar_scenedb", features = ["gpu"] }
wgpu = { workspace = true }   # Helio's workspace wgpu = 30 — MUST resolve to the same
                              # major as scenedb's own dep; assert in a test (see below)
```
```rust
pub struct SceneDbBinding {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}
impl SceneDbBinding {
    /// Rebuilt at renderer construction — Test 13's mechanism. Binds:
    /// 0 instance transforms, 1 instance info, 2 slot mirror, 3 generations,
    /// 4 mesh configurator, 5 cluster DAG, 6 meshlets, 7 cell metadata,
    /// (geometry V/I bind at draw, textures via Helio's bind array).
    pub fn new(device: &wgpu::Device, store: &SceneGpuStore, meshes: &MeshRegistry,
               clusters: &ClusterBuffer, meshlets: &MeshletBuffer) -> Self;
}
```
All entries read-only storage buffers, layout per `SCENE_BINDINGS_WGSL` (write the WGSL: the C5 structs — Instance mat4, InstanceInfo, MeshMetadata 72 B, ClusterNode 48 B, MeshletEntry 32 B, CellMeta {alpha f32, domain u32} — plus `array<u32>` mirrors/generations; scalar fields only). **Smoke test (the seam proof):** build a real `SceneGpuStore` + one cell, `write_transform` + `write_instance_info`, boundary; construct `SceneDbBinding`; run a 4-line compute shader through the bind group copying `instances[0]` and `instance_info[0]` into a Helio-owned output buffer; readback equals the written values. Plus a version-lock test: `assert_eq!(env!("CARGO_PKG_VERSION")…` — no; simplest honest lock: the smoke test itself IS the proof (mismatched wgpu majors would fail at compile on the `wgpu::Buffer` types crossing the crate boundary). Add a comment saying exactly that.
Commits: submodule commit first (`feat(helio-scenedb): SceneDbBinding seam …` on scenedb20-m3, pushed), then Pulsar-Native gitlink bump commit.
- [ ] Steps: crate + WGSL + failing smoke → implement → `cd crates/renderer/helio && cargo test -p helio-scenedb -- --test-threads=1` green → both commits.

---

### Task 10: Helio-side Test 3 reflection harness (NEW infrastructure)

**Files:** `crates/renderer/helio/crates/helio-scenedb/tests/binding_layout.rs` (+ naga dev-dep `naga = { version = "30", features = ["wgsl-in"] }` in helio-scenedb).

Port the Pulsar-side `wgsl_struct_layout` harness (tests/gpu_layout.rs — copy the helper, adapt to naga 30 if T1 changed the API) and reflect `SCENE_BINDINGS_WGSL` (the ACTUAL seam WGSL — that's the point: the renderer-side source of truth): assert Instance 64/InstanceInfo 8/MeshMetadata 72 (all 18 offsets)/ClusterNode 48/MeshletEntry 32/CellMeta 8 against `std::mem::size_of` of the re-exported Rust types (`pulsar_scenedb::gpu::{MeshMetadata, ClusterNode, MeshletEntry, InstanceInfo…}` — re-export whatever's missing from mod.rs in a tiny Pulsar-side commit if needed). This harness is what M3-β/γ extend for every new shader struct.
- [ ] TDD; submodule + gitlink commits — `test(helio-scenedb): Test 3 reflection harness over the seam WGSL (M3-a T10)`

---

### Task 11: Material buffer — **GATED ON R8** (do not execute until the Rev 2.4 material amendment is approved)

Placeholder scope (finalize from the approved R8 text): `#[repr(C)] MaterialRow` (64 B per the design's proposal — exact fields from R8), `MaterialRegistry` mirroring MeshRegistry (register/get/entries/rebuild/upload counter, validation per R8), `SceneGpuStore::material_buffer` re-sized/strided if R8 differs from the 32 B placeholder (buffer stride comment updated), C5 CONTRACTS row, Test 3 rows both sides, binding entry added to `SceneDbBinding`+WGSL. **The executing agent must read the approved amendment text (Research repo draft / CONTRACTS) and STOP if it has not landed.**

---

### Task 12: Docs wrap + dual-workspace acceptance matrix

lib.rs milestone status (M3-α complete; β next), README (submodule build instructions: `git submodule update --init crates/renderer/helio`; the two-workspace test matrix; wgpu-30/fork coexistence note), design doc Status line → "M3-α implemented". Matrix (ALL green, sequential): Pulsar side — the 8 commands from Global Constraints; Helio side — `cd crates/renderer/helio && cargo test -p helio-scenedb -- --test-threads=1`. Commit `docs(scenedb): M3-alpha docs — alignment + seam complete`.

## Deferred

M3-β (cull/indirect passes, Cull/Draw witnesses, Tests 13/2/4/5), M3-γ (VG/HLOD), everything in the design's M4 gate (§6/§10), R8 execution if still unapproved (Task 11 carries to β's plan in that case).

## Verification (end-to-end)

Task 12's dual matrix. New named gates: the seam smoke test (T9 — first real cross-crate buffer share on wgpu 30), the Helio-side Test 3 harness (T10), the expected-gen alignment + staleness tests (T8 — Test 2's data path), Test 14 extension with instance-info (T4).
