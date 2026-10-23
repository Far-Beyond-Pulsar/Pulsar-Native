# RFC: Realtime Texel Scaling — one density signal for geometry *and* texels

- **Status:** Draft / discussion — *not* a committed plan. Handing up for direction.
- **Author:** Sepehr
- **Owner to weigh in:** @tristanpoland
- **Date:** 2026-07-23
- **Scope note:** This is a north-star + decomposition brief, deliberately above the level of a SceneDB-grade phase plan. It exists to agree on *whether* and *in what order*, not *how* down to the buffer layout. Each slice below would get its own spec → plan → build cycle once we pick a first bite.

---

## TL;DR

Helio's **geometry** pipeline is already Nanite-class: it selects level-of-detail by *projected pixel error*, every frame, on the GPU, with no CPU readback, assisted by a Hi-Z pyramid and bounded by an explicit budget. Helio's **textures** do none of that. Every texture is uploaded monolithic at full resolution with **a single mip level** (`mip_level_count: 1`) and pinned in VRAM.

So the engine already measures, per frame, *how much geometric detail the screen needs* — and then ignores the identical question for texels.

The proposal is to close that asymmetry and, in doing so, unify the two. Compute the **texel analog** of the geometry LOD error term, drive texture residency with it, and eventually merge the two feedback loops into **one density signal** that arbitrates a **single streaming budget** across geometry and texels. The payoff is not one win but three, from one mechanism:

1. **Workflow** — artists ship one high-res asset. The engine derives mips / pages / residency. Nobody hand-authors a mip chain or an LOD ladder for textures ever again.
2. **Performance** — the resident texel working set becomes *bounded by what the screen needs*, not by how many textures the scene references. VRAM stops scaling with content and starts scaling with screen resolution.
3. **Quality** — sampling always at the correct density means no shimmer/aliasing from over-sampling and no blur from under-sampling. Correct-mip-always is a quality result, not just a perf one.

These are not in tension: a single "what does the screen actually need right now" signal *simultaneously* bounds the working set (perf), hits the right density (quality), and removes the authoring step (workflow).

---

## 1. The asymmetry, in code

### 1.1 Geometry already scales detail by pixel error

`crates/renderer/helio/crates/helio-pass-virtual-geometry/`:

- **Per-frame, GPU-driven LOD selection.** `select_object_lod` (lib.rs) computes the projected screen error of each candidate LOD and picks the coarsest one under a pixel budget:

  ```
  projected_error = geometric_error[level] * max_scale * focal_pixels / distance
  select coarsest level where projected_error <= max_error_pixels
  ```

- **A quality ladder expressed in pixels.** `LodQuality { Low, Medium, High, Ultra }` maps directly to a tolerated error in *output pixels*: `4.0 / 2.0 / 1.0 / 0.5` px (`LodQuality::max_error_pixels`).
- **Two GPU compute stages, no readback.** A "select" pass chooses one LOD per object; a "cull" pass culls the selected LOD's meshlets in fixed 64-meshlet spans (`VG_CULL_MESHLETS_PER_WORK_ITEM`). CPU readback exists *only* on the debug path (`DebugReadbackState`).
- **Hi-Z assisted.** `CullUniforms` carries `hiz_mip_count` and `lod_error_threshold_px`; occlusion uses the Hi-Z pyramid (`helio-pass-hiz`).
- **Explicitly budgeted.** `VirtualGeometryBudget` caps published meshlets (`DEFAULT_MAX_PUBLISHED_MESHLETS = 262_144`) and clamps worst-case draw counts.

This is a mature, shipping, pixel-error-driven residency system.

### 1.2 Textures do not scale at all

`crates/renderer/helio/crates/helio/src/scene/resources/textures.rs`:

- `insert_texture` uploads the whole image via `create_texture_with_data` with **`mip_level_count: 1`** (textures.rs:74). No mip chain is generated, ever.
- The view is a plain default view (textures.rs:84); usage is `TEXTURE_BINDING | COPY_DST`. There is no partially-resident texture, no page table, no streaming path.
- Textures live in a bindless pool (doc comment: "maximum of 16384 concurrent textures … bindless array limit"), referenced by material slots (`MaterialTextures`: `base_color`, `normal`, `roughness_metallic`, `emissive`, `occlusion`, `specular_color`, `specular_weight`) through `GpuMaterialTextureSlot { texture_index }`.

The consequences follow directly:

- **Every referenced texture is fully resident at full res, whether it covers 4 000 pixels or 4.** VRAM scales with *content count*, not with *screen need*.
- **No mip chain ⇒ minification shimmer.** A single-mip texture sampled below its Nyquist rate aliases. Correct trilinear/anisotropic filtering isn't even possible.
- **Distance is free of consequence.** A texture on a mountain 8 km away is as expensive as one on the weapon in your hands.

### 1.3 The engine already *promises* the fix in its own settings

`crates/core/pulsar_settings/src/project/streaming.rs` declares — as user-facing toggles with **no implementation behind them**:

- `texture_streaming` — *"Stream texture mip levels progressively (reduces VRAM usage)"* — default **on**.
- `texture_stream_pool_mb` — VRAM budget for the texture streaming pool, default **512 MB**.
- `virtual_texturing_enabled` — *"Enable runtime virtual texturing (RVT) for large terrain surfaces"* — default off.
- `virtual_texture_tile_size` — 64 / 128 / 256 / 512, default **128**.
- `nanite_streaming` — geometry streaming toggle.

The intent is already committed to the repo. The switches exist; nothing turns. This RFC is, in one sentence, *"make those switches do the thing they claim."*

---

## 2. The core idea: one density signal

The insight that makes this a *synthesis* rather than a bolt-on: **geometry LOD selection and texture residency want the same inputs and answer the same question.** Both need `focal_pixels`, screen dimensions, per-surface distance, and the Hi-Z pyramid. Both are asking *"at what density does the camera need this surface, right now?"* Geometry answers it for triangles; nothing answers it for texels.

So we build the texel answer as a mirror of the geometry answer, and then — the actual innovation — **fuse them into a single per-frame density pass feeding a single budget arbiter.** Today, three residency systems each guess independently and contend for the same PCIe bus:

- **SceneDB** decides which geometry is resident and GPU-culls it.
- **helio-pass-virtual-geometry** decides which meshlet LOD is drawn.
- *(missing)* a texel residency system.

Adding VT as a *fourth* independent guesser would be the naive move. The Pulsar-specific opportunity is that SceneDB already owns GPU-side residency bookkeeping, so the texel loop can share it — one signal, one budget, no subsystem double-paying for the same pixel.

```
          ┌─────────────────────────────────────────────┐
          │   Per-frame density pass (reuses Hi-Z)       │
          │   "what density does each visible surface    │
          │    need?" → geometric error + texel error    │
          └───────────────┬─────────────────┬───────────┘
                          │                 │
                geometry LOD          texel page / mip
                selection             residency need
                          │                 │
          ┌───────────────▼─────────────────▼───────────┐
          │      Single streaming-budget arbiter          │
          │   (VRAM + PCIe bandwidth, one pool)           │
          └───────────────┬─────────────────┬───────────┘
                  meshlet publication   texel page uploads
```

---

## 3. What this reuses (the reason it's credible)

The hard, scary parts of a virtual-texturing system already exist in Helio in the geometry form and are proven in production. The texel system is largely a *re-parameterization* of machinery that already runs every frame:

| Concern | Geometry (exists today) | Texel (this RFC) |
|---|---|---|
| Detail metric | projected pixel error `err·scale·focal/dist` | texel/pixel ratio (∂uv/∂screen) → needed mip/page |
| Quality ladder | `LodQuality` → {4,2,1,0.5} px | same knob → target texels-per-pixel |
| Feedback substrate | Hi-Z pyramid (`helio-pass-hiz`) | same Hi-Z, plus a small feedback target |
| Selection | GPU "select" compute pass | GPU "needed pages" compute pass |
| No CPU readback | debug-only readback | same discipline (compact GPU-side need buffer) |
| Budget | `VirtualGeometryBudget` (meshlets) | `TexelBudget` (resident pages / pool MB) |
| Bounded publication | `clamp_draw_count` | clamp resident page uploads/frame |

We are not inventing VT from a cold start. We are extending an existing, working GPU-driven residency pattern to a second resource type, then merging the control planes.

---

## 4. Decomposition — four slices, each ships a real win

Each slice is independently demoable and de-risks the next. Ordering is a recommendation, not a mandate — see §6.

### Slice 1 — Density signal + auto-mip residency *(the foundation)*
Generate mip chains on ingest (kills the authoring step immediately). Add a lightweight GPU feedback pass that records, per visible texture, the **finest mip actually sampled** (from uv derivatives, or a dedicated mip-feedback target). A residency manager keeps only mips at-or-coarser-than needed resident and evicts the rest against `texture_stream_pool_mb`.

*Granularity:* whole mips per texture — **no page table, no shader indirection yet.** This is the smallest coherent bite, highest certainty, and it ships **all three wins in their simplest honest form**: auto-mips (workflow), bounded resident set (perf/mem), correct mip (quality). Crucially, it builds the **density-feedback primitive** that Slices 2–4 all depend on. Wires up the existing `texture_streaming` setting for real.

### Slice 2 — Sparse virtual texture *(true residency)*
Split textures into pages (`virtual_texture_tile_size`, 128 default). Introduce a page table + physical page atlas; the shader samples through one indirection. Feedback now records **needed pages**, not whole mips. Working set becomes constant regardless of texture count or authored size. This is the slice that makes **planetary megatexture** possible (one giant virtual texture over a planet surface, pages streamed by camera — ties into the #357/#358 terrain-delta work). Wires up `virtual_texturing_enabled`.

### Slice 3 — Unify with geometry feedback *(the actual innovation)*
Merge the texel density signal and the geometry LOD signal into **one feedback pass** and **one budget arbiter**. Geometry LOD and texel page residency negotiate the same VRAM + bandwidth pool, so no subsystem double-pays and the engine degrades gracefully under pressure by trading detail where it's least visible. This is the defensible engine-level claim, and it's only cheap because SceneDB already owns residency bookkeeping.

### Slice 4 — Decoupled texel-space shading *(the ceiling, later)*
Shade into the page atlas (texel space) at a rate **decoupled** from screen pixels, caching and amortizing shading across frames. Highest risk, highest reward: near-free temporal stability and expensive-shading amortization. Explicitly last; it presumes Slices 1–3.

| # | Slice | Ships | Enables |
|---|---|---|---|
| 1 | Density signal + auto-mip residency | auto-mips · bounded set · no shimmer | the feedback primitive |
| 2 | Sparse virtual texture | constant working set · true streaming | planetary megatexture |
| 3 | Unify geometry + texel feedback | one budget, no double-pay | the innovation claim |
| 4 | Decoupled texel-space shading | temporal stability · amortized shading | perf/quality ceiling |

---

## 5. Prior art, and why Pulsar's version is not a clone

- **UE5 Virtual Textures / SVT + Nanite** — two *separate* virtualization systems (texture and geometry) with independent feedback and budgets. Our differentiator is Slice 3: **one** feedback signal and **one** budget across both.
- **id Tech MegaTexture / RAGE** — unique, non-repeating texel space over huge surfaces, but authored/baked and infamous for pop and disk cost. Our density signal is derived, not baked; the planetary case (Slice 2) is the modern take.
- **Texel/object-space shading** (Slice 4) — active research area; only sane to attempt *after* a residency substrate exists.

Pulsar's specific unlock is that SceneDB already centralizes GPU residency, so the "one budget for everything" architecture is cheap here in a way it isn't in engines where geometry and texture streaming grew up as strangers.

---

## 6. Where to take the first bite — Tristan's call

My recommendation is **Slice 1**: it is the smallest bite with the highest certainty, it ships all three wins immediately, it turns on a setting the engine already advertises, and it builds the feedback primitive everything else needs. Proving the density loop before adding page-table indirection de-risks the entire program.

Two credible alternatives I'd want your read on:

- **Slice 2 first** — skip whole-mip streaming, go straight to page-granular VT. Gets the "real" thing sooner but lands three hard subsystems (page table, atlas, shader indirection) at once with no proven feedback loop underneath. Higher risk, slower to first demoable win.
- **Planetary-first** — build the density + VT machinery *specifically* for the Helio planet surface as a bounded testbed, tightly coupled to #357/#358, and generalize later. Fastest path to a killer orbit-to-ground demo, but front-loads planetary coordinate/streaming complexity.

---

## 7. Open questions for direction

1. **Priority vs. SceneDB roadmap.** Does this slot after M3-γ, or is it a parallel track? It touches Helio and SceneDB residency, so sequencing matters.
2. **Feedback mechanism.** Analytic uv-derivative estimate in the gbuffer pass vs. a dedicated low-res feedback render target (the classic VT approach). Trade precision for a pass.
3. **Ingest ownership.** Mip generation on import — does it live in `helio-asset-compat`'s `texture_loader`, in the cooker, or both (editor-time preview vs. cooked)?
4. **Budget authority.** Should the unified arbiter (Slice 3) live in SceneDB (it owns residency) or Helio (it owns the frame)? This is the load-bearing architectural decision.
5. **Scope guard.** Is Slice 4 (decoupled shading) in the vision at all, or do we explicitly cap the ambition at Slice 3 for now?

---

## Appendix — code references

- `crates/renderer/helio/crates/helio/src/scene/resources/textures.rs:74` — `mip_level_count: 1` (the gap)
- `crates/renderer/helio/crates/helio-pass-virtual-geometry/src/lib.rs` — `LodQuality`, `select_object_lod`, `VirtualGeometryBudget`, `CullUniforms { hiz_mip_count, lod_error_threshold_px }`
- `crates/renderer/helio/crates/helio-pass-virtual-geometry/src/rendering.rs` — two-stage select/cull pass, debug-only readback
- `crates/renderer/helio/crates/helio-pass-hiz/` — Hi-Z pyramid (shared feedback substrate)
- `crates/renderer/helio/crates/helio/src/material.rs` — `MaterialTextures`, `GpuMaterialTextureSlot`, bindless slots
- `crates/core/pulsar_settings/src/project/streaming.rs:33-77` — `texture_streaming`, `texture_stream_pool_mb`, `virtual_texturing_enabled`, `virtual_texture_tile_size` (the unimplemented promises)
