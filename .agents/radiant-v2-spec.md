# Helio Radiant v2 - Material Pipeline Specification

> **Status**: Draft
> **Supersedes**: material_class_ranges, per-pass template_registry, TransparentPass fixed shader
> **Design goal**: A fully generic material evaluation system where every render pass declares evaluation points, every material declares which points it targets, and the engine compiles only the variants needed for each point with zero overhead for materials that opt out of a pass.

---

## How to read this document

This specification is assembled from seven focused sub-specifications. Each covers one subsystem in depth.

- [Helio Radiant v2 — Eval Point Registry](01-eval-point-registry.md)
- [Helio Radiant v2 — Material Registration & Shader Composition](02-material-registration.md)
- [Helio Radiant v2 — Instance Data & GPU Dispatch](03-instance-dispatch.md)
- [Helio Radiant v2 — Fragment Pass Integration](04-fragment-passes.md)
- [05 — Compute Pass Integration](05-compute-passes.md)
- [06 — Editor Integration](06-editor-integration.md)
- [Helio Radiant v2 — Migration & Backward Compatibility](07-migration.md)

---
# Radiant v2 — Final Revision

> **Note**: This is the third and final pass. It supersedes the first-pass (merged
> sub-specs) and second-pass (template abstraction) sections below. Where they
> conflict, this revision wins. It incorporates all feedback from the preceding
> design review and is the authoritative specification moving forward.

---

## S.1 Two-Tier Architecture: Fast Path vs Custom Path

The system has two tiers. Tier 1 is the common case and pays **zero** overhead.
Tier 2 is available for materials that need custom behavior and pays only what
it uses.

### S.1.1 Tier 1 — Fast Path (no custom eval functions)

90%+ of materials in any shipped title are simple variants of a built-in shading
model: different base color, roughness, metalness, normal map, etc. They never
need custom WGSL.

These materials are created by instantiating an engine-shipped **template**
(e.g., "pbr", "clear_coat", "skin", "glass") with different parameters:

```rust
// Tier 1 — no custom WGSL, no new templates, no eval point registration
let red_plastic = scene.create_material(
    pbr_template_id,    // built-in PBR template
    MaterialParams {
        base_color: [0.9, 0.1, 0.08],
        roughness: 0.85,
        metallic: 0.0,
    },
    textures: &[albedo_tex, rough_tex],
);
```

**Cost for Tier 1 materials:**

| Resource | Cost | Notes |
|---|---|---|
| Dispatch table entries | 0 added (shares built-in template) | Same entry as every other PBR material |
| Shader variants | 0 new compiled | Shares the single PBR variant |
| Per-pixel mask check in gbuffer | 0 cycles | Never-taken predicated branch |
| Per-pixel mask check in transparent pass | 0 cycles | early-Z kills fragment before shader |
| Per-pixel mask check in compute passes | 0 cycles | Tile is culled by material visibility hierarchy |

A Tier 1 material costs **exactly nothing** beyond the instance data it already
needs (transform, material ID, parameters). 10,000 Tier 1 materials cost the
same as 1 — all share the same template, the same dispatch entry, the same PSO.

### S.1.2 Tier 2 — Custom Path (user-defined eval functions)

For materials that genuinely need a different shading model (custom BRDF,
multi-pass evaluation, novel lighting), the user registers a new template:

```rust
// Tier 2 — new template with custom eval functions
let custom_brdf = engine.register_template(Template {
    name: "my_custom_brdf",
    targets: &["gbuffer"],                     // eval points this template uses
    eval_sources: Map { "gbuffer" => my_wgsl }, // custom WGSL
    param_schema: ParamSchema { /* ... */ },
});
```

**Cost for Tier 2 materials:**

| Resource | Cost | Notes |
|---|---|---|
| Dispatch table entries | +1 | One entry per registered template |
| Shader variants | +1 compiled | One variant per (template × eval point) |
| Per-pixel mask check | ~1 ALU per pixel per targeted pass | Only runs in passes the template targets |

A Tier 2 material pays for exactly what it uses. If it targets `["gbuffer"]`
only (same as Tier 1), the cost is identical to Tier 1 — the mask happens to
match, so the check behaves identically. If it targets `["gbuffer", "ssr"]`,
it pays the check only in the SSR pass, where it's a tiny fraction of the
overall SSR cost.

### S.1.3 Scaling Proof

| Scenario | Templates | Materials | Dispatch entries | Per-pixel cost |
|---|---|---|---|---|
| All PBR, one mesh | 1 | 1 | 1 | 0 (never-taken branch) |
| AAA game, all built-in templates | 30 | 10,000 | 30 | 0 for Tier 1, ~1 ALU for Tier 2 |
| Worst case: all custom | 500 | 10,000 | 500 | 1 indexed load + 1 ALU |
| Any scenario | N | M | N (not M) | Invariant with M |

The per-pixel cost depends only on the number of TEMPLATES visible on screen,
not the number of materials. Since any frame typically shows a small fraction
of all registered templates (you don't see 500 material types in one frame),
the practical cost is even lower than the worst case.

---

## S.2 Invariance of Per-Pixel Check Cost

The per-pixel check is:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
if (mask & (1u << THIS_BIT)) == 0u { discard; }
```

This is a single indexed load + bit test. The instruction stream is **identical**
regardless of whether the dispatch table has 1 entry or 10,000. The only
difference is the index value, which comes from instance data and varies per
pixel naturally. There is no loop, no `if` chain, no switch statement over
template types. The ALU cost is the same for every pixel in every scenario.

The dispatch table at 10,000 templates is 40 KB — well within L1 cache on any
modern GPU (AMD RDNA3: 32 KB L1 per CU × 60 CUs = 1,920 KB total L1; NVIDIA
Ada: 128 KB L1 per SM × 72 SMs = 9,216 KB total L1). The buffer is read-only,
uniform across all threads, and accessed coherently on the same cache line
within a warp — effectively zero latency.

---

## S.3 Material Visibility Hierarchy (Compute Passes)

For compute passes (SSR, SSS, post-process), where early-Z is unavailable and
thread divergence is expensive, a **material visibility mip chain** is built
during the gbuffer pass.

### S.3.1 Construction

One compute dispatch builds a pyramid from the `template_id_tex`:

```
Level 0 (1080p):     template_id per pixel (R32Uint, already written by gbuffer)
Level 1 (540p):      OR of eval_point_mask for each 2×2 block
Level 2 (270p):      OR of 4×4 blocks
...down to...
Level N (~64×64):    Entire-screen region bitmask
```

Each texel at level L stores the **union of all eval_point_masks** for its
region. If any pixel in the 4×4 block targets SSR, the block's texel has
SSR_BIT set.

Cost: one compute dispatch, ~0.05 ms at 1080p. Identical hardware path to
Hi-Z depth pyramid construction (which every engine already does).

### S.3.2 Usage in Compute Passes

Before dispatching per-pixel work, the compute pass checks the coarse level
that covers its tile:

```wgsl
@compute @workgroup_size(16, 16, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    // Coarse check: does this 16x16 tile have any pixel targeting SSR?
    let tile_id = id.xy >> 4;  // which 16x16 tile
    let coarse_mask = textureLoad(material_visibility_mip, tile_id, COARSE_LEVEL).r;
    
    if (coarse_mask & (1u << SSR_BIT)) == 0u {
        return;  // Entire workgroup exits — no divergence
    }
    
    // Fine check: per-pixel within the active tile
    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << SSR_BIT)) == 0u { return; }
    
    // ... actual SSR work ...
}
```

### S.3.3 Performance

| Scenario | Without hierarchy | With hierarchy |
|---|---|---|
| Large uniform region (e.g., sky = no SSR) | Every pixel checks mask | Entire tile culled atomically |
| Small SSR object in frame (e.g., glass sphere) | Pixels in glass area check + pass; all others check + fail | Only tiles covering the glass area run per-pixel |
| Dense scene with many SSR surfaces everywhere | All pixels check + pass | Tiles with SSR surfaces run per-pixel; tiles without exit early |

The hierarchy **cannot be worse** than per-pixel alone. At tile boundaries
where materials mix, it falls through to the per-pixel check — same cost as
without the hierarchy. On large uniform regions (most of the screen), it
eliminates the check entirely.

---

## S.4 Template Library Is the Shading Model Library

The engine ships ~30 built-in templates. These aren't "variations of PBR" —
they're **distinct shading archetypes**, each with the exact BRDF and pipeline
integration that archetype requires:

| Template | Eval points | BRDF/Model | Key features |
|---|---|---|---|
| `pbr` | gbuffer | Cook-Torrance GGX | Metallic workflow, IBL, clear coat optional |
| `skin` | gbuffer, sss | d-Lobe/s-Lobe + SSS | Dual-lobe specular, subsurface scattering, transmission |
| `hair` | gbuffer | d-Box / Kajiya-Kay | Anisotropic specular, directional AO, scattering |
| `fabric` | gbuffer | Microfiber + sheen | Sheen lobe, fuzz normal, cloth D/G |
| `clear_coat` | gbuffer | Dual-layer GGX | Base + clear coat, thickness-yellowing |
| `glass` | gbuffer, transparent | Fresnel + transmission | IOR, thin-film, tinted transmission |
| `water` | transparent, ssr | Gerstner + foam | Animated waves, caustics, foam, Fresnel |
| `eye` | gbuffer | Cornea/iris/sclera | Three-region shading, limbal ring |
| `translucent` | gbuffer, sss | Lambertian transmission | Thickness-based absorption, scattering |
| `iridescent` | gbuffer | Thin-film interference | Wavelength-shifting F0 |
| `velvet` | gbuffer | Inverted Gaussian | Back-scatter highlight, soft edge |
| ... | ... | ... | ... |

Each template is a complete, production-grade WGSL file (~200-500 lines)
authored by the engine team. Once written, it's shared by all materials
of that type in every project.

Users extending the engine can author their own templates (Tier 2). The
pipeline handles them identically — they get a dispatch table entry, a
compiled variant, and full evaluation point support.

---

## S.5 AAA Resolution and Frame Rate

**4K, 120 fps, no overhead:**

| Concern | Mitigation |
|---|---|
| 8M pixels × per-pixel check | Only runs for pixels that pass early-Z (fragment) or aren't culled by hierarchy (compute). 90%+ of pixels never reach it. |
| Wavefront divergence | The hierarchy culls at tile granularity (16×16). Divergence only occurs at material boundaries within a tile — typically < 5% of pixels. |
| Dispatch table storage | 4 bytes × templates. 30 built-in = 120 bytes (L1). 500 custom = 2 KB (L1). Negligible. |
| Template ID texture | 4 bytes × 8M = 32 MB at 4K. Already part of the gbuffer. |
| Mip chain construction | One compute dispatch, ~0.1 ms at 4K. Same cost as Hi-Z. |
| Editor complexity | Templates are authored once by engine team. Tier 1 users never see the WGSL — they pick a template and set parameters. |

**The system adds zero measurable cost to the common case (Tier 1, basic PBR)
and enables novel flexibility for the rare case (Tier 2, custom materials).**
This is the correct tradeoff for an engine where the number of materials is
unbounded and the editor drives material creation.

---

## S.6 Relationship to Previous Spec Sections

| Previous section | Status |
|---|---|
| First-pass (sub-specs 01-07) | Still accurate for implementation details of eval points, template composition, scene rebuild, etc. |
| Second-pass (template abstraction) | Superseded by S.1 (two-tier), S.3 (hierarchy), S.4 (template library) |
| This revision (S.1-S.6) | **Authoritative final design** |

---
---
> **Second-pass revision**: The following section replaces and refines several
> assumptions from the first-pass sections below. In particular, it introduces
> the **Template** abstraction layer â€” materials inherit dispatch properties
> from templates, making dispatch cost independent of material count â€” and
> provides a rigorous per-pixel check cost analysis.
# Radiant v2 — Second-Pass Revision

> **Note**: This document sits at the top of the merged spec. It replaces and
> refines several assumptions from the first pass. In particular, it introduces
> the **Template** abstraction layer and addresses the per-pixel check cost.

---

## Two-Layer Architecture: Templates and Materials

### Problem with First-Pass Design

The first-pass spec assumed that every material registers its own eval functions
and carries its own `eval_point_mask`. This scales poorly: if 10,000 materials
all use the same "PBR" shader with different parameters (color, roughness, etc.),
they would need 10,000 identical entries in the dispatch table and 10,000
identical masks. Worse, the per-pixel check would read a unique mask for each
material — no better than per-class dispatch.

### Solution: Templates as the Unit of Dispatch

Introduce a **Template** layer between the engine and materials:

```
Engine ships ~30 templates
       │
       │ Each template declares:
       │   • Which eval points it targets (mask)
       │   • WGSL eval functions for each target
       │   • Parameter schema (class_params + textures)
       │
       ▼
Users create Materials by instantiating a template
       │
       │ A material = template_id + parameter values + texture refs
       │ NO shader code, NO new eval points, NO new masks
       │
       ▼
Engine evaluates per-template, not per-material
       │
       │ 10,000 materials using 3 templates = 3 dispatch entries
       │ The per-pixel check reads the template mask, not the material mask
```

### How It Works

**Registration (at engine startup):**

```rust
// Engine ships 30 built-in templates
let pbr_template = engine.register_template(Template {
    name: "pbr",
    targets: &["gbuffer"],
    eval_sources: Map { "gbuffer" => pbr_gbuffer_wgsl },
    param_schema: ParamSchema { /* base_color, roughness, metallic, ... */ },
});

let glass_template = engine.register_template(Template {
    name: "glass",
    targets: &["gbuffer", "transparent"],
    eval_sources: Map {
        "gbuffer" => glass_gbuffer_wgsl,
        "transparent" => glass_transparent_wgsl,
    },
    param_schema: ParamSchema { /* tint, ior, ... */ },
});
```

**Material creation (at scene build time):**

```rust
// User picks a template and fills in parameters
let red_glass = scene.create_material(
    glass_template.id,       // template ID
    MaterialParams {
        color: [0.8, 0.1, 0.1],
        roughness: 0.02,
        ior: 1.5,
    },
    textures: &[albedo_tex, normal_tex],
);
// Result: a MaterialId. No shader compilation. No new dispatch entries.
// The eval_point_mask is INHERITED from glass_template.
```

### Pipeline Dispatch Changes

The instance data stores `template_id` instead of (or in addition to) `material_class`:

```rust
struct GpuInstanceData {
    // ... existing fields ...
    pub template_id: u32,      // indexes into template_dispatch[]
}
```

The template dispatch table is a tiny GPU buffer:

```rust
struct GpuTemplateDispatch {
    /// Bit N = 1 => this template targets evaluation point N
    pub template_id: u32,
}
```

Size: 4 bytes × ~30 templates = **120 bytes total**. Fits in a single cache line.

The per-pixel check reads from this table, not from per-material data:

```wgsl
// Fragment shader:
let mask = template_dispatches[input.template_id].eval_point_mask;
if (mask & (1u << THIS_BIT)) == 0u { discard; }
```

### Scaling Properties

| Scenario | Templates | Materials | Dispatch entries | Check cost |
|---|---|---|---|---|
| Tiny scene | 10 | 50 | 10 | 10 masks |
| AAA game | 30 | 10,000 | 30 | 30 masks |
| Worst case | 32 | unlimited | 32 | 32 masks |

The check cost is bounded by the number of TEMPLATES, not the number of materials.
Since the engine ships ~30 templates and third parties may add a handful more,
the dispatch table is never larger than ~64 entries. The check is a single
4-byte read from a table that fits in L1 cache.

### User-Defined Templates

Users aren't limited to the 30 built-in templates. They can define their own:

```rust
// Third party or advanced user creates a custom template
let my_template = engine.register_template(Template {
    name: "custom_iridescent",
    targets: &["gbuffer", "ssr", "post_process"],
    eval_sources: Map {
        "gbuffer" => my_gbuffer_wgsl,
        "ssr" => my_ssr_wgsl,
        "post_process" => my_post_wgsl,
    },
    param_schema: ParamSchema { /* ... */ },
});
```

Each custom template adds one entry to the dispatch table. Total templates across
all sources (engine + third party + user) should stay well under 64 — the u32
bitmask supports up to 32 eval points, so 32 dispatch entries is the practical
maximum without extending the mask to u64.

### Summary

| Aspect | First-pass (per-material) | Second-pass (per-template) |
|---|---|---|
| Who defines eval targets? | Every material | Every template |
| Who writes shader code? | Every material (WGSL) | Template author (once) |
| Dispatch table size | N materials × 4 bytes | ~30 templates × 4 bytes = 120 bytes |
| Per-pixel check reads | Unique mask per material | Shared mask per template |
| Scales to 10,000 materials | Poor (10,000 dispatch entries) | Excellent (still 30 entries) |
| User-defined materials | Write WGSL | Pick template + set params |

---

## Per-Pixel Check Cost Analysis

### The Concern

Every fragment shader in every pass executes:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_BIT)) == 0u { discard; }
```

If this runs for every pixel of every draw in every pass, doesn't it add up?

### Fragment Passes: The check is free

Fragment passes benefit from **early-Z culling**. The GPU's hierarchical Z-buffer
discards pixels that fail the depth test BEFORE the fragment shader runs.

**Flow for a fully opaque pixel in the transparent pass:**

1. Rasterizer emits pixel at (x, y) with depth z
2. Early-Z test: compare z against depth buffer → **FAIL** (opaque geometry wrote z earlier)
3. Fragment shader NEVER RUNS
4. The `eval_point_mask` check is never reached

**Result**: For opaque pixels in the transparent pass, the check costs exactly
0 cycles. The pixel is killed by early-Z before any shader code executes.

**Flow for a transparent pixel in the transparent pass:**

1. Rasterizer emits pixel at (x, y) with depth z
2. Early-Z test: compare z against depth buffer → **PASS** (no opaque geometry here)
3. Fragment shader runs
4. Reads `eval_point_mask` from flat-interpolated input (already in register)
5. Tests bit: `(mask & TRANSPARENT_BIT) != 0` → passes
6. Continues to evaluate the transparent function

**Flow for the gbuffer pass (any material):**

1. Rasterizer emits pixel
2. Early-Z test: gbuffer writes depth, so the first geometry to write wins
3. Fragment shader runs for every pixel that passes early-Z
4. Reads `eval_point_mask` (from VertexOutput, already in register)
5. Tests bit: `(mask & GBUFFER_BIT) != 0` → always true for gbuffer-targeting materials
6. This is a **never-taken branch** — the condition is always false for the common case
7. Modern GPUs predicated branches that are never taken cost zero (the instruction is
   issued but the result is discarded — no pipeline stall)

### Compute Passes: The check is measurable but trivial

Compute passes have no early-Z. Every pixel in the dispatch grid runs the thread.
The check pattern is:

```wgsl
if (mask & (1u << SSR_BIT)) == 0u { return; }
```

**Cost breakdown for one pixel that does NOT target SSR:**

| Operation | Cost |
|---|---|
| Texture load (material_class) | ~20-40 cycles (L2 hit) |
| Storage buffer load (dispatch table) | ~20-40 cycles (L2 hit, tiny table) |
| Bit test + conditional return | ~1 cycle (predicated) |
| **Total** | **~40-80 cycles** |

Vs. running the full SSR shader on that pixel: **~500-2000+ cycles** (ray marching).

The check saves 10-50× more than it costs for non-targeting pixels.

For pixels that DO target SSR:
| Operation | Cost |
|---|---|
| Check overhead | ~40-80 cycles |
| SSR ray march | ~500-2000 cycles |
| **Check as fraction of total** | **~4-8%** |

### Real-world Scene Analysis

Consider a scene with:
- 1 glass sphere (2% of screen)
- 1 water surface (5% of screen)
- Everything else opaque PBR (93% of screen)

**Transparent pass (fragment):**
- 93% of pixels → early-Z kills before fragment shader → **0 cycles for check**
- 7% of pixels → fragment runs, check passes → continues to transparent eval

**SSR pass (compute):**
- 100% of pixels → compute thread runs
- 98% of pixels (no SSR material) → check returns → **40-80 cycles**
- 2% of pixels (glass has SSR) → check passes → SSR ray march

The check cost is dominated by the compute pass. For a 1080p frame (2M pixels),
that's 2M × 60 cycles = **120M cycles** ≈ **2ms at 60 GHz** ≈ **~5% of frame time**
for the SSR pass — but only if SSR is enabled and the scene has no SSR materials
anywhere. If even one material targets SSR, the check was necessary anyway to
dispatch correctly.

### Conclusion

| Pass type | Check cost | Notes |
|---|---|---|
| Fragment (opaque material in gbuffer) | 0 | Never-taken branch, predicated |
| Fragment (opaque pixel in transparent pass) | 0 | Early-Z kills before fragment shader |
| Fragment (transparent material in transparent pass) | 1 ALU | Mask in register, bit test |
| Compute (non-targeting pixel) | ~40-80 cycles | Texture + buffer read + return |
| Compute (targeting pixel) | ~40-80 cycles overhead vs full eval | < 10% of total |

**The check is free in fragment passes and a trivial fraction of compute passes.**
No optimization needed.



> **Status**: Draft  
> **Part of**: Helio Radiant v2 Material Pipeline Specification  
> **Supersedes**: `RadiantTemplateRegistry` per-pass ownership, ad-hoc `material_class_ranges`, static `GBufferPass::template_registry`  
> **Design goal**: A centralized registry where every render pass declares its material evaluation points (named injection sites), enabling zero-overhead per-pixel dispatch, third-party extensibility, and stable compile-time indexing.

---

## 1. Core Concept

An **eval point** (evaluation point) is a named shader-injection site inside a render pass. Each point defines the exact WGSL entry-point signature that a material must provide, the base shader that surrounds it (bindings, vertex plumbing, fixed-function state), and the GPU pipeline configuration (blend mode, depth state, threading model).

The **EvalPointRegistry** is the single global authority for all eval points across all render passes in the graph. It is built once during graph construction (after `graph.lock()`), frozen before any material registration, and treated as immutable for the lifetime of the render graph.

---

## 2. `EvalPoint` Struct

```rust
/// A single material evaluation point declared by a render pass.
///
/// Each eval point represents one shader-injection site:
///   - "gbuffer"        → material's `eval_gbuffer(...)` is spliced into the GBuffer fragment shader
///   - "transparent"    → material's `eval_transparent(...)` is spliced into the transparent fragment shader
///   - "shadow"         → material's `eval_shadow(...)` is spliced into the shadow depth fragment shader
///   - etc.
///
/// The index is assigned at registration time and is stable for the lifetime
/// of the render graph. It is used as a bit position in the u32 eval-point mask.
#[derive(Clone, Debug)]
pub struct EvalPoint {
    // ── Identity ────────────────────────────────────────────────────────

    /// Human-readable name, e.g. "gbuffer", "transparent", "ssr".
    ///
    /// Must be unique across all registered eval points. Convention is
    /// snake_case, matching the entry function name after the `eval_` prefix.
    pub name: &'static str,

    /// Fixed index assigned by the registry at registration time.
    ///
    /// Bit position in the eval-point mask: `mask & (1 << index)`.
    /// Guaranteed to be in `0..=31` (u32 bitmask limit).
    /// Stable for the entire lifetime of the owning render graph.
    pub index: u32,

    // ── Entry function contract ─────────────────────────────────────────

    /// Full WGSL function signature that materials must provide.
    ///
    /// This is the exact declaration text used for composition and validation:
    ///
    /// ```wgsl
    /// fn eval_ssr(material_id: u32, world_pos: vec3f, normal: vec3f,
    ///             roughness: f32, f0: vec3f) -> SsrParams
    /// ```
    ///
    /// The parser extracts `entry_fn_name` from this string for the composition
    /// algorithm (see §5.2 of the parent spec).
    pub entry_fn_signature: &'static str,

    /// Entry function name extracted from `entry_fn_signature` (e.g. `"eval_ssr"`).
    ///
    /// Used by `compose_fn_override()` to locate and replace the function body
    /// in the base shader. Derived automatically at registration time via
    /// a simple parser that extracts the identifier after `fn `.
    pub entry_fn_name: &'static str,

    // ── GPU pipeline definition ─────────────────────────────────────────

    /// Base WGSL source that defines the pass's fixed plumbing.
    ///
    /// Contains:
    ///   1. All `@group(N) @binding(M)` decorations the pass provides
    ///   2. The vertex shader (for Fragment passes) or compute entry (for Compute passes)
    ///   3. A default implementation of the eval function
    ///   4. `// RADIANT_OVERRIDE_START` / `// RADIANT_OVERRIDE_END` markers
    ///
    /// Stored as a `&'static str` (embedded via `include_str!` at compile time).
    /// The shader module is NOT compiled here — it is compiled lazily per
    /// `ShaderVariantKey` when a material targets this point.
    ///
    /// Build-time validation: the registry verifies that the string is non-empty
    /// and contains the entry function name. Full WGSL parsing is deferred to
    /// the shader compiler pipeline.
    pub base_shader: &'static str,

    /// Pipeline layout shared by all render-pipeline variants of this eval point.
    ///
    /// Created once during pass initialization from the bind group layouts declared
    /// in `base_shader`. Stored as `None` for compute-only eval points.
    pub pipeline_layout: Option<wgpu::PipelineLayout>,

    /// Pipeline layout shared by all compute-pipeline variants of this eval point.
    ///
    /// Created once during pass initialization. Stored as `None` for fragment-only
    /// eval points.
    pub compute_layout: Option<wgpu::ComputePipelineLayout>,

    // ── Fixed-function state ────────────────────────────────────────────

    /// Blend mode for fragment passes.
    ///
    /// One of: `Opaque`, `AlphaBlend`, `Add`, `Multiply`, `Custom(...)`.
    /// Maps to wgpu's `BlendState` and `ColorTargetState`.
    /// For compute passes this field is ignored (should be `BlendMode::None`).
    pub blend_mode: BlendMode,

    /// Depth/stencil state for fragment passes.
    ///
    /// Controls depth write, depth comparison function, and stencil operations.
    /// - `Some(DepthStencilState { depth_write: true, depth_compare: Less, .. })` — gbuffer writes depth
    /// - `Some(DepthStencilState { depth_write: false, depth_compare: Equal, .. })` — transparent reads only
    /// - `None` — no depth attachment (compute or color-only passes)
    pub depth_state: Option<DepthStencilState>,

    /// Threading model — determines whether the pass uses rasterization or compute dispatch.
    pub threading: ThreadingModel,
}
```

### 2.1 `BlendMode`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// No blending. Overwrites the render target.
    Opaque,
    /// Source-over alpha blending: `src.alpha * src.rgb + (1 - src.alpha) * dst.rgb`.
    AlphaBlend,
    /// Additive blending: `src.rgb + dst.rgb`.
    Add,
    /// Multiplicative blending: `src.rgb * dst.rgb`.
    Multiply,
    /// Custom blend state for special cases (e.g. dual-source blending).
    Custom(wgpu::BlendState),
}
```

### 2.2 `DepthStencilState`

```rust
#[derive(Clone, Debug)]
pub struct DepthStencilState {
    /// Format of the depth/stencil attachment.
    pub format: wgpu::TextureFormat,
    /// Whether the fragment shader writes to the depth buffer.
    pub depth_write: bool,
    /// Depth comparison function (e.g. `Less`, `LessEqual`, `Always`).
    pub depth_compare: wgpu::CompareFunction,
    /// Optional stencil state (default: disabled).
    pub stencil: Option<wgpu::StencilState>,
}
```

### 2.3 `ThreadingModel`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadingModel {
    /// Traditional rasterized pass. The material's eval function runs in the
    /// fragment shader. Mesh data (vertices, instance data with eval_point_mask)
    /// provides per-pixel dispatch.
    Fragment,
    /// Full-screen compute dispatch. No rasterization; the thread reads the
    /// material class from a per-pixel texture and the eval_point_mask from a
    /// GPU dispatch table.
    ///
    /// `group_size` is the workgroup size in (X, Y, Z) dimensions.
    /// Typical values: `(8, 8, 1)` for screen-space effects, `(64, 1, 1)` for
    /// linear dispatches.
    Compute {
        group_size: (u32, u32, u32),
    },
}
```

### 2.4 `EvalPoint` Construction Example

```rust
EvalPoint {
    name: "gbuffer",
    index: 0,  // assigned by registry
    entry_fn_signature: "fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData",
    entry_fn_name: "eval_gbuffer",  // derived from signature
    base_shader: include_str!("../shaders/gbuffer_base.wgsl"),
    pipeline_layout: Some(gbuffer_pipeline_layout),
    compute_layout: None,
    blend_mode: BlendMode::Opaque,
    depth_state: Some(DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write: true,
        depth_compare: wgpu::CompareFunction::Less,
        stencil: None,
    }),
    threading: ThreadingModel::Fragment,
}
```

---

## 3. `EvalPointRegistry` Struct

```rust
/// Central registry of all evaluation points in the current render graph.
///
/// # Lifecycle
///
/// 1. **Created** during `RenderGraph::lock()` in `build_default_graph_internal`.
/// 2. **Populated** by calling `register_eval_points(&mut self)` on every pass in the graph.
/// 3. **Frozen** — after construction, no new points may be added.
/// 4. **Read** by `renderer.eval_points()` and `renderer.eval_point_index()` during
///    material registration and shader variant compilation.
///
/// # Thread Safety
///
/// After freezing, the registry is `Send + Sync`. Reads from `get()` and `index()`
/// are lock-free (backed by `Arc<RegistryInner>`).
#[derive(Clone)]
pub struct EvalPointRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    /// Points stored in registration order. Index in this vec equals the
    /// `EvalPoint.index` field. Immutable after construction.
    points: Vec<EvalPoint>,

    /// Fast name-to-index lookup. Immutable after construction.
    name_map: HashMap<&'static str, u32>,

    /// Whether the registry is sealed. If true, `register()` panics.
    frozen: bool,

    /// Maximum number of eval points the registry was configured for.
    /// Defaults to 32 (u32 bitmask limit). Can be lowered to 8 or 16 via
    /// `EvalPointRegistry::with_capacity()` for graphs that don't need
    /// the full 32-point range.
    capacity: u32,
}
```

### 3.1 Registration Flow

```rust
impl EvalPointRegistry {
    /// Create a new empty registry with the default capacity of 32.
    pub fn new() -> Self { ... }

    /// Create a new empty registry with the specified capacity (max 32).
    pub fn with_capacity(capacity: u32) -> Self {
        assert!(capacity <= 32, "eval point capacity cannot exceed 32 (u32 bitmask limit)");
        Self {
            inner: Arc::new(RegistryInner {
                points: Vec::with_capacity(capacity as usize),
                name_map: HashMap::with_capacity(capacity as usize),
                frozen: false,
                capacity,
            }),
        }
    }

    /// Register an eval point. Assigns it the next available index.
    ///
    /// # Panics
    ///
    /// - If the registry is frozen (call to `freeze()` already happened).
    /// - If `point.name` is already registered (duplicate detection).
    /// - If the registry has reached its capacity limit.
    /// - If `point.base_shader` is empty (post-condition violation).
    /// - If `point.entry_fn_signature` does not contain a valid `fn <name>(...)`
    ///   pattern (parse failure).
    pub fn register(&mut self, mut point: EvalPoint) -> u32 {
        let inner = Arc::make_mut(&mut self.inner);

        assert!(!inner.frozen, "cannot register eval point after registry is frozen");
        assert!(
            !inner.name_map.contains_key(point.name),
            "duplicate eval point name: '{}'",
            point.name
        );
        assert!(
            (inner.points.len() as u32) < inner.capacity,
            "eval point capacity reached ({}) — cannot register '{}'",
            inner.capacity,
            point.name,
        );
        assert!(!point.base_shader.is_empty(), "base_shader for '{}' is empty", point.name);

        // Extract the entry function name from the signature
        let fn_name = extract_fn_name(point.entry_fn_signature)
            .expect("entry_fn_signature must contain 'fn <name>(...)");
        point.entry_fn_name = fn_name;

        // Assign the fixed index
        let index = inner.points.len() as u32;
        point.index = index;

        inner.name_map.insert(point.name, index);
        inner.points.push(point);

        index
    }

    /// Seal the registry. Future `register()` calls will panic.
    ///
    /// Called once after all passes have registered their eval points.
    /// After freezing, the registry can be shared immutably across threads.
    pub fn freeze(&mut self) {
        let inner = Arc::make_mut(&mut self.inner);
        inner.frozen = true;
    }
}
```

### 3.2 Lookup Methods

```rust
impl EvalPointRegistry {
    /// Returns a reference to all eval points, in registration order.
    ///
    /// The index in the slice equals `point.index`.
    /// This is the primary lookup path for material registration (iterating
    /// all points to compose and compile per-variant shaders).
    pub fn points(&self) -> &[EvalPoint] {
        &self.inner.points
    }

    /// Look up an eval point by name. Returns `None` if not found.
    ///
    /// Used during material registration to validate that the requested
    /// eval points exist:
    ///
    /// ```rust
    /// for target in &material.targets {
    ///     let point = registry.get(target)
    ///         .ok_or_else(|| format!("unknown eval point '{}'", target))?;
    ///     // ... compose + compile ...
    /// }
    /// ```
    pub fn get(&self, name: &str) -> Option<&EvalPoint> {
        self.inner.name_map.get(name).and_then(|&idx| self.inner.points.get(idx as usize))
    }

    /// Fast name-to-index lookup. Returns `None` if not found.
    ///
    /// This is the primary GPU-mask-building path:
    ///
    /// ```rust
    /// let mut mask = 0u32;
    /// for target in &material.targets {
    ///     if let Some(idx) = registry.index(target) {
    ///         mask |= 1 << idx;
    ///     }
    /// }
    /// ```
    pub fn index(&self, name: &str) -> Option<u32> {
        self.inner.name_map.get(name).copied()
    }

    /// Returns the number of registered eval points.
    pub fn count(&self) -> u32 {
        self.inner.points.len() as u32
    }

    /// Returns the configured capacity of the registry.
    pub fn capacity(&self) -> u32 {
        self.inner.capacity
    }

    /// Returns true if this registry is frozen.
    pub fn is_frozen(&self) -> bool {
        self.inner.frozen
    }

    /// Iterates all eval points, yielding `(index, &EvalPoint)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &EvalPoint)> + '_ {
        self.inner.points.iter().enumerate().map(|(i, p)| (i as u32, p))
    }
}
```

### 3.3 Internal Helper: `extract_fn_name`

```rust
/// Extract the function name from a WGSL function signature.
///
/// Input:  `"fn eval_gbuffer(material_id: u32, ...) -> SurfaceData"`
/// Output: `"eval_gbuffer"`
///
/// Panics if the signature doesn't contain a valid `fn <name>(` pattern.
fn extract_fn_name(signature: &str) -> Option<&str> {
    let sig = signature.trim();
    // Expect: "fn <name>("
    let rest = sig.strip_prefix("fn ")?;
    let paren = rest.find('(')?;
    Some(rest[..paren].trim())
}
```

---

## 4. Pass Registration

### 4.1 The `RenderPass::register_eval_points` Trait Method

Every render pass that evaluates materials **must** implement this method. It is the sole mechanism by which eval points enter the registry.

```rust
/// Trait that every material-evaluating render pass must implement.
///
/// Called during graph construction (after `graph.lock()`) to register
/// the pass's injection sites into the global `EvalPointRegistry`.
///
/// # Contract
///
/// - Must call `registry.register(...)` for each eval point the pass provides.
/// - Must not mutate any pass-internal state (the registry is the only mutable
///   reference available).
/// - Must be idempotent in practice (though the registry detects duplicates
///   and panics, so double-registration is a hard error).
/// - Must not capture the registry reference beyond the method call.
///
/// # Performance
///
/// Called exactly once during graph construction. The implementation should
/// be O(N) where N is the number of eval points the pass declares (typically 1-2).
pub fn register_eval_points(&self, registry: &mut EvalPointRegistry);
```

### 4.2 Default Implementation

Passes that do not evaluate materials (e.g. a pure post-process blur, a debug overlay) return the default no-op:

```rust
// Provided as a default on the trait so non-material passes opt out.
fn register_eval_points(&self, _registry: &mut EvalPointRegistry) {}
```

### 4.3 When `register_eval_points` Is Called

The call sequence during `build_default_graph_internal`:

```
build_default_graph_internal()
  │
  ├── 1. Create empty RenderGraph
  │
  ├── 2. Instantiate all passes:
  │       let gbuffer = GBufferPass::new(device, config);
  │       let transparent = TransparentPass::new(device, config);
  │       let shadow = ShadowPass::new(device, config);
  │       // ...
  │
  ├── 3. Add passes to graph:
  │       graph.add_pass(Box::new(gbuffer));
  │       graph.add_pass(Box::new(transparent));
  │       graph.add_pass(Box::new(shadow));
  │       // ...
  │
  ├── 4. Lock the graph:
  │       graph.lock(width, height);
  │       // At this point the graph resolves resource dependencies,
  │       // allocates inter-pass textures, and runs resize hooks.
  │
  ├── 5. Collect eval points from all passes:
  │       let mut registry = EvalPointRegistry::new();
  │       for pass in graph.passes() {
  │           pass.register_eval_points(&mut registry);
  │       }
  │       registry.freeze();
  │
  └── 6. Store registry on the renderer:
          renderer.set_eval_registry(registry);
```

**Why after `graph.lock()`?** Several passes create their pipeline layouts (`wgpu::PipelineLayout`) during `on_resize()` or during a post-lock initialization step, because layouts depend on the final resolution and texture formats. Since `EvalPoint` stores `pipeline_layout` and `compute_layout` fields, these must be available before the eval point is registered. Locking the graph first ensures all size-dependent resources exist.

### 4.4 Graph Builder Integration

```rust
impl RenderGraph {
    /// Collect all eval points from all registered passes into a single registry.
    ///
    /// Called once after `lock()`. The resulting registry is frozen and stored
    /// on the renderer for the duration of the graph's lifetime.
    pub fn collect_eval_points(&self) -> EvalPointRegistry {
        let mut registry = EvalPointRegistry::new();
        for pass in self.passes() {
            pass.register_eval_points(&mut registry);
        }
        registry.freeze();
        registry
    }
}
```

### 4.5 Pass Implementation Example

```rust
impl RenderPass for GBufferPass {
    fn name(&self) -> &'static str {
        "GBufferPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "gbuffer",
            index: 0,                              // ignored — assigned by registry
            entry_fn_signature: "fn eval_gbuffer(\
                material_id: u32, \
                world_pos: vec3f, \
                normal: vec3f, \
                uv: vec2f\
            ) -> SurfaceData",
            entry_fn_name: "",                      // ignored — derived from signature
            base_shader: include_str!("../shaders/gbuffer_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::Opaque,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }

    // ... execute(), prepare(), etc. ...
}
```

---

## 5. Built-in Eval Points

The engine ships with the following eval points. They are registered in this exact order, which determines their fixed indices.

### 5.1 Full Table

| Index | Name | Pass | Entry Function Signature | Blend | Depth State | Threading |
|-------|------|------|--------------------------|-------|-------------|-----------|
| 0 | `gbuffer` | `GBufferPass` | `fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData` | Opaque | Write (Depth32Float, Less) | Fragment |
| 1 | `transparent` | `TransparentPass` | `fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f` | AlphaBlend | Read-only (Depth32Float, Equal) | Fragment |
| 2 | `shadow` | `ShadowPass` | `fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool` | None | Write (Depth32Float, Less) | Fragment |
| 3 | `depth_prepass` | `DepthPrepass` | `fn eval_depth(world_pos: vec3f) -> f32` | None | Write (Depth32Float, Less) | Fragment |
| 4 | `ssr` | `SsrPass` | `fn eval_ssr(material_id: u32, world_pos: vec3f, normal: vec3f, roughness: f32, f0: vec3f) -> SsrParams` | N/A | N/A | Compute `(8, 8, 1)` |
| 5 | `sss` | `SssBlurPass` | `fn eval_sss(material_id: u32) -> SssParams` | N/A | N/A | Compute `(8, 8, 1)` |
| 6 | `decal` | `DecalPass` | `fn eval_decal(material_id: u32, uv: vec2f) -> DecalOutput` | N/A | N/A | Compute `(8, 8, 1)` |
| 7 | `post_process` | `PostProcessPass` | `fn eval_post(input: vec4f) -> vec4f` | N/A | N/A | Compute `(16, 16, 1)` |
| 8 | `velocity` | `VelocityPass` | `fn eval_velocity(world_pos_prev: vec3f) -> vec2f` | None | N/A | Fragment |

### 5.2 Per-Point Detail

#### 5.2.1 `gbuffer` (Index 0)

- **Pass**: `GBufferPass` (`helio-pass-gbuffer`)
- **Base shader**: `gbuffer_base.wgsl` — includes camera UBO, globals, instance buffer, material buffer, albedo/normal/ORM/emissive texture arrays, Template ID texture write
- **Entry function**: `fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData`
- **Returns**: `SurfaceData` — a struct containing `albedo: vec4f`, `normal: vec3f`, `roughness: f32`, `metallic: f32`, `specular_f0: vec3f`, `emissive: vec3f`, `subsurface_color: vec3f`, `flags: u32`
- **Blend**: `Opaque` — writes directly, no blending
- **Depth**: Write, `CompareFunction::Less`, `Depth32Float`
- **Threading**: `Fragment` — rasterized with multi-draw indirect
- **Dispatch mask path**: Embedded in `GpuInstanceData.eval_point_mask`, passed as flat-interpolated vertex attribute. Fragment shader checks `mask & (1 << 0) != 0`, discards if false.
- **Notes**: This is the primary surface pass. Every opaque PBR material targets this by default (backward compatibility shim). Also writes the `template_id_tex` (R32Uint) at target index 4 for downstream compute passes.

#### 5.2.2 `transparent` (Index 1)

- **Pass**: `TransparentPass` (`helio-pass-transparent`)
- **Base shader**: `transparent_base.wgsl` — camera UBO, globals, instance buffer, material buffer, color target
- **Entry function**: `fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f`
- **Returns**: `vec4f` — RGBA color with pre-multiplied alpha
- **Blend**: `AlphaBlend` — `SrcAlpha / OneMinusSrcAlpha`
- **Depth**: Read-only, `CompareFunction::Equal` (opaque geometry already wrote depth), depth writes disabled
- **Threading**: `Fragment`
- **Dispatch mask path**: Same as gbuffer — `eval_point_mask` in instance data
- **Notes**: Renders after the gbuffer pass. Uses the same geometry but only evaluates fragments for materials whose mask includes bit 1. Depth is set to Equal so only surfaces at the correct depth are shaded (handles ordering via painter's algorithm + depth buffer from gbuffer).

#### 5.2.3 `shadow` (Index 2)

- **Pass**: `ShadowPass` (`helio-pass-shadow`)
- **Base shader**: `shadow_base.wgsl` — light matrices, shadow map atlas bindings
- **Entry function**: `fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool`
- **Returns**: `bool` — `true` = casts shadow (default), `false` = alpha-tested / discarded
- **Blend**: None — writes to a depth-only shadow atlas
- **Depth**: Write, `CompareFunction::Less`, `Depth32Float` (or `Depth24Plus` for the shadow format)
- **Threading**: `Fragment`
- **Dispatch mask path**: Instance data mask
- **Notes**: Used for alpha-tested shadow casting (foliage, chain-link fences). Materials that don't target `"shadow"` still write depth (the `discard` path), which is correct for shadow occlusion. Only materials that need alpha-testing (or custom shadow behavior) provide a custom `eval_shadow`.

#### 5.2.4 `depth_prepass` (Index 3)

- **Pass**: `DepthPrepass` (`helio-pass-depth-prepass`)
- **Base shader**: `depth_base.wgsl` — camera UBO, instance buffer with minimal vertex output (position only)
- **Entry function**: `fn eval_depth(world_pos: vec3f) -> f32`
- **Returns**: `f32` — the modified depth value (default: returns `world_pos` z in clip space; used for parallax mapping, procedural displacement)
- **Blend**: None
- **Depth**: Write, `CompareFunction::Less`, `Depth32Float`
- **Threading**: `Fragment`
- **Dispatch mask path**: Instance data mask
- **Notes**: Runs before the gbuffer pass to establish a depth buffer for early-Z culling. Typically no material provides a custom `eval_depth` (they let the default pass through). Used by materials with parallax occlusion mapping or vertex displacement to write correct depth early.

#### 5.2.5 `ssr` (Index 4)

- **Pass**: `SsrPass` (`helio-pass-ssr`)
- **Base shader**: `ssr_base.wgsl` — gbuffer textures (albedo, normal, ORM, depth), material dispatch table, SSR constants
- **Entry function**: `fn eval_ssr(material_id: u32, world_pos: vec3f, normal: vec3f, roughness: f32, f0: vec3f) -> SsrParams`
- **Returns**: `SsrParams` — a struct containing `reflectivity: f32`, `ray_length: f32`, `roughness_scale: f32`, `enable_ssr: u32`
- **Blend**: N/A
- **Depth**: N/A
- **Threading**: `Compute` with workgroup size `(8, 8, 1)`
- **Dispatch mask path**: `template_id_tex` (R32Uint) + GPU dispatch table (`GpuTemplateDispatch[]`). Thread reads material class, looks up mask, checks bit 4.
- **Notes**: Screen-space reflections. Materials default to full SSR. A rough material (e.g. cloth) can override to return `enable_ssr = 0` to skip SSR for its pixels. A mirror material can extend ray length.

#### 5.2.6 `sss` (Index 5)

- **Pass**: `SssBlurPass` (`helio-pass-**sss** — subsurface scattering blur pass)
- **Base shader**: `sss_base.wgsl` — gbuffer textures, SSS blur kernels, material dispatch table
- **Entry function**: `fn eval_sss(material_id: u32) -> SssParams`
- **Returns**: `SssParams` — a struct containing `subsurface_color: vec3f`, `radius: f32`, `strength: f32`, `enable_sss: u32`
- **Blend**: N/A
- **Depth**: N/A
- **Threading**: `Compute` with workgroup size `(8, 8, 1)`
- **Dispatch mask path**: Template ID texture + GPU dispatch table
- **Notes**: Subsurface scattering blur. A skin material provides custom SSS parameters (warm subsurface color, wider radius). A plastic material provides no SSS (or SSS with zero radius). The pass performs a separable blur using the parameters.

#### 5.2.7 `decal` (Index 6)

- **Pass**: `DecalPass` (`helio-pass-decal`)
- **Base shader**: `decal_base.wgsl` — decal projection matrices, gbuffer textures, material dispatch table
- **Entry function**: `fn eval_decal(material_id: u32, uv: vec2f) -> DecalOutput`
- **Returns**: `DecalOutput` — a struct containing `color: vec4f`, `normal: vec3f`, `roughness: f32`, `metallic: f32`, `blend: u32`
- **Blend**: N/A (writes directly to gbuffer overlay targets or modifies gbuffer values in-place)
- **Depth**: N/A
- **Threading**: `Compute` with workgroup size `(8, 8, 1)`
- **Dispatch mask path**: Template ID texture + GPU dispatch table
- **Notes**: Decal projection onto gbuffer surfaces. Each decal is a "material" that targets this eval point. The pass processes all decals in a compute dispatch, projecting their textures onto the gbuffer at the correct UVs (derived from the decal's projection matrix stored in material parameters).

#### 5.2.8 `post_process` (Index 7)

- **Pass**: `PostProcessPass` (`helio-pass-postprocess`)
- **Base shader**: `post_base.wgsl` — input color texture, tone-mapping parameters, material dispatch table
- **Entry function**: `fn eval_post(input: vec4f) -> vec4f`
- **Returns**: `vec4f` — the post-processed color
- **Blend**: N/A
- **Depth**: N/A
- **Threading**: `Compute` with workgroup size `(16, 16, 1)` (larger group for better occupancy on full-res dispatch)
- **Dispatch mask path**: Template ID texture + GPU dispatch table. Note: the entire screen dispatches; the mask check determines whether to apply the custom post effect or the identity pass-through.
- **Notes**: Per-pixel material post-processing. A material can inject a custom color grade, stylized effect, or blend pass. Default is identity (`return input`). Note that `input: vec4f` is the scene color at the pixel — not a material parameter. Material ID is implicit via the Template ID texture.

#### 5.2.9 `velocity` (Index 8)

- **Pass**: `VelocityPass` (`helio-pass-velocity` — may be part of an existing pass or a separate crate)
- **Base shader**: `velocity_base.wgsl` — camera UBO with previous-frame matrices, instance buffer
- **Entry function**: `fn eval_velocity(world_pos_prev: vec3f) -> vec2f`
- **Returns**: `vec2f` — 2D velocity vector in screen-space UV space (for TAA, motion blur)
- **Blend**: None
- **Depth**: N/A (writes to a Rg16Float velocity texture)
- **Threading**: `Fragment`
- **Dispatch mask path**: Instance data mask
- **Notes**: Writes per-pixel screen-space velocity. The default implementation computes `screen_pos_current - screen_pos_previous` from the previous-frame world position (reprojected). A custom implementation can override this for animated meshes, skinned vertices, or particle systems where the previous-frame position is known from material parameters.

### 5.3 Registration Order Guarantee

Built-in eval points are registered in the order shown above, producing the indices 0–8. This ordering matters for two reasons:

1. **Bitmask stability**: Index 0 (`gbuffer`) is always bit 0. Materials compiled with `mask = 1 << 0` always target gbuffer. This allows baked shaders to hardcode bit positions for the most common cases.
2. **Backward compatibility**: The `legacy` registration path (`register_partial_str`) implicitly sets `targets = &["gbuffer"]`, producing `mask = 1 << 0`. This never needs to change.

---

## 6. Third-Party Extension

### 6.1 Pattern

Any crate outside the engine core can add new eval points by:

1. Implementing `RenderPass` for a custom pass
2. Implementing `register_eval_points` on that pass
3. Adding the pass to the render graph before `graph.lock()`

No engine core modification is required. The `EvalPointRegistry` is a plain `Vec` + `HashMap` — it has no knowledge of which crate registered which point.

### 6.2 Example: Motion Blur Pass

```rust
// crates/subsystems/my-motion-blur/src/lib.rs

use helio_core::{
    EvalPoint, EvalPointRegistry, RenderPass, PassContext,
    PrepareContext, ThreadingModel, BlendMode, DepthStencilState, Result,
};

pub struct MotionBlurPass {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::ComputePipelineLayout,
    settings: MotionBlurSettings,
}

impl RenderPass for MotionBlurPass {
    fn name(&self) -> &'static str {
        "MotionBlurPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "motion_blur_velocity",
            index: 0,  // assigned by registry
            entry_fn_signature: "fn eval_motion_blur_velocity(\
                world_pos_prev: vec3f\
            ) -> vec2f",
            entry_fn_name: "",  // derived
            base_shader: include_str!("shaders/motion_blur_base.wgsl"),
            pipeline_layout: None,
            compute_layout: Some(self.layout.clone()),
            blend_mode: BlendMode::None,
            depth_state: None,
            threading: ThreadingModel::Compute { group_size: (8, 8, 1) },
        });
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("depth");
        builder.read("material_class");
        builder.read("gpu_template_dispatches");
        builder.write_color("motion_blur_velocity",
            wgpu::TextureFormat::Rg16Float,
            ResSize::Internal,
        );
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        // Compute dispatch: per-pixel velocity read from Template ID texture
        let mut pass = ctx.begin_compute_pass("MotionBlur");
        pass.set_pipeline(&self.pipeline);
        // Dispatch over screen tiles
        let (w, h) = ctx.screen_size();
        pass.dispatch_workgroups(
            (w + 7) / 8,
            (h + 7) / 8,
            1,
        );
        Ok(())
    }
}
```

### 6.3 Third-Party Material Registration

A third-party material targeting this new eval point:

```rust
renderer.register_material(
    "custom_motion_blur_object",
    custom_motion_blur_wgsl,    // contains fn eval_motion_blur_velocity(...)
    &["gbuffer", "motion_blur_velocity"],  // standard gbuffer + new custom point
);
```

### 6.4 Index Assignment for Third-Party Points

Third-party eval points receive indices **after** all built-in points, in registration order. If the engine ships with 9 built-in points (indices 0–8), the first third-party point registered gets index 9, the next gets index 10, etc., up to the capacity limit (default 32).

### 6.5 Why No Engine Core Modification?

The `RenderGraph::collect_eval_points` method iterates `Box<dyn RenderPass>` trait objects. Since `RenderPass` is a public trait in `helio-core`, any crate can implement it and be added to the graph. The registry has no whitelist, no hardcoded names, and no engine-side validation beyond uniqueness and capacity.

This is by design — it mirrors the plugin architecture of Pulsar (see `PLUGIN_SYSTEM.md`: plugins implement traits, engine iterates trait objects).

---

## 7. Eval Point Indexing

### 7.1 Assignment

Indices are assigned sequentially starting from 0 in the order `register()` is called. The first point registered receives index 0, the second receives index 1, etc.

```rust
pub fn register(&mut self, mut point: EvalPoint) -> u32 {
    let index = self.inner.points.len() as u32;
    point.index = index;
    self.inner.points.push(point);
    self.inner.name_map.insert(point.name, index);
    index  // return the assigned index
}
```

### 7.2 Stability Guarantee

Indices are **stable for the lifetime of the owning render graph**. The render graph lives as long as the `Renderer` owns it, which is typically the entire application lifetime (or until a graph rebuild, e.g. on resolution change or pipeline configuration change).

**What does stability mean in practice?**

- The index assigned to `"gbuffer"` at graph construction never changes until the graph is destroyed.
- Materials compiled with `mask = 1 << 0` always refer to `"gbuffer"`.
- A material's `eval_point_mask` stored in `GpuInstanceData` remains valid across frames.
- Shader variants cached in `ShaderVariantKey { eval_point: index, ... }` remain valid.

**What breaks stability?**

- A **graph rebuild** (e.g. switching from a forward renderer to a deferred renderer, or changing screen dimensions to a different resolution tier). This destroys the old registry and creates a new one. All eval points are re-registered, potentially in a different order.
- **Adding/removing passes at runtime** (see §8.3). The graph must be rebuilt, invalidating all indices.

For the stable case (standard rendering pipeline, no pass swapping), indices are fixed.

### 7.3 Bitmask Mapping

Each eval point index maps to a bit position in the u32 eval-point mask:

```
bit 0  → index 0  → "gbuffer"
bit 1  → index 1  → "transparent"
bit 2  → index 2  → "shadow"
bit 3  → index 3  → "depth_prepass"
bit 4  → index 4  → "ssr"
bit 5  → index 5  → "sss"
bit 6  → index 6  → "decal"
bit 7  → index 7  → "post_process"
bit 8  → index 8  → "velocity"
bit 9  → index 9  → (third-party point 1)
bit 10 → index 10 → (third-party point 2)
...
bit 31 → index 31 → (last available slot)
```

**Table of mask values for common combinations:**

| Combination | Mask (hex) | Mask (binary) |
|---|---|---|
| gbuffer only | `0x00000001` | `...0001` |
| transparent only | `0x00000002` | `...0010` |
| gbuffer + transparent | `0x00000003` | `...0011` |
| gbuffer + ssr | `0x00000011` | `...0001 0001` |
| gbuffer + transparent + ssr (glass) | `0x00000013` | `...0001 0011` |
| shadow only | `0x00000004` | `...0100` |
| depth_prepass only | `0x00000008` | `...1000` |
| gbuffer + shadow + depth_prepass (standard) | `0x0000000D` | `...1101` |
| All 32 bits set | `0xFFFFFFFF` | all 1s |

### 7.4 GPU-Side Usage

**Fragment path** — the vertex shader reads `GpuInstanceData.eval_point_mask` and passes it as a flat-interpolated attribute. The fragment shader checks:

```wgsl
// Each eval point has a compile-time constant for its bit position.
// Set by the shader composition system via #define or a let binding.
const GBUFFER_BIT: u32 = 0u;
const TRANSPARENT_BIT: u32 = 1u;
// ... etc.

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4f {
    // Single predicated instruction — zero cost for never-taken branch
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << GBUFFER_BIT)) == 0u {
        discard;
    }
    // ... evaluate gbuffer material ...
}
```

**Compute path** — the compute shader reads from the GPU dispatch table:

```wgsl
@group(1) @binding(4) var<storage, read> template_dispatches: array<GpuTemplateDispatch>;
@group(1) @binding(5) var template_id_tex: texture_2d<u32>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;

    if (mask & (1u << SSR_BIT)) == 0u {
        return;  // skip — this pixel's material doesn't target SSR
    }
    // ... evaluate SSR material ...
}
```

### 7.5 Compile-Time Bit Constants

The shader composition system can inject the per-point bit constants into the composed WGSL. This allows the shader to hardcode bit positions without runtime indirection:

```rust
fn inject_bit_constants(composed: &mut String, registry: &EvalPointRegistry) {
    for (index, point) in registry.iter() {
        let const_name = format!("{}_BIT", point.name.to_uppercase());
        // e.g. "const GBUFFER_BIT: u32 = 0u;"
        writeln!(composed, "const {}: u32 = {}u;", const_name, index).unwrap();
    }
}
```

---

## 8. Registry Lifecycle

### 8.1 Phase Diagram

```
                         ┌─────────────────────────┐
                         │   build_default_graph()  │
                         └──────────┬──────────────┘
                                    │
                                    ▼
                    ┌───────────────────────────────┐
                    │  RenderGraph::new()            │
                    │  graph.add_pass(pass)          │  ── Register passes
                    │  graph.add_pass(pass)          │
                    └──────────┬────────────────────┘
                               │
                               ▼
                    ┌───────────────────────────────┐
                    │  graph.lock(width, height)     │  ── Resolve resources,
                    │                                │     allocate textures,
                    │                                │     run resize hooks
                    └──────────┬────────────────────┘
                               │
                               ▼
          ┌────────────────────────────────────────────┐
          │  registry = EvalPointRegistry::new()        │
          │  for pass in graph.passes():                │  ── COLLECT PHASE
          │      pass.register_eval_points(&mut registry)│     (mutable)
          │  registry.freeze()                          │
          └──────────────────┬─────────────────────────┘
                             │
                             ▼
          ┌────────────────────────────────────────────┐
          │  renderer.set_eval_registry(registry)       │  ── FROZEN
          │                                             │     (immutable)
          └──────────────────┬─────────────────────────┘
                             │
                             ▼
          ┌────────────────────────────────────────────┐
          │  renderer.register_material(name, wgsl,     │  ── READ PHASE
          │                           &["gbuffer"])     │     registry.get()
          │  registry.index("transparent")              │     registry.index()
          │  registry.points().iter()                   │
          └──────────────────┬─────────────────────────┘
                             │
                             ▼
          ┌────────────────────────────────────────────┐
          │  Frame N: pass.execute()                   │  ── DISPATCH PHASE
          │    Fragment: mask in GpuInstanceData        │     (no registry access)
          │    Compute:  mask in GpuTemplateDispatch[]  │
          └────────────────────────────────────────────┘
```

### 8.2 When the Registry is Created

The registry is created inside `build_default_graph_internal` in the `helio` crate (or the engine-specific graph builder). The exact call site:

```rust
// crates/renderer/helio/crates/helio/src/renderer/setup.rs (or similar)

fn build_default_graph_internal(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &RenderConfig,
) -> (RenderGraph, EvalPointRegistry) {
    let mut graph = RenderGraph::new();

    // ── Instantiate passes ──────────────────────────────────────────
    let gbuffer = GBufferPass::new(device, config);
    let transparent = TransparentPass::new(device, config);
    let shadow = ShadowPass::new(device, config);
    let depth_prepass = DepthPrepass::new(device, config);
    let ssr = SsrPass::new(device, config);
    let sss = SssBlurPass::new(device, config);
    let decal = DecalPass::new(device, config);
    let post = PostProcessPass::new(device, config);
    let velocity = VelocityPass::new(device, config);

    // ── Register passes ─────────────────────────────────────────────
    graph.add_pass(Box::new(depth_prepass));
    graph.add_pass(Box::new(gbuffer));
    graph.add_pass(Box::new(transparent));
    graph.add_pass(Box::new(ssr));
    graph.add_pass(Box::new(sss));
    graph.add_pass(Box::new(decal));
    graph.add_pass(Box::new(velocity));
    graph.add_pass(Box::new(post));
    graph.add_pass(Box::new(shadow));

    // ── Lock graph (resolves resources, allocates textures) ──────────
    graph.lock(config.width, config.height);

    // ── Collect eval points ─────────────────────────────────────────
    let registry = graph.collect_eval_points();
    // registry is now frozen

    (graph, registry)
}
```

### 8.3 When the Registry is Frozen

`registry.freeze()` is called immediately after all passes have registered their points. The freeze transition is:

1. **Set `frozen = true`** — any subsequent `register()` call will panic.
2. **Publish to renderer** — the frozen registry is stored on the `Renderer` via `renderer.set_eval_registry(registry)`.
3. **Share for reads** — `RegistryInner` is behind `Arc`, so `Clone` gives cheap read access to any subsystem that needs it (material system, shader compiler, editor).

After freezing, the `EvalPointRegistry` is **immutable**. The `&self` methods (`get`, `index`, `points`, `iter`) remain available. The `Arc` makes it safe to share across threads.

### 8.4 How the Renderer Accesses It

```rust
// On the Renderer:
impl Renderer {
    /// Returns a reference to the frozen eval point registry.
    /// Panics if called before the registry is set (during graph construction).
    pub fn eval_registry(&self) -> &EvalPointRegistry {
        self.eval_registry.as_ref()
            .expect("eval registry not set — graph not yet constructed")
    }

    /// Convenience: look up an eval point by name.
    pub fn eval_point(&self, name: &str) -> Option<&EvalPoint> {
        self.eval_registry().get(name)
    }

    /// Convenience: look up an eval point index by name.
    pub fn eval_point_index(&self, name: &str) -> Option<u32> {
        self.eval_registry().index(name)
    }

    /// Set the registry (called during graph construction).
    /// This overwrites any previous registry (for graph rebuilds).
    pub(super) fn set_eval_registry(&mut self, registry: EvalPointRegistry) {
        assert!(registry.is_frozen(), "registry must be frozen before storage");
        self.eval_registry = Some(registry);
    }
}
```

### 8.5 Graph Rebuild

When the render graph is rebuilt (resolution change, pipeline configuration change, or hot-reload of pass code):

1. The old `Renderer` is drained of its graph and registry.
2. `build_default_graph_internal` creates a new graph and a new registry.
3. `renderer.set_eval_registry(new_registry)` installs the new frozen registry.
4. All materials must be re-registered (their shader variants were compiled against the old eval point indices). The renderer's material system detects the registry version change and invalidates the shader variant cache.

```rust
impl Renderer {
    /// Rebuild the render graph. Called on resize or config change.
    pub fn rebuild_graph(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, config: &RenderConfig) {
        let (new_graph, new_registry) = build_default_graph_internal(device, queue, config);
        let old_registry = self.eval_registry.replace(new_registry);

        // If eval point indices changed, invalidate shader cache
        if old_registry.as_ref().map_or(true, |r| r.count() != new_registry.count()) {
            self.shader_cache.clear();
            self.material_system.recompile_all_materials(device, &new_registry);
        }

        self.graph = new_graph;
    }
}
```

---

## 9. Error Cases

### 9.1 Duplicate Name Registration

```rust
// ── Cause ──────────────────────────────────────────────────────────
// Two passes register eval points with the same name:

impl RenderPass for GBufferPass {
    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint { name: "gbuffer", ... });
    }
}

impl RenderPass for MyCustomPass {
    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint { name: "gbuffer", ... });  // ERROR
    }
}

// ── Behavior ──────────────────────────────────────────────────────
// Panics with: "duplicate eval point name: 'gbuffer'"
//
// This is a **programming error** (not a runtime error that should be handled).
// It indicates either:
//   a) Two built-in passes have a naming collision (engine bug)
//   b) A third-party pass uses a name that conflicts with a built-in point
//   c) A pass calls register_eval_points twice (logic bug)
```

### 9.2 Capacity Exceeded

```rust
// ── Cause ──────────────────────────────────────────────────────────
// More eval points are registered than the u32 bitmask can represent.
// Default capacity is 32 (bits 0..31).

// Registry with default capacity (32):
for i in 0..32 {
    registry.register(EvalPoint { name: &format!("point_{}", i), ... });
}
registry.register(EvalPoint { name: "point_32", ... });
// Panics: "eval point capacity reached (32) — cannot register 'point_32'"

// ── Behavior ──────────────────────────────────────────────────────
// Panics with the capacity error message.
//
// ── Mitigation ────────────────────────────────────────────────────
// If a project genuinely needs more than 32 eval points:
//   a) The registry uses `atomic::u64` on the GPU side instead of u32
//   b) GpuInstanceData.eval_point_mask expands to u64
//   c) Capacity is set to 64 via EvalPointRegistry::with_capacity(64)
// These changes require GPU-side updates and are a future extension.
// Current spec assumes u32 mask for zero-cost embedding in instance data.
```

### 9.3 Registration After Freeze

```rust
// ── Cause ──────────────────────────────────────────────────────────
// Code attempts to register an eval point after registry.freeze():

let mut registry = EvalPointRegistry::new();
registry.register(EvalPoint { name: "gbuffer", ... });
registry.freeze();
registry.register(EvalPoint { name: "custom", ... });
// Panics: "cannot register eval point after registry is frozen"

// ── Behavior ──────────────────────────────────────────────────────
// Panics. This is a programming error — the graph builder must ensure
// all passes register their points before freeze.
//
// ── Root Cause ────────────────────────────────────────────────────
// Typically caused by:
//   a) A pass being added to the graph AFTER graph.lock()
//   b) Calling collect_eval_points() before all passes are registered
```

### 9.4 Empty Base Shader

```rust
// ── Cause ──────────────────────────────────────────────────────────
EvalPoint {
    name: "custom",
    base_shader: "",  // empty!
    ...
};

registry.register(point);
// Panics: "base_shader for 'custom' is empty"

// ── Behavior ──────────────────────────────────────────────────────
// Panics. The base shader must contain at minimum the entry function
// declaration and the RADIANT_OVERRIDE markers.
```

### 9.5 Invalid Entry Function Signature

```rust
// ── Cause ──────────────────────────────────────────────────────────
EvalPoint {
    entry_fn_signature: "not_a_function(material_id: u32)",  // missing "fn " prefix
    ...
};

registry.register(point);
// Panics: "entry_fn_signature must contain 'fn <name>(...)' — got 'not_a_function(...)'"

// ── Behavior ──────────────────────────────────────────────────────
// Panics. The signature parser requires the canonical "fn <name>(" form.
```

### 9.6 What Happens When a Pass Is Removed from the Graph

When a pass is removed:

```rust
graph.remove_pass("SsrPass");  // hypothetical API
```

1. **The eval point registry is unaffected until the next graph rebuild.** The registry is frozen; its contents are immutable.
2. **Material masks may still have bit 4 set** (the `ssr` point). If `SsrPass` is removed from the graph, the `ssr` eval point's bit is stale but harmless: no compute dispatch reads the mask for SSR, and the bit is simply ignored.
3. **On the next graph rebuild** (which must happen after removing a pass), a new registry is created without `ssr`. All materials targeting `ssr` must be re-registered (or they will fail validation with `"unknown eval point 'ssr'"`).

```rust
// ── Recommendation ────────────────────────────────────────────────
// When a pass is removed at runtime:
//   1. graph.remove_pass("SsrPass")
//   2. Invalidate all materials that target "ssr":
//      for each material_class where mask & (1 << ssr_index) != 0:
//          mark material for recompilation
//   3. Rebuild graph: renderer.rebuild_graph(...)
//   4. Material system recompiles invalidated materials
//   5. The new registry does not include "ssr"
//   6. Materials that specified "ssr" in their target list must be
//      updated or they will fail the "unknown eval point" check
```

### 9.7 Material References Nonexistent Eval Point

```rust
// ── During material registration ──────────────────────────────────
let class = renderer.register_material(
    "my_mat",
    my_wgsl,
    &["gbuffer", "nonexistent_point"],  // "nonexistent_point" not in registry
);

// ── Behavior ──────────────────────────────────────────────────────
// Returns Err(MaterialError::UnknownEvalPoint("nonexistent_point"))
// or panics depending on the API design. The spec recommends returning
// a Result to allow the editor to display a user-friendly error message.
```

### 9.8 Capacity Mismatch Across Graph Rebuild

```rust
// ── Cause ──────────────────────────────────────────────────────────
// A graph rebuild changes the number or order of eval points.
// Old material dispatch masks (stored in GpuInstanceData) refer to
// bits that now map to different eval points.

// Before rebuild:
//   index 0 = "gbuffer", index 1 = "transparent"
//   Material A mask = 0x01 (targets gbuffer)
//
// After rebuild (third-party point added before transparent):
//   index 0 = "gbuffer", index 1 = "custom_pass", index 2 = "transparent"
//   Material A mask = 0x01 → still targets gbuffer ✓ (bit 0 unchanged)
//   Material B mask = 0x02 → previously "transparent", now "custom_pass" ✗

// ── Behavior ──────────────────────────────────────────────────────
// Silent misbehavior: Material B now evaluates "custom_pass" instead of
// "transparent", which may produce wrong rendering or GPU errors.
//
// ── Mitigation ────────────────────────────────────────────────────
// On graph rebuild with different eval point configuration:
//   1. Detect change: compare registry version hash
//   2. Invalidate all shader variants
//   3. Recompile all materials against the new registry
//   4. Rebuild instance buffers with updated masks
```

---

## 10. Testing Strategy

### 10.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let mut reg = EvalPointRegistry::new();
        let idx = reg.register(EvalPoint {
            name: "test_point",
            entry_fn_signature: "fn eval_test(x: f32) -> f32",
            base_shader: "fn eval_test(x: f32) -> f32 { return x; }",
            // ... other fields ...
        });
        assert_eq!(idx, 0);
        assert_eq!(reg.index("test_point"), Some(0));
        assert!(reg.get("test_point").is_some());
    }

    #[test]
    fn duplicate_name_panics() {
        let mut reg = EvalPointRegistry::new();
        reg.register(EvalPoint { name: "gbuffer", ... });
        std::panic::catch_unwind(|| {
            reg.register(EvalPoint { name: "gbuffer", ... });
        }).expect_err("should have panicked on duplicate");
    }

    #[test]
    fn capacity_limit() {
        let mut reg = EvalPointRegistry::with_capacity(2);
        reg.register(EvalPoint { name: "a", ... });
        reg.register(EvalPoint { name: "b", ... });
        std::panic::catch_unwind(|| {
            reg.register(EvalPoint { name: "c", ... });
        }).expect_err("should have panicked on capacity exceeded");
    }

    #[test]
    fn freeze_prevents_registration() {
        let mut reg = EvalPointRegistry::new();
        reg.register(EvalPoint { name: "a", ... });
        reg.freeze();
        std::panic::catch_unwind(|| {
            reg.register(EvalPoint { name: "b", ... });
        }).expect_err("should have panicked on registration after freeze");
    }

    #[test]
    fn unknown_name_returns_none() {
        let reg = EvalPointRegistry::new();
        assert!(reg.get("nonexistent").is_none());
        assert!(reg.index("nonexistent").is_none());
    }

    #[test]
    fn extract_fn_name_from_signature() {
        assert_eq!(
            extract_fn_name("fn eval_gbuffer(material_id: u32, ...) -> SurfaceData"),
            Some("eval_gbuffer")
        );
        assert_eq!(
            extract_fn_name("fn eval_test()"),
            Some("eval_test")
        );
        assert!(extract_fn_name("not_a_fn(x: f32)").is_none());
    }

    #[test]
    fn empty_base_shader_panics() {
        let mut reg = EvalPointRegistry::new();
        std::panic::catch_unwind(|| {
            reg.register(EvalPoint { name: "bad", base_shader: "", ... });
        }).expect_err("should have panicked on empty base shader");
    }
}
```

### 10.2 Integration Tests

- **Graph builder test**: Create a `RenderGraph` with three passes, call `collect_eval_points()`, verify registry contains exactly the expected points in the expected order.
- **Mask building test**: Register materials with various target combinations, verify `eval_point_mask` bits are correctly set.
- **Graph rebuild test**: Rebuild graph with a different pass set, verify old materials fail validation with appropriate error.

---

## 11. Future Considerations

### 11.1 Extending to u64 Masks

If 32 eval points prove insufficient:

1. Change `eval_point_mask` from `u32` to `u64` in `GpuInstanceData`
2. Change `capacity` default from `32` to `64`
3. Update the GPU-side bit check from `1u << bit` to `1ul << bit`
4. Add 4 bytes to `GpuInstanceData` (was previously padding, now may push into a new cache line)

This is a straightforward mechanical change if needed. The current 32-point capacity is conservative — most engines ship with 8–12 material evaluation points.

### 11.2 Dynamic Eval Points at Runtime

For runtime-loaded plugins that add new passes after graph construction:

1. **Option A**: Require a graph rebuild (simpler, safer — current approach)
2. **Option B**: Support "hot" registration into a mutable eval point slot (requires pre-allocating reserve slots in the registry, dynamic mask expansion, and shader variant recompilation)

Option A is recommended for v2. Option B may be added in v3 if profiling shows graph rebuilds are a bottleneck.

### 11.3 Eval Point Aliases

Support for aliasing one eval point to another (e.g. `"decal"` → `"gbuffer"`) for backward compatibility during migration. This is a registry-level remapping that would be applied during material mask construction:

```rust
registry.add_alias("decal", "gbuffer");
// material targeting "decal" → mask includes bit 0
```

---


> **Part of**: Helio Radiant v2 Specification  
> **Supersedes**: v1 `register_partial_str`, `register_str`, `RadiantTemplateRegistry` internal API  
> **Status**: Draft  
> **Crate**: `pulsar_rendering` (core), `pulsar_render_shared` (types)

---

## Table of Contents

1. [`register_material()` API](#1-register_material-api)
2. [Shader Composition Algorithm](#2-shader-composition-algorithm)
3. [Multi-eval Materials](#3-multi-eval-materials)
4. [Shader Variant Caching](#4-shader-variant-caching)
5. [Backward Compatibility Shim](#5-backward-compatibility-shim)
6. [template ID Management](#6-material-class-id-management)
7. [Dispatch Mask Computation](#7-dispatch-mask-computation)
8. [Error Handling](#8-error-handling)
9. [WGSL Template Examples](#9-wgsl-template-examples)

---

## 1. `register_material()` API

### 1.1 Full Signature

```rust
impl Renderer {
    /// Register a material with the Helio Radiant v2 system.
    ///
    /// # Parameters
    /// - `name`:           Human-readable material name (used for debug labels, profiling).
    /// - `wgsl_source`:    Full WGSL source containing one or more eval functions.
    /// - `targets`:        Slice of eval point name strings this material targets.
    ///
    /// # Returns
    /// A `MaterialClassId` (newtype over u32) that identifies this material
    /// in all subsequent scene data (GpuInstanceData, template_id_tex, etc.).
    ///
    /// # Panics
    /// - If any eval point name in `targets` is not registered in the eval point registry.
    /// - If `wgsl_source` does not contain the required entry function for any target point.
    /// - If the number of bits set across all targets exceeds 32 (would overflow u32 mask).
    ///
    /// # Thread Safety
    /// Must be called on the main thread during scene setup or hot-reload.
    /// Not `Send` — the internal shader cache is !Send.
    pub fn register_material(
        &mut self,
        name: &str,
        wgsl_source: &str,
        targets: &[&str],
    ) -> MaterialClassId;
}
```

### 1.2 Return Type

```rust
/// Opaque handle representing a registered material class.
/// Internally a u32 index into the material registry Vec.
/// Must be stable across hot-reload (see §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialClassId(u32);

impl MaterialClassId {
    pub const INVALID: MaterialClassId = MaterialClassId(u32::MAX);
    pub fn as_u32(self) -> u32 { self.0 }
}
```

### 1.3 Internal Flow (Step by Step)

```
register_material(name, wgsl_source, targets)
│
├─ 1. Validate targets
│   ├─ For each target_name in targets:
│   │   ├─ Lookup eval_point = eval_registry.get(target_name)
│   │   ├─ IF not found → panic!("unknown eval point: '{target_name}'")
│   │   ├─ Extract expected_fn_name from eval_point.entry_fn_signature
│   │   │   (parsed from "fn eval_gbuffer(...)" → "eval_gbuffer")
│   │   ├─ Check wgsl_source contains expected_fn_name via regex
│   │   │   r"(?m)^fn {expected_fn_name}\s*\("
│   │   │   IF not found → panic!("material '{name}' missing fn {expected_fn_name}")
│   │   └─ Check no other fn with same name exists in base shader
│   └─ IF targets.len() > 32 (or would overflow u32 mask)
│       → panic!("too many target eval points (>32): {targets}")
│
├─ 2. Assign template ID
│   ├─ class_id = self.next_class_id()
│   │   (see §6 for ID management algorithm)
│   └─ Store material record:
│       MaterialRecord {
│           name: name.to_string(),
│           wgsl_source: wgsl_source.to_string(),
│           targets: targets.to_vec(),
│           class_id,
│           eval_point_mask: 0, // computed below
│           variant_cache: HashMap::new(), // per-eval-point compiled variants
│       }
│
├─ 3. For each (target_name, eval_point):
│   │
│   ├─ 3a. Compose shader
│   │   ├─ composed = compose_fn_override(
│   │   │     base_shader:  eval_point.base_shader,
│   │   │     override_src: wgsl_source,
│   │   │     fn_name:      eval_point.entry_fn_name,
│   │   │   )
│   │   │   Returns full WGSL with overridden eval function body.
│   │   │
│   │   ├─ 3b. Build ShaderVariantKey
│   │   │   └─ key = ShaderVariantKey {
│   │   │         eval_point:     eval_point.index,
│   │   │         template_id:    class_id.as_u32(),
│   │   │         graph_hash:     0,       // reserved for future WGSL snippets
│   │   │         feature_flags:  0,       // reserved for compile-time feature flags
│   │   │       }
│   │   │
│   │   └─ 3c. Compile variant
│   │       ├─ Attempt WGPU shader module compilation
│   │       ├─ IF fail:
│   │       │   → Emit detailed compilation error with:
│   │       │     - Material name
│   │       │     - Target eval point name
│   │       │     - Composed WGSL source (for debugging)
│   │       │     - WGPU compilation error message
│   │       │   → panic!("WGSL compilation failed for material '{name}' at eval point '{target_name}'")
│   │       ├─ Store in self.variant_cache[key] = CompiledVariant { module, pipeline: None }
│   │       │   (pipeline is None → lazily compiled on first dispatch, see §4)
│   │       └─ IF eval_point.threading == Fragment:
│   │           → Pre-create RenderPipeline (eager compile for fragment)
│   │           ELSE:
│   │           → Lazy compile (ComputePipeline created on first dispatch)
│   │
│   └─ 3d. IF any variant compilation fails → cleanup already-registered variants
│       and re-panic with combined error
│
├─ 4. Compute dispatch mask
│   ├─ mask = 0u32
│   ├─ For each target_name in targets:
│   │   ├─ idx = eval_registry.index(target_name)
│   │   │   (unwraps: already validated in step 1)
│   │   └─ mask |= 1 << idx
│   ├─ Store mask in material record:
│   │   material.eval_point_mask = mask
│   └─ (Mask propagates to GpuInstanceData during scene rebuild, see §7)
│
├─ 5. Emit debug logging
│   ├─ log::debug!("registered material '{name}' as class_id={} mask={:#010x}",
│   │     class_id.as_u32(), mask);
│   └─ For each target: log::trace!("  target '{}' (index={})", target_name, idx)
│
└─ 6. Return class_id
```

### 1.4 Multi-step Error Recovery

If step 3c fails for any variant, the implementation **must** clean up all
variants already inserted into the cache before panicking. This prevents a
partial material registration from leaving the variant cache in an
inconsistent state.

```rust
// Pseudo-code for error recovery:
let mut compiled_keys = Vec::new();
for (key, composed, eval_point) in variants_to_compile {
    match compile_variant(device, key, &composed, eval_point) {
        Ok(v) => {
            self.variant_cache.insert(key, v);
            compiled_keys.push(key);
        }
        Err(e) => {
            // Cleanup: remove all previously compiled variants
            for k in compiled_keys { self.variant_cache.remove(&k); }
            panic!("material '{name}' variant '{:?}' failed: {e}", key);
        }
    }
}
```

### 1.5 Registration Timing

`register_material()` may be called:

| Phase | Behavior |
|---|---|
| **Engine startup** | All built-in materials registered before first frame |
| **Scene load** | Materials registered when their assets are discovered |
| **Editor hot-reload** | Re-registers material with same `name`; old ID may be reused (see §6) |
| **Runtime (game)** | Not recommended but possible; new variants compiled on-demand |
| **Thread pool** | NOT allowed — must be called on the main WGPU thread |

---

## 2. Shader Composition Algorithm

### 2.1 `compose_fn_override()` Signature

```rust
/// Composes a material's override function into an eval point's base shader.
///
/// # Algorithm
/// 1. Find the default eval function declaration in `base_shader` by matching
///    `fn <fn_name>(` at the start of a line (possibly preceded by whitespace).
/// 2. Extract the function body using brace-depth tracking.
/// 3. Find the matching function in `override_src` by the same rule.
/// 4. Replace the base shader's function body (from opening `{` to matching `}`)
///    with the override's function body.
/// 5. Return the resulting composed WGSL string.
///
/// # Preconditions
/// - `base_shader` must contain exactly one function named `fn_name`.
/// - `override_src` must contain exactly one function named `fn_name`.
///
/// # Panics
/// - If `fn_name` is not found in `base_shader`.
/// - If `fn_name` is not found in `override_src`.
/// - If multiple functions named `fn_name` exist in either source.
/// - If brace-depth tracking hits EOF before closing the function body.
pub fn compose_fn_override(
    base_shader: &str,
    override_src: &str,
    fn_name: &str,
) -> String;
```

### 2.2 Detailed Algorithm

```
compose_fn_override(base_shader, override_src, fn_name):
│
├─ A. Find function in base_shader
│   ├─ Build regex: r"(?m)^(?:\s*?)fn \{fn_name\}\("
│   │   (matches "fn eval_gbuffer(" at line start, with optional leading whitespace)
│   ├─ Search base_shader for match
│   │   ├─ IF 0 matches → panic!("base shader missing fn '{fn_name}'")
│   │   ├─ IF >1 matches → panic!("base shader has multiple fn '{fn_name}'")
│   │   └─ Record: fn_start = match.start()
│   │
│   ├─ From fn_start, scan forward to find opening brace '{'
│   │   (skip past the parameter list and return type)
│   │   - Track parentheses depth to avoid matching '{' inside parameter defaults
│   │   - WGSL does not support default parameters, but future-proof
│   │   - When paren_depth == 0 and char == '{': body_start = pos + 1
│   │
│   └─ From body_start, find matching closing brace '}':
│       ├─ Initialize brace_depth = 1
│       ├─ Iterate characters from body_start:
│       │   ├─ '{' → brace_depth++
│       │   ├─ '}' → brace_depth--
│       │   │   IF brace_depth == 0: body_end = pos; break
│       │   └─ Skip string/comment contents:
│       │       ├─ "//" → skip to '\n'
│       │       └─ "/*" → skip to "*/"
│       ├─ IF EOF reached with brace_depth > 0
│       │   → panic!("unterminated function body for '{fn_name}' in base shader")
│       └─ Record: body_range = body_start..body_end
│           (the inner content without { })
│
├─ B. Find function in override_src
│   ├─ Same regex search as step A
│   ├─ 0 matches → panic!("override source missing fn '{fn_name}'")
│   ├─ >1 matches → panic!("override source has multiple fn '{fn_name}'")
│   ├─ Find opening '{' (same paren-tracking logic)
│   ├─ Find matching '}' (same brace-depth logic)
│   └─ Record: override_body = override_src[body_start..body_end]
│       (the inner content without { })
│
├─ C. Assemble composed shader
│   ├─ Take base_shader[0..body_start]  (everything before the body)
│   ├─ Append override_body
│   ├─ Append base_shader[body_end..]   (everything after the closing brace)
│   └─ Return the concatenated string
│
└─ Note: bindings, vertex shader, struct definitions are all preserved
    from base_shader — only the eval function body is replaced.
```

### 2.3 What About Things the Override References Outside Its Scope?

The override function may reference:

| Reference | Behavior | Valid? |
|---|---|---|
| **Parameters** (material_id, world_pos, etc.) | Provided by the pass's vertex/fragment entry point — always available | ✅ OK |
| **Globals** (camera, globals uniform) | Defined in base shader, available to all functions | ✅ OK |
| **Material textures** (`material_textures[N]`) | Defined in base shader bindings | ✅ OK |
| **Helper functions** defined elsewhere in override_src | Must be defined in override_src itself | ✅ OK |
| **Helper functions** defined ONLY in base shader | Available — base shader unchanged | ✅ OK |
| **Helper functions** defined in BOTH | Override's function is unused (base's is used) | ⚠️ Redundant but harmless |
| **`#define` / preprocessor macros** | WGSL has no preprocessor | N/A |
| **Functions with same name but different signature** | WGSL does not support overloading | N/A |
| **Render pass internal state** (e.g., `gbuffer_targets` struct) | Defined in base shader | ✅ OK |
| **GPU storage buffers not declared in base shader** | WGPU compilation error — undefined symbol | ❌ Must add to base shader bindings |

**Critical invariant**: The override function's signature **must** match the
base shader's expected signature exactly. The base shader calls the function
with specific argument types; if the override declares different parameters,
WGPU compilation will fail with a type mismatch error. This is caught at
variant compilation time (step 3c in `register_material`).

### 2.4 Edge Cases

#### 2.4.1 Function Not Found in Base Shader

```rust
// Cause: The eval point's base_shader.wgsl was modified and no longer
// contains the expected default eval function.
// Behavior: Panics with a clear message.
// Mitigation: This is a programmer error (pass author must keep the base
// shader in sync with the eval point declaration).
```

#### 2.4.2 Multiple Functions with Same Name

```rust
// In WGSL, function overloading does not exist. However, a user could
// accidentally define two functions named e.g. `eval_gbuffer` in their
// override source.
//
// Behavior: compose_fn_override panics with a descriptive error.
//
// Note: The base shader should never have duplicates either — this is
// validated at eval point registration time.
```

#### 2.4.3 Syntax Errors in Override Body

Syntax errors are **not** caught by `compose_fn_override()` (which is a
text-level operation). They are caught during WGPU shader module compilation
(step 3c). The error message includes:

```
WGSL compilation failed for material 'glass' at eval point 'transparent':
  ┌─ composed_shader.wgsl:142:9
  │
142│     let fresnel = pow(1.0 - NdV, 4.0
  │                                   ^ expected ')'
  │
  = material source: glass.wgsl
  = base shader: transparent_base.wgsl
  = eval point: "transparent" (index 1)
```

#### 2.4.4 Brace Characters in WGSL Strings

WGSL has no string literals that could contain braces (no `const char*`
with embedded `{}`). The only concern is comments:

```wgsl
// This { has a brace — safe, skipped by comment parser
/* This } also safe — skipped by block comment parser */
```

The brace tracker skips `//` line comments and `/* */` block comments,
so braces inside comments are ignored.

#### 2.4.5 Empty Override Body

If the override function body is empty (just `fn eval(...) { }`), the
composed shader will compile but likely produce incorrect results (default
initialization of return type). This is a material authoring error, not a
system error. No panic — WGPU will compile it.

#### 2.4.6 Base Shader Has No `RADIANT_OVERRIDE` Markers

`compose_fn_override` does NOT rely on `RADIANT_OVERRIDE_START` / `END`
markers. It finds the function purely by name and brace-tracking. The
markers are a **debugging aid** for humans reading composed output but
are not required. If markers exist inside the function body they are
silently replaced along with the rest of the body.

#### 2.4.7 Override Contains Unused Additional Functions

If the override WGSL contains helper functions (e.g., `fn wave_normal()`),
they are **not** automatically included in the composed shader. Only the
matching eval function body is extracted.

To include helpers, the material author must **duplicate them in the base
shader** or the material must include them inside the eval function body
as nested functions (WGSL supports nested functions) or as separate
functions that get compiled in the same module.

Wait — this is actually a critical subtlety. Let me clarify:

**What happens to helper functions defined in `override_src` but outside
the eval function?**

They are DISCARDED. `compose_fn_override` extracts ONLY the function body
of `eval_gbuffer`, not the surrounding source. If the material depends on
helper functions defined outside that function, the composed shader will
fail to compile.

**Solution for shared helpers**: There are three approaches:

1. **Define helpers inside the eval function** (WGSL supports local
   functions within functions):
   ```wgsl
   fn eval_gbuffer(...) -> SurfaceData {
       fn helper(x: f32) -> f32 { return x * 2.0; }
       return helper(1.0);
   }
   ```

2. **Define helpers in the base shader** (available to all materials):
   ```wgsl
   // transparent_base.wgsl
   fn common_math_helper(v: vec3f) -> f32 { return length(v); }
   
   fn eval_transparent(...) -> vec4f {
       return vec4f(common_math_helper(...)); // default impl
   }
   ```

3. **Make the material a single self-contained function** (recommended):
   Keep the entire material logic inside the eval function body. This is
   the intended pattern for the v2 system.

#### 2.4.8 Override Contains `@` Attributes on the Function

If the override source has `@fragment` or `@compute` attributes before the
function, they are **not** included in the extracted body (the regex matches
`fn eval_` at line start, not `@fragment`). This is correct — the eval
function is never an entry point in the composed shader; only the pass's
main entry point (`fs_main`, `cs_main`) is.

---

## 3. Multi-eval Materials

### 3.1 Convention

A single WGSL source file may provide multiple eval functions, one per
targeted evaluation point. The naming convention is:

```
eval_{eval_point_name}
```

For example, a material targeting `"gbuffer"` and `"transparent"` must
provide both:

```wgsl
fn eval_gbuffer(...) -> SurfaceData { ... }
fn eval_transparent(...) -> vec4f { ... }
```

### 3.2 How Composition Handles Multi-eval

When `register_material("glass", glass_wgsl, &["gbuffer", "transparent"])`
is called, the engine iterates each target and composes independently:

```
Target 1: "gbuffer"
  ├─ compose_fn_override(gbuffer_base.wgsl, glass_wgsl, "eval_gbuffer")
  │   ├─ Finds fn eval_gbuffer in gbuffer_base.wgsl (default impl)
  │   ├─ Finds fn eval_gbuffer in glass_wgsl (override)
  │   ├─ Replaces default body with override body
  │   └─ Returns gbuffer_base.wgsl + glass's eval_gbuffer body
  └─ Compile variant → cache key (eval_point=0, template_id=5, ...)

Target 2: "transparent"
  ├─ compose_fn_override(transparent_base.wgsl, glass_wgsl, "eval_transparent")
  │   ├─ Finds fn eval_transparent in transparent_base.wgsl
  │   ├─ Finds fn eval_transparent in glass_wgsl
  │   ├─ Replaces default body with override body
  │   └─ Returns transparent_base.wgsl + glass's eval_transparent body
  └─ Compile variant → cache key (eval_point=1, template_id=5, ...)
```

The two composed shaders are **completely independent**. They have:
- Different base shaders (different bindings, different entry points)
- Different extracted function bodies
- Different compiled WGPU modules
- Different pipeline objects

### 3.3 Shared Helper Functions

If the material WGSL contains helper functions outside the eval functions:

```wgsl
// glass.wgsl
fn fresnel_factor(NdV: f32) -> f32 {
    return pow(1.0 - NdV, 4.0);
}

fn eval_gbuffer(...) -> SurfaceData {
    let f = fresnel_factor(...);
    // ...
}

fn eval_transparent(...) -> vec4f {
    let f = fresnel_factor(...);
    // ...
}
```

**Problem**: `fresnel_factor` is defined in `glass.wgsl` but outside both
eval functions. When `compose_fn_override` extracts only the body of
`eval_gbuffer`, `fresnel_factor` is NOT included. The composed shader
will fail to compile with "undefined identifier 'fresnel_factor'".

**Solutions**:

1. **Duplicate the helper inside each eval function** (simple but violates
   DRY):
   ```wgsl
   fn eval_gbuffer(...) -> SurfaceData {
       fn fresnel_factor(NdV: f32) -> f32 { return pow(1.0 - NdV, 4.0); }
       let f = fresnel_factor(...);
       // ...
   }
   fn eval_transparent(...) -> vec4f {
       fn fresnel_factor(NdV: f32) -> f32 { return pow(1.0 - NdV, 4.0); }
       // ...
   }
   ```

2. **Define the helper in the base shader** (available to ALL materials
   targeting that eval point — not ideal for material-specific helpers).

3. **Use the `radiant_helpers` mechanism**: A future extension could
   extract all top-level `fn` definitions and splice them into the
   composed output, but this adds complexity and risks name collisions.
   **Not in v2.0.**

4. **Recommended**: Use an `include` pattern at the WGSL level. Define
   shared helpers in a separate `.wgsl` file and `#include` them in each
   eval function body using the engine's WGSL preprocessor (if available).
   As of v2.0, WGSL has no `#include`, but the engine could provide one
   as a pre-processing step.

### 3.4 Helper Inclusion in Variant Caching

If the engine does implement helper extraction (future optimization), the
`graph_hash` field in `ShaderVariantKey` should incorporate a hash of the
extracted helpers to ensure that two materials that share the same eval
function body but different helpers get different cached variants.

### 3.5 Partial Registration Failure in Multi-eval

If a material targets 4 eval points and compilation fails for point 3,
all 4 variants should be cleaned up. The material is considered not
registered. The caller receives a panic. This is the same cleanup logic
described in §1.4.

### 3.6 Performance Implication

Multi-eval materials pay the cost of:

1. **N shader compilations** at registration time (one per eval point)
2. **N pipeline objects** in GPU memory
3. **N draw calls** (each pass draws the same geometry, but only the
   relevant eval point is active per pass)

The third point is the key trade-off: glass geometry is drawn twice (once
in gbuffer, once in transparent), but each draw only does the work for
that pass. This is unavoidable — glass needs both passes. The v2 system
makes this explicit rather than implicit.

---

## 4. Shader Variant Caching

### 4.1 `ShaderVariantKey` Struct

```rust
/// Canonical key for a compiled shader variant.
///
/// Two materials with the same key share a compiled PSO/compute shader.
/// The key is designed so that any semantic difference in the material
/// evaluation produces a different key.
#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
#[repr(C)]
pub struct ShaderVariantKey {
    /// Index into the global EvalPointRegistry.
    /// Determines which base shader, bindings, and threading model to use.
    pub eval_point: u32,

    /// template ID assigned by register_material().
    /// Separates different materials targeting the same eval point.
    pub template_id: u32,

    /// Hash of the WGSL snippet graph (when the material graph compiler
    /// produces snippet-based outputs). Zero = no graph, the WGSL is
    /// hand-written. Future use for editor's material graph.
    pub graph_hash: u64,

    /// Bitmask of compile-time feature flags (e.g., USE_NORMAL_MAPS,
    /// USE_EMISSIVE, USE_SUBSURFACE). Zeros = no flags, full-featured
    /// variant. Used for shader stripping (future optimization).
    pub feature_flags: u32,
}
```

**Total size**: 4 + 4 + 8 + 4 = **20 bytes** (plus HashMap overhead).

### 4.2 Key Uniqueness Rules

| Variation | Different Key? | Rationale |
|---|---|---|
| Same material, different eval point | ✅ Yes (`eval_point` differs) | Different base shader, different pipeline |
| Different material, same eval point | ✅ Yes (`template_id` differs) | Different function body in composed WGSL |
| Same material, same eval point, different graph_hash | ✅ Yes | Different WGSL snippet composition |
| Same material, same eval point, different feature_flags | ✅ Yes | Different compiled variant (shader stripping) |
| Exact same material re-registered (hot-reload) | ❌ Same key | Template ID reused, same eval, same WGSL |
| Two identical materials with different names | ❌ Same key | Hash collision is intentional — cache hit |

### 4.3 Cache Structure

The variant cache is organized as a **per-eval-point HashMap**. This
provides natural sharding and avoids a single giant HashMap.

```rust
/// Per-eval-point variant cache.
/// Each eval point gets its own HashMap so that lookups during pass
/// execution are cache-friendly (a pass only queries its own eval point).
struct PerEvalPointCache {
    /// The compiled variants for this eval point.
    /// Key: (template_id, graph_hash, feature_flags) — eval_point is
    /// implicit from which cache this is.
    variants: HashMap<(u32, u64, u32), CompiledVariant>,

    /// WGPU device reference for creating pipelines.
    device: Arc<wgpu::Device>,

    /// WGPU queue reference for writing shader constants (if any).
    queue: Arc<wgpu::Queue>,
}

/// A compiled variant. The pipeline is lazily created for compute shaders.
enum CompiledVariant {
    /// Fully compiled: module + render pipeline (fragment passes).
    /// Pipeline is eagerly created because fragment passes need PSO
    /// state (blend, depth, rasterization) that is known at compile time.
    FragmentReady {
        module: wgpu::ShaderModule,
        pipeline: wgpu::RenderPipeline,
    },

    /// Partially compiled: only the module is ready (compute passes).
    /// Pipeline is created lazily on first dispatch because compute
    /// pipelines don't need as much fixed-function state and we may
    /// never dispatch this material for this eval point on a given frame.
    ComputeModuleReady {
        module: wgpu::ShaderModule,
    },

    /// Fully compiled compute variant.
    ComputeReady {
        module: wgpu::ShaderModule,
        pipeline: wgpu::ComputePipeline,
    },
}
```

The top-level cache:

```rust
struct ShaderVariantCache {
    /// One cache per eval point index.
    /// Indexed by eval_point_index (0..eval_registry.count()).
    per_eval: Vec<PerEvalPointCache>,

    /// Quick lookup: (eval_point, template_id) → variant.
    /// Used during scene rebuild for debug validation.
    debug_key_check: HashMap<(u32, u32), ShaderVariantKey>,
}
```

### 4.4 Lazy Compilation Strategy

| Pass Type | Compilation Strategy | Rationale |
|---|---|---|
| **Fragment** (gbuffer, transparent, shadow) | **Eager**: pipeline created at `register_material()` time | Fragment passes need PSO state (blend, depth, stencil). The material is registered before the scene renders; we know it will be drawn. The compilation cost is upfront. |
| **Compute** (SSR, SSS, post-process) | **Lazy**: module created at `register_material()` time, pipeline created on first dispatch | Compute passes may never dispatch a given material (e.g., SSR pass skips pixels not targeting `"ssr"`). Creating the pipeline lazily avoids wasted compilation. |

```rust
fn get_or_create_compute_pipeline(
    &mut self,
    key: ShaderVariantKey,
    eval_point: &EvalPoint,
    device: &wgpu::Device,
) -> &wgpu::ComputePipeline {
    let cache = &mut self.variant_cache.per_eval[key.eval_point as usize];

    match cache.variants.entry((key.template_id, key.graph_hash, key.feature_flags)) {
        Entry::Occupied(entry) => {
            // Upgrade from ComputeModuleReady to ComputeReady if needed
            match entry.get() {
                CompiledVariant::ComputeReady { pipeline, .. } => return pipeline,
                CompiledVariant::ComputeModuleReady { module } => {
                    let pipeline = device.create_compute_pipeline(
                        ComputePipelineDescriptor {
                            label: Some(&format!(
                                "material-{}:eval-{}",
                                key.template_id, key.eval_point
                            )),
                            layout: eval_point.compute_layout.as_ref().unwrap(),
                            module,
                            entry_point: "cs_main",
                        }
                    );
                    entry.insert(CompiledVariant::ComputeReady {
                        module: module.clone(),
                        pipeline,
                    });
                    // Return reference through the cache's lifetime
                    match entry.into_mut() {
                        CompiledVariant::ComputeReady { pipeline, .. } => pipeline,
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }
        }
        Entry::Vacant(_) => {
            panic!("variant not found — must call register_material first");
        }
    }
}
```

### 4.5 Cache Lookup During Rendering

During pass execution, the renderer:

1. Gets the eval point index from the pass (known at pass construction)
2. Iterates the indirect draw buffer / dispatch grid
3. For each draw/dispatch, the material ID comes from instance data or
   `template_id_tex`
4. Looks up `(template_id, graph_hash, feature_flags)` in the per-eval-point
   cache
5. Sets the pipeline (either `RenderPipeline` for fragment or
   `ComputePipeline` for compute)
6. Binds the eval point's bind groups
7. Issues the draw/dispatch

```rust
fn bind_material_variant(
    pass_ctx: &mut PassContext,
    material_id: u32,
    eval_point_index: u32,
) {
    let key = (material_id, 0u64, 0u32); // graph_hash=0, features=0
    let cache = &pass_ctx.variant_cache.per_eval[eval_point_index as usize];

    let variant = cache.variants.get(&key)
        .expect("material variant must be registered");

    match variant {
        CompiledVariant::FragmentReady { pipeline, .. } => {
            pass_ctx.rpass.set_pipeline(pipeline);
        }
        CompiledVariant::ComputeReady { pipeline, .. } => {
            pass_ctx.cpass.set_pipeline(pipeline);
        }
        CompiledVariant::ComputeModuleReady { .. } => {
            // Should not happen: we should have upgraded to ComputeReady
            // before dispatching. But handle gracefully.
            panic!("ComputeModuleReady encountered during dispatch — call get_or_create first");
        }
    }
}
```

### 4.6 Cache Invalidation

The variant cache is invalidated when:

| Event | Action |
|---|---|
| Material re-registered (hot-reload) | Old variant removed from all per-eval caches, new variants inserted. Old pipelines are dropped (GPU freed when nothing references them). |
| Eval point base shader changes | All variants for that eval point are invalidated. This is rare (only happens when a pass changes its base shader). |
| Device lost | Entire cache is flushed. Pipelines are re-created on the new device. |
| Material removed (§6) | Variants removed from all per-eval caches. |

### 4.7 Thread Safety

The variant cache is **not `Sync`**. It is accessed only from the main
WGPU thread during rendering and material registration. This avoids
any locking overhead in the hot path.

```rust
// This is intentional:
// impl !Sync for ShaderVariantCache {}
```

---

## 5. Backward Compatibility Shim

### 5.1 v1 API Mapping

The v1 API had two registration methods:

```rust
// v1 API (RadiantTemplateRegistry)
pub fn register_str(&mut self, name: &str, wgsl_source: &str) -> u32;
pub fn register_partial_str(&mut self, name: &str, wgsl_source: &str) -> u32;
```

Both targeted the gbuffer pass exclusively. In v2, these are shimmed:

```rust
// ── v2 backward compatibility shim ──────────────────────────────────────

/// v1 compatibility: register a material targeting only the gbuffer eval point.
///
/// This is equivalent to:
/// ```ignore
/// renderer.register_material(name, wgsl, &["gbuffer"])
/// ```
///
/// # Deprecated
/// Use `register_material(name, wgsl, &["gbuffer"])` instead.
/// This shim will be removed in v3.0.
#[deprecated(since = "2.0", note = "use register_material(name, wgsl, &[\"gbuffer\"])")]
pub fn register_str(&mut self, name: &str, wgsl_source: &str) -> MaterialClassId {
    log::debug!(
        "register_str('{name}') is deprecated — targeting gbuffer only. \
         Use register_material with explicit targets."
    );
    self.register_material(name, wgsl_source, &["gbuffer"])
}

/// v1 compatibility: register a partial shader string targeting gbuffer.
///
/// There is no semantic difference between `register_str` and
/// `register_partial_str` in v2 — both call `register_material`.
/// In v1, `register_partial_str` did not include bindings; in v2,
/// the base shader provides bindings always.
#[deprecated(since = "2.0", note = "use register_material(name, wgsl, &[\"gbuffer\"])")]
pub fn register_partial_str(&mut self, name: &str, wgsl_source: &str) -> MaterialClassId {
    log::debug!(
        "register_partial_str('{name}') is deprecated — targeting gbuffer only."
    );
    self.register_material(name, wgsl_source, &["gbuffer"])
}
```

### 5.2 Behavioral Differences from v1

| Aspect | v1 (`register_partial_str`) | v2 Shim |
|---|---|---|
| Return type | `u32` | `MaterialClassId` (newtype, but `.as_u32()` for migration) |
| Target pass | gbuffer only (hardcoded) | gbuffer only (explicit `&["gbuffer"]`) |
| Base shader | Passed partial WGSL, engine prepended bindings | Eval point's base shader provides bindings |
| Compilation | Eager, no lazy option | Eager for fragment, lazy for compute (but gbuffer is fragment, so eager) |
| Mask computation | None (material_class_ranges on CPU) | `eval_point_mask` set in material record |
| Deprecation warning | N/A | `log::debug!` level (not loud, but discoverable) |

### 5.3 Migration Detection

To help identify remaining v1 API usage in the codebase:

```rust
// At renderer startup:
fn check_for_v1_usage(&self) {
    let v1_call_count = ATOMIC_V1_CALL_COUNT.load(Ordering::Relaxed);
    if v1_call_count > 0 {
        log::warn!(
            "{} calls to deprecated v1 material registration API detected. \
             Migrate to register_material(name, wgsl, &[...]) for v2 features.",
            v1_call_count
        );
    }
}
```

Each shim call increments an atomic counter. At the end of the first frame,
if the counter is non-zero, a warning is emitted.

### 5.4 What `register_str` Does NOT Support

The v1 shim does NOT support:

- Multiple eval points (glass, water, etc.)
- Compute passes (SSR, SSS)
- Transparent pass custom materials
- The `graph_hash` feature (always 0)
- `feature_flags` stripping (always 0)

Materials that need these features must use the v2 `register_material` API.

### 5.5 Internal Registry Mapping

The old `RadiantTemplateRegistry` type still exists internally but is
relegated to a helper within the per-eval-point cache:

```rust
// v2 internal: no longer a top-level API type.
// Exists only as the variant cache infrastructure.
pub(crate) struct RadiantTemplateRegistry {
    // Now just an alias for PerEvalPointCache
}
```

---

## 6. template ID Management

### 6.1 ID Allocation Algorithm

template IDs are allocated from a monotonically increasing counter
with a **free list** for reclaimed IDs.

```rust
struct MaterialIdManager {
    /// Monotonically increasing counter for fresh IDs.
    next_id: u32,

    /// Free list of IDs reclaimed from removed materials.
    /// Sorted in ascending order so we reuse the lowest available ID first.
    free_list: Vec<u32>,

    /// Maximum ID ever allocated (for bounds checking on GPU dispatch table).
    max_allocated: u32,
}

impl MaterialIdManager {
    fn allocate(&mut self) -> u32 {
        if let Some(recycled) = self.free_list.pop() {
            // Reuse the most recently freed ID (LIFO — better cache behavior)
            recycled
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.max_allocated = self.max_allocated.max(id);
            id
        }
    }

    fn free(&mut self, id: u32) {
        // Insert in sorted order to maintain LIFO reuse
        if let Err(pos) = self.free_list.binary_search(&id) {
            self.free_list.insert(pos, id);
        }
        // Do NOT decrement next_id — IDs are never truly "reused" until freed
    }
}
```

### 6.2 ID Stability Rules

| Scenario | ID Behavior | Rationale |
|---|---|---|
| Fresh registration (new material) | New ID allocated | No previous ID to reuse |
| Registration with same name (hot-reload) | **Same ID if possible** | Existing GpuInstanceData references this ID; changing it would require a full scene rebuild |
| Material removed | ID goes to free list | Can be reused by a later registration |
| Engine restart | IDs start from 0 | No persistent state |
| Two materials with same name registered sequentially without removal | Different IDs | They are different registrations |
| Material re-registered with different targets | **Same ID**, cache invalidated | Scene data references the ID; only the variant cache changes |

### 6.3 Hot-Reload ID Stability

The critical requirement: **template IDs must not change during
hot-reload**. Scene data (GpuInstanceData) is not rebuilt during hot-reload
— only shaders and material records are refreshed. If the ID changed, the
GPU instance data would reference the wrong material.

```rust
fn handle_material_hot_reload(&mut self, name: &str, new_wgsl: &str, new_targets: &[&str]) {
    // Find existing material record by name
    if let Some(existing) = self.material_registry.get_by_name(name) {
        let class_id = existing.class_id;

        // Remove old variants from all per-eval-point caches
        self.remove_material_variants(class_id);

        // Re-register with the SAME class_id
        self.register_material_with_id(name, new_wgsl, new_targets, class_id);

        log::debug!("hot-reloaded material '{name}' — class_id={} preserved", class_id.as_u32());
    } else {
        // New material, allocate fresh ID
        self.register_material(name, new_wgsl, new_targets);
    }
}
```

The `register_material_with_id` internal method is identical to
`register_material` but skips ID allocation and uses the provided ID.

### 6.4 ID Overflow

With `u32` IDs, there are ~4 billion possible values. Even at 1000 materials
per frame with hot-reload, this would take:

```
4_294_967_296 IDs ÷ 1000 materials/frame ≈ 4.3 million frames
≈ 4.3M / 60fps ≈ 71,582 hours ≈ 8.17 years
```

Overflow is not a practical concern. Nevertheless, `allocate()` panics
if `next_id` wraps to `MAX_ID` without any free list entries:

```rust
fn allocate(&mut self) -> u32 {
    if let Some(recycled) = self.free_list.pop() {
        return recycled;
    }
    if self.next_id == u32::MAX {
        panic!("template ID exhaustion — all 2^32 IDs allocated");
    }
    let id = self.next_id;
    self.next_id += 1;
    self.max_allocated = self.max_allocated.max(id);
    id
}
```

### 6.5 ID Reuse After Material Removal

When a material is removed:

1. Its `MaterialClassId` is freed back to the free list
2. All variants in per-eval-point caches for that ID are removed
3. `GpuTemplateDispatch` table entries for that ID are zeroed
4. New material registrations may recycle the ID

```rust
fn remove_material(&mut self, class_id: MaterialClassId) {
    // 1. Remove from registry
    self.material_registry.remove(class_id);

    // 2. Remove all variants from per-eval-point caches
    for eval_cache in &mut self.variant_cache.per_eval {
        eval_cache.variants.retain(|&(tid, ..), _| tid != class_id.as_u32());
    }

    // 3. Zero the dispatch table entry
    if let Some(dispatch) = self.gpu_template_dispatches.get_mut(class_id.as_u32() as usize) {
        *dispatch = GpuTemplateDispatch::default();
    }

    // 4. Free the ID
    self.id_manager.free(class_id.as_u32());

    log::debug!("removed material class_id={}", class_id.as_u32());
}
```

### 6.6 ID Collision Safety

Because IDs are allocated from a single counter (with free list), and no
two materials can have the same name simultaneously, ID collisions are
impossible under normal operation. The `material_registry` enforces
unique names at registration time:

```rust
fn register_material(..., targets, ...) -> MaterialClassId {
    // Enforce unique names at any given time
    if self.material_registry.has_name(name) {
        panic!("material name '{name}' already registered");
    }
    // ...
}
```

Hot-reload bypasses this check intentionally (it replaces the existing
registration).

### 6.7 Maximum Active Materials

The GPU dispatch table (`GpuTemplateDispatch[]`) is sized to
`max_allocated + 1`. This is updated whenever `max_allocated` grows:

```rust
fn ensure_dispatch_table_size(&mut self) {
    let needed = self.id_manager.max_allocated as usize + 1;
    if self.gpu_template_dispatches.len() < needed {
        self.gpu_template_dispatches.resize(needed, GpuTemplateDispatch::default());
        // Re-upload to GPU if buffer needs resizing
        self.upload_dispatch_table();
    }
}
```

---

## 7. Dispatch Mask Computation

### 7.1 Mask Construction During Registration

During `register_material()`, after all targets are validated, the
`eval_point_mask` is computed:

```rust
fn compute_eval_point_mask(
    eval_registry: &EvalPointRegistry,
    targets: &[&str],
) -> u32 {
    let mut mask = 0u32;
    for target_name in targets {
        let idx = eval_registry
            .index(target_name)
            .expect("target eval point must exist (validated earlier)");
        mask |= 1u32 << idx;
    }
    mask
}
```

**Constraint**: `targets.len() <= 32`. Since WGSL bitwise operations on
u32 are well-defined, and the mask is a u32, at most 32 eval points can
exist. This is enforced by a check in `EvalPointRegistry::register()`:

```rust
impl EvalPointRegistry {
    pub fn register(&mut self, point: EvalPoint) -> EvalPointId {
        if self.points.len() >= 32 {
            panic!(
                "cannot register eval point '{}': maximum 32 eval points reached",
                point.name
            );
        }
        // ...
    }
}
```

### 7.2 Mask Storage

The mask is stored in two places:

1. **CPU side**: `MaterialRecord.eval_point_mask: u32`
2. **GPU side**: Embedded in `GpuInstanceData.eval_point_mask` (for fragment)
   or in `GpuTemplateDispatch.eval_point_mask` (for compute)

### 7.3 Propagation to GpuInstanceData

During `rebuild_instance_buffers()` (called when the scene changes), every
instance gets its material's mask:

```rust
fn rebuild_instance_buffers(&mut self, scene: &Scene) {
    let instances: Vec<GpuInstanceData> = scene.drawables.iter().map(|drawable| {
        let material_class = drawable.material_class_id;
        let mask = self.get_material_mask(material_class);

        GpuInstanceData {
            model: drawable.transform.to_cols_array(),
            normal_mat: drawable.normal_matrix.to_cols_array_2d(),
            bounds: drawable.bounds.to_array(),
            mesh_id: drawable.mesh_id,
            material_id: material_class.as_u32(),
            flags: drawable.flags.bits(),
            lightmap_index: drawable.lightmap_index,
            eval_point_mask: mask,  // ← baked in at rebuild time
        }
    }).collect();

    self.gpu_scene
        .instance_buffer
        .write(&instances, 0, instances.len());
}
```

**Key invariant**: The mask is set once per instance per scene rebuild.
If a material's targets change (via hot-reload), the mask update takes
effect on the next scene rebuild, not before. This avoids the need for
per-frame mask uploads.

### 7.4 Propagation to GpuTemplateDispatch (Compute)

The dispatch table is uploaded once per scene change:

```rust
fn upload_dispatch_table(&mut self) {
    let entries: Vec<GpuTemplateDispatch> = (0..self.id_manager.max_allocated + 1)
        .map(|id| {
            let mask = self
                .material_registry
                .get_by_id(MaterialClassId(id))
                .map(|m| m.eval_point_mask)
                .unwrap_or(0);
            GpuTemplateDispatch {
                eval_point_mask: mask,
                _pad: [0u32; 3],
            }
        })
        .collect();

    self.gpu_scene
        .material_dispatch_buffer
        .write(&entries, 0, entries.len());
}
```

### 7.5 Bit Layout Example

| Bit | Eval Point | Example Material Mask |
|---|---|---|
| 0 | `gbuffer` | PBR: `0b_0000_0001` (0x01) |
| 1 | `transparent` | Water: `0b_0000_0010` (0x02) |
| 2 | `shadow` | All: `0b_0000_0100` (0x04) |
| 3 | `depth_prepass` | All: `0b_0000_1000` (0x08) |
| 4 | `ssr` | Glass: `0b_0001_0001` (0x11 = gbuffer + ssr) |
| 5 | `sss` | Skin: `0b_0010_0001` (0x21 = gbuffer + sss) |
| 6 | `decal` | Decal: `0b_0100_0010` (0x42 = transparent + decal) |
| 7 | `post_process` | Full-screen: varies |
| 8..31 | Third-party passes | Reserved |

**Glass material mask**: targets `gbuffer` + `transparent`:
- `mask = (1 << 0) | (1 << 1) = 0x03`

**Basic PBR mask**: targets only `gbuffer`:
- `mask = (1 << 0) = 0x01`

### 7.6 Mask Checking on GPU

**Fragment path** (in vertex shader output → flat interpolated → fragment shader):

```wgsl
// Fragment shader for any eval point
// THIS_EVAL_POINT is a shader #define inserted by the composition system
// (e.g., `const THIS_EVAL_POINT = 0u;` for gbuffer)

fn fs_main(input: VertexOutput) -> ... {
    // Single instruction, predicated.
    // For the gbuffer pass, THIS_EVAL_POINT = 0.
    // For basic PBR, mask = 0x01. (0x01 & (1 << 0)) = 0x01 → pass.
    // For water in gbuffer, mask = 0x02. (0x02 & (1 << 0)) = 0 → discard.
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }
    // ... evaluate material ...
}
```

**Compute path** (per-pixel through template_id_tex):

```wgsl
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;

    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        return; // Early exit for non-targeting pixels
    }
    // ... evaluate material ...
}
```

### 7.7 `THIS_EVAL_POINT` Constant Insertion

The composition system inserts a compile-time constant into the composed
WGSL so the mask check is a constant expression:

```rust
fn compose_with_eval_point_constant(
    base_shader: &str,
    override_src: &str,
    fn_name: &str,
    eval_point_index: u32,
) -> String {
    let composed = compose_fn_override(base_shader, override_src, fn_name);

    // Prepend the eval point constant
    format!(
        "const RADIANT_THIS_EVAL_POINT = {}u;\n{}",
        eval_point_index,
        composed
    )
}
```

The base shader's mask check uses `RADIANT_THIS_EVAL_POINT`:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << RADIANT_THIS_EVAL_POINT)) == 0u {
    discard;
}
```

This ensures the GPU compiler can evaluate `(1u << RADIANT_THIS_EVAL_POINT)`
as a compile-time constant — it becomes a single bit-test instruction.

### 7.8 Mask Update Timing

```
Event                                    Mask State
──────────────────────────────────────────────────────────
register_material("pbr", ..., &[gbuffer])  CPU: mask=0x01
                                           GPU: not yet uploaded
Scene rebuild                              GPU instances: mask=0x01
                                           GPU dispatch table: mask=0x01
Frame N rendering                          Fragment: mask check uses baked mask
                                           Compute: mask check uses dispatch table
Hot-reload ("pbr", new_wgsl, &[gbuffer,ssr]) CPU: mask updated to 0x11
                                           GPU: NOT yet updated (next scene rebuild)
Immediate next frame                       GPU: still 0x01 (stale but safe —
                                           old mask is subset of new, no false positives)
Scene rebuild (frame N+1)                  GPU: mask=0x11
```

The mask is **eventually consistent** across hot-reload. Between the
hot-reload and the next scene rebuild, the GPU uses the old mask, which
is a subset of the new mask. This means an SSR-enabled material won't
be evaluated in the SSR pass until the next scene rebuild — but it will
never produce incorrect rendering, only slightly less optimal rendering
(missing the SSR contribution for at most one frame).

### 7.9 Zero Mask

A zero mask (`eval_point_mask = 0`) means the material targets no eval
points. This is intentionally allowed for:

- **Placeholder materials**: Assigned to geometry that hasn't been
  assigned a real material yet (editor workflow).
- **Collision-only meshes**: Invisible geometry that only participates
  in physics.
- **Removed materials**: If a material is removed but its instances
  haven't been updated yet, the mask is 0 and all passes discard the
  geometry.

Zero-mask instances are drawn by the gbuffer pass (for depth occlusion)
but immediately discard in the fragment shader. They write nothing to
the gbuffer targets.

---

## 8. Error Handling

### 8.1 Error Classification

| Error Category | Detection Point | Severity | Recovery |
|---|---|---|---|
| Unknown eval point name | `register_material()` | Panic | Fix target names |
| Missing eval function in WGSL | `register_material()` | Panic | Add function to material WGSL |
| Multiple functions with same name | `compose_fn_override()` | Panic | Rename duplicate functions |
| WGSL compilation failure | `register_material()` variant compile | Panic | Fix WGSL syntax |
| Function not found in base shader | `compose_fn_override()` | Panic | Fix eval point base shader |
| Unterminated function body (EOF) | `compose_fn_override()` | Panic | Fix base/override shader syntax |
| Too many eval points (>32) | `register_material()` or `EvalPointRegistry::register()` | Panic | Reduce eval points or design differently |
| ID exhaustion | `MaterialIdManager::allocate()` | Panic | Will not happen in practice |
| Material name collision | `register_material()` | Panic | Remove old material first (or hot-reload) |
| WGPU device lost | Variant compilation | Recover | Flush cache, recompile on new device |
| Feature flag bit overflow | (future: `register_material()` with features) | Panic | Reduce feature flags |

### 8.2 Panic vs. Result

The v2 material registration API uses **panics** (not `Result`) for all
validation errors. Rationale:

1. **Registration errors are programmer errors**. A missing eval function
   is a bug in the material WGSL, not a runtime recoverable condition.
2. **The system has no valid fallback**. If a material can't compile for
   a required eval point, there's no "degraded" mode that would produce
   correct rendering.
3. **Consistency with WGPU's own error model**. WGPU panics on shader
   compilation failure; matching this pattern keeps the error model uniform.
4. **Easier debugging**. Panics with full context (material name, composed
   WGSL, WGPU error) are more actionable than propagating `Err` up through
   the scene graph.

### 8.3 Detailed Error Messages

#### 8.3.1 Unknown Eval Point

```
thread 'main' panicked at 'material registration failed: unknown eval point 'ssr_lite'
  material: 'custom_glass'
  available eval points: gbuffer (0), transparent (1), shadow (2), depth_prepass (3),
                        ssr (4), sss (5), decal (6), post_process (7), velocity (8)

  Did you mean 'ssr'? (suggestion via edit distance, Levenshtein ≤ 2)'
```

#### 8.3.2 Missing Eval Function

```
thread 'main' panicked at 'material registration failed: missing required function 'eval_transparent'
  material: 'custom_glass'
  targets: ["gbuffer", "transparent"]
  material WGSL defines functions: eval_gbuffer, fresnel_factor

  Fix: add 'fn eval_transparent(...) -> vec4f { ... }' to custom_glass.wgsl'
```

#### 8.3.3 WGSL Compilation Error

```
thread 'main' panicked at 'material registration failed: WGSL compilation error
  material: 'glass'
  target eval point: 'transparent' (index 1)
  base shader: transparent_base.wgsl

  WGPU error:
    ┌─ composed_shader.wgsl:89:18
    │
  89│     let color = vec4f(0.5, 0.3, 0.8, 0.6;
    │                                   ^ expected ')'

  Composed shader written to: /tmp/radiant_composed_glass_transparent.wgsl'
```

The composed WGSL is written to a temporary file for inspection. The file
path is included in the panic message.

### 8.4 WGSL Syntax Error in Override (Not Base)

When a syntax error is in the material's override code (not the base
shader), the error message should help the user locate the error in their
source:

```
thread 'main' panicked at 'WGSL compilation error in material override
  material: 'glass'
  file: assets/shaders/materials/glass.wgsl

  ┌─ [material source]:17:5
  │
  17│     return vec4f(color, 0.5
  │     ^^^^^^^^^^^^^^^^^^^^^^^^^ expected ')'

  The error is in your material override code, in function 'eval_transparent'.
  Check assets/shaders/materials/glass.wgsl around line 17.

  (Composed shader written to /tmp/radiant_composed_glass_transparent.wgsl
   for reference — the base shader is correct)
```

### 8.5 Brace Mismatch in Function Body

```
thread 'main' panicked at 'shader composition failed: unterminated function body
  phase: reading function 'eval_gbuffer' from material source
  material: 'glass'
  file: assets/shaders/materials/glass.wgsl

  The opening '{' at line 23 has no matching '}'.
  Check for unbalanced braces in 'eval_gbuffer'.

  Caused by:
  line 42:     fn helper(x: f32) -> f32 { return x * 2.0;
  (missing closing brace of helper function)'
```

### 8.6 Function Signature Mismatch

This is technically caught by WGPU compilation, not by `compose_fn_override`.
The composed shader will have:

```wgsl
// Base shader calls:
let result = eval_gbuffer(input.material_id, input.world_position,
                          input.world_normal, input.tex_coords);

// Override declares:
fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f,
                uv: vec2f, extra_param: f32) -> SurfaceData {
```

WGPU error:

```
error: too many arguments for function 'eval_gbuffer'
  ┌─ composed_shader.wgsl:142:92
  │
142│     let result = eval_gbuffer(input.material_id, input.world_position, ...
  │                                ^ 4 arguments, expected 5

  The material override declares 'fn eval_gbuffer(...)' with 5 parameters,
  but the base shader calls it with 4. The signature must match exactly.
```

### 8.7 Eval Point Index Overflow

```rust
// During EvalPointRegistry::register():
if self.points.len() >= 32 {
    panic!(
        "cannot register eval point '{point_name}': \
         maximum of 32 eval points reached (currently have {})",
        self.points.len()
    );
}
```

### 8.8 Cache Miss During Rendering

```rust
// During get_or_create_variant():
fn get_or_create_variant(&self, key: ShaderVariantKey) -> &CompiledVariant {
    let cache = &self.variant_cache.per_eval[key.eval_point as usize];
    match cache.variants.get(&(key.template_id, key.graph_hash, key.feature_flags)) {
        Some(variant) => variant,
        None => {
            // This should never happen if register_material was called.
            // But if it does (e.g., someone modified the ID manually), panic
            // with a clear explanation.
            panic!(
                "variant not cached: eval_point={} template_id={} graph_hash={} flags={}.\n\
                 This means a draw/dispatch was issued for a material that was never\n\
                 registered, or the ShaderVariantKey was constructed incorrectly.\n\
                 Check that register_material() was called for material class {}.",
                key.eval_point, key.template_id, key.graph_hash, key.feature_flags,
                key.template_id
            );
        }
    }
}
```

### 8.9 Error Recovery Paths

| Failure Point | Recovery Action |
|---|---|
| WGSL compilation during `register_material` | Clean up all variants for this material, re-panic. No partial state remains. |
| WGPU device lost mid-frame | All variant caches flushed. Material records preserved. Next `get_or_create_variant` recompiles on the new device. |
| `register_material` called before eval points exist | Panic with message "eval point registry is empty — ensure all passes are registered before materials". |
| Material removal while instances reference it | Instances with stale material IDs have mask=0 → passes discard them. Next scene rebuild removes the instances. |

### 8.10 Debug Mechanisms

```rust
// Emit all available info when a panic occurs.
// Called from the panic handler if material registration is involved.
fn debug_dump_material_state(&self) {
    log::error!("=== Material Registry State ===");
    for (id, material) in self.material_registry.iter() {
        log::error!("  class_id={}: name='{}' mask={:#010x} targets={:?}",
            id.as_u32(), material.name, material.eval_point_mask, material.targets);
    }
    log::error!("=== Eval Point Registry ===");
    for point in self.eval_registry.points() {
        log::error!("  index={}: name='{}' entry='{}'",
            point.index, point.name, point.entry_fn_signature);
    }
    log::error!("=== Variant Cache Summary ===");
    for (i, cache) in self.variant_cache.per_eval.iter().enumerate() {
        log::error!("  eval_point={}: {} variants cached", i, cache.variants.len());
    }
}
```

---

## 9. WGSL Template Examples

### 9.1 `glass.wgsl` — Multi-eval (gbuffer + transparent)

```wgsl
// glass.wgsl
// Targets: gbuffer, transparent
// Helio Radiant v2 material — provides both eval functions.
//
// The gbuffer variant writes surface parameters for SSR/lighting.
// The transparent variant composites the glass layer with fresnel alpha.

// ── Shared constants ────────────────────────────────────────────────────
// These are duplicated inside each eval function (see §3.3 for why).
// For production, this material should inline these into each function body.

// ── G-buffer eval: surface parameters for PBR lighting + SSR ────────────
fn eval_gbuffer(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    uv: vec2f
) -> SurfaceData {
    // Sample textures
    let base_color = textureSampleBaseClampToEdge(
        material_textures[material_id].albedo, sampler_linear, uv
    ).rgb;

    let roughness_metallic = textureSampleBaseClampToEdge(
        material_textures[material_id].orm, sampler_linear, uv
    );

    // Glass surface: smooth, non-metallic, low F0
    var s: SurfaceData;
    s.albedo = base_color;
    s.normal = normal;
    s.roughness = 0.02 + roughness_metallic.g * 0.1;
    s.metallic = 0.0;
    s.specular_f0 = mix(vec3f(0.04), base_color, roughness_metallic.b);
    s.emissive = vec3f(0.0);
    s.subsurface_color = vec3f(0.9, 0.9, 1.0) * 0.3;
    s.flags = SURFACE_FLAG_SUBSURFACE;
    return s;
}

// ── Transparent eval: alpha-blended composite ───────────────────────────
fn eval_transparent(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    uv: vec2f
) -> vec4f {
    // Fresnel effect for glass
    let V = normalize(camera.position_near.xyz - world_pos);
    let NdV = max(dot(normal, V), 0.0001);
    let fresnel = pow(1.0 - NdV, 4.0);

    // Sample base color with glass tint
    let base_color = textureSampleBaseClampToEdge(
        material_textures[material_id].albedo, sampler_linear, uv
    ).rgb;

    // Composite: reflection on edges, transmission in center
    let reflect_color = vec3f(0.12, 0.14, 0.18); // dark gray reflection
    let transmit_color = base_color * 0.95;

    let color = mix(transmit_color, reflect_color, fresnel);
    let alpha = mix(0.35, 0.85, fresnel); // more opaque at edges

    return vec4f(color, alpha);
}
```

### 9.2 `water_transparent.wgsl` — Single-eval (transparent only)

```wgsl
// water_transparent.wgsl
// Targets: transparent
// Helio Radiant v2 material — provides only eval_transparent.
// Does NOT participate in gbuffer (no surface data written).
// Uses depth-only from gbuffer pass, then alpha-blends in transparent pass.

// ── Transparent eval: animated water surface ────────────────────────────
fn eval_transparent(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    uv: vec2f
) -> vec4f {
    // Animate time
    let t = f32(globals.frame_index) * globals.delta_time;

    // Compute wave normal
    let wave_uv = uv * 8.0;
    let wave_offset = sin(wave_uv.x + t * 1.5) * cos(wave_uv.y + t * 1.2);
    let wave_normal = normalize(normal + vec3f(wave_offset * 0.3, 0.0, wave_offset * 0.2));

    // Fresnel
    let V = normalize(camera.position_near.xyz - world_pos);
    let NdV = max(dot(wave_normal, V), 0.0001);
    let fresnel = pow(1.0 - NdV, 5.0);

    // Deep water color, lighter at edges
    let deep = vec3f(0.005, 0.02, 0.06);
    let shallow = vec3f(0.02, 0.08, 0.15);
    let color = mix(deep, shallow, fresnel);

    // Add specular highlight from sun
    let L = normalize(globals.sun_direction.xyz);
    let half_vec = normalize(L + V);
    let spec = pow(max(dot(wave_normal, half_vec), 0.0), 512.0);
    let color = color + vec3f(0.8, 0.85, 1.0) * spec * 0.6;

    // Alpha: mostly opaque, thinner at edges
    let alpha = mix(0.85, 0.6, fresnel);

    return vec4f(color, alpha);
}
```

### 9.3 `custom_ssr.wgsl` — Single-eval (ssr compute pass only)

```wgsl
// custom_ssr.wgsl
// Targets: ssr
// Helio Radiant v2 material — provides only eval_ssr for the SSR compute pass.
// Does NOT participate in gbuffer or transparent passes.
// The SSR pass reads the gbuffer's normal/roughness and this material
// can override the SSR parameters per-pixel.

// ── SSR eval: custom ray-march parameters ───────────────────────────────
fn eval_ssr(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    roughness: f32,
    f0: vec3f
) -> SsrParams {
    // Iridescent paint — overrides roughness and F0 based on angle
    let V = normalize(camera.position_near.xyz - world_pos);
    let NdV = max(dot(normal, V), 0.0);

    // Iridescence: F0 shifts with viewing angle
    let hue = NdV * 6.28318; // full rainbow cycle
    let iridescent_f0 = vec3f(
        sin(hue) * 0.5 + 0.5,
        sin(hue + 2.094) * 0.5 + 0.5,
        sin(hue + 4.188) * 0.5 + 0.5,
    ) * 0.3 + 0.04;

    // Override SSR parameters
    var params: SsrParams;
    params.roughness = roughness * 0.5; // sharper reflections
    params.f0 = mix(f0, iridescent_f0, NdV);
    params.ray_length = 100.0; // long rays for metallic flakes
    params.thickness = 0.5;
    params.binary_search_count = 8u;
    return params;
}
```

### 9.4 `simple_pbr.wgsl` — Default Gbuffer-only (backward compatible)

```wgsl
// simple_pbr.wgsl
// Targets: gbuffer (backward compatible with v1 register_partial_str)
// Helio Radiant v2 material — standard PBR gbuffer eval.

// ── G-buffer eval: standard PBR surface ─────────────────────────────────
fn eval_gbuffer(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    uv: vec2f
) -> SurfaceData {
    let material = materials[material_id];
    let tex = material_textures[material_id];

    let albedo = textureSampleBaseClampToEdge(tex.albedo, sampler_linear, uv);
    let orm = textureSampleBaseClampToEdge(tex.orm, sampler_linear, uv);
    let normal_map = textureSampleBaseClampToEdge(tex.normal, sampler_linear, uv);

    // Decode normal from tangent space
    let TBN = construct_tbn(normal, uv, normal_map.xy);
    let world_normal = normalize(TBN * (normal_map.rgb * 2.0 - 1.0));

    var s: SurfaceData;
    s.albedo = albedo.rgb;
    s.normal = world_normal;
    s.roughness = orm.g;
    s.metallic = orm.r;
    s.specular_f0 = mix(vec3f(0.04), albedo.rgb, orm.r);
    s.emissive = material.emissive.rgb;
    s.subsurface_color = vec3f(0.0);
    s.flags = 0u;
    return s;
}
```

### 9.5 `masked_opaque.wgsl` — Gbuffer with alpha test

```wgsl
// masked_opaque.wgsl
// Targets: gbuffer
// Uses alpha-test (clip) to create cutout opacity in the gbuffer pass.
// The transparent pass is NOT targeted — this material uses the gbuffer's
// alpha test feature.

fn eval_gbuffer(
    material_id: u32,
    world_pos: vec3f,
    normal: vec3f,
    uv: vec2f
) -> SurfaceData {
    let material = materials[material_id];
    let tex = material_textures[material_id];

    let albedo = textureSampleBaseClampToEdge(tex.albedo, sampler_linear, uv);

    // Alpha test: discard pixels below threshold
    // This runs inside the gbuffer fragment shader, before the normal
    // material evaluation continues.
    let alpha = albedo.a;
    if (alpha < 0.5) {
        discard; // ← note: this discard is INSIDE the eval function,
                 //   which is inside the gbuffer's fs_main. WGSL allows
                 //   discard anywhere in a fragment shader.
    }

    let orm = textureSampleBaseClampToEdge(tex.orm, sampler_linear, uv);

    var s: SurfaceData;
    s.albedo = albedo.rgb;
    s.normal = normal;
    s.roughness = orm.g;
    s.metallic = orm.r;
    s.specular_f0 = mix(vec3f(0.04), albedo.rgb, orm.r);
    s.emissive = vec3f(0.0);
    s.subsurface_color = vec3f(0.0);
    s.flags = SURFACE_FLAG_ALPHA_TEST;
    return s;
}
```

---

## Appendix A: Composition Output Example

To illustrate what `compose_fn_override` produces, here is the transparent
base shader with the glass material's `eval_transparent` body inserted:

### Input: `transparent_base.wgsl` (simplified)

```wgsl
struct Camera { position_near: vec4f; ... };
struct Globals { frame_index: u32; delta_time: f32; sun_direction: vec4f; ... };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> globals: Globals;

fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f {
    return vec4f(0.3, 0.3, 0.4, 0.5);
}

struct VertexOutput { ... };

@vertex fn vs_main(...) -> VertexOutput { ... }

@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4f {
    // RADIANT_OVERRIDE_START
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << RADIANT_THIS_EVAL_POINT)) == 0u {
        discard;
    }
    return eval_transparent(
        input.material_id, input.world_position,
        input.world_normal, input.tex_coords
    );
    // RADIANT_OVERRIDE_END
}
```

### Override source: glass.wgsl (eval_transparent body only)

```wgsl
    // (extracted body of fn eval_transparent from glass.wgsl)
    var V: vec3f = normalize(camera.position_near.xyz - world_pos);
    var NdV: f32 = max(dot(normal, V), 0.0001);
    var fresnel: f32 = pow(1.0 - NdV, 4.0);
    var color: vec3f = mix(vec3f(0.95, 0.96, 0.97), vec3f(0.12, 0.14, 0.18), fresnel);
    var alpha: f32 = mix(0.35, 0.75, fresnel);
    return vec4f(color, alpha);
```

### Output: composed shader

```wgsl
const RADIANT_THIS_EVAL_POINT = 1u; // transparent eval point index

struct Camera { position_near: vec4f; ... };
struct Globals { frame_index: u32; delta_time: f32; sun_direction: vec4f; ... };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> globals: Globals;

fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f {
    // ══ OVERRIDE BODY START (from glass.wgsl) ══
    var V: vec3f = normalize(camera.position_near.xyz - world_pos);
    var NdV: f32 = max(dot(normal, V), 0.0001);
    var fresnel: f32 = pow(1.0 - NdV, 4.0);
    var color: vec3f = mix(vec3f(0.95, 0.96, 0.97), vec3f(0.12, 0.14, 0.18), fresnel);
    var alpha: f32 = mix(0.35, 0.75, fresnel);
    return vec4f(color, alpha);
    // ══ OVERRIDE BODY END ══
}

struct VertexOutput { ... };

@vertex fn vs_main(...) -> VertexOutput { ... }

@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4f {
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << RADIANT_THIS_EVAL_POINT)) == 0u {
        discard;
    }
    return eval_transparent(
        input.material_id, input.world_position,
        input.world_normal, input.tex_coords
    );
}
```

---

## Appendix B: `compose_fn_override` Reference Implementation

```rust
/// Compose a material's override function into a base shader.
///
/// This is the core text-level composition: it finds the function named
/// `fn_name` in both the base shader and the override source, and replaces
/// the base's function body with the override's function body.
///
/// # Panics
/// - If `fn_name` is not found in either source.
/// - If more than one function named `fn_name` exists in either source.
/// - If a function body is unterminated (unbalanced braces).
pub fn compose_fn_override(
    base_shader: &str,
    override_src: &str,
    fn_name: &str,
) -> String {
    let (base_body_start, base_body_end) = find_fn_body(base_shader, fn_name)
        .expect("base shader must contain fn '{fn_name}'");
    let (override_body_start, override_body_end) = find_fn_body(override_src, fn_name)
        .expect("override source must contain fn '{fn_name}'");

    let mut result = String::with_capacity(
        base_shader.len() - (base_body_end - base_body_start) + (override_body_end - override_body_start)
    );
    result.push_str(&base_shader[..base_body_start]);
    result.push_str(&override_src[override_body_start..override_body_end]);
    result.push_str(&base_shader[base_body_end..]);
    result
}

/// Find the body range (after `{` to before `}`) of a function by name.
///
/// Returns `(body_start, body_end)` where `body_start` is the byte offset
/// of the first character AFTER the opening `{`, and `body_end` is the
/// byte offset of the closing `}` (i.e., the range `[body_start..body_end]`
/// contains the inner body without braces).
///
/// The search uses line-start regex: `^fn <fn_name>(` to avoid matching
/// nested or commented references.
fn find_fn_body(source: &str, fn_name: &str) -> Option<(usize, usize)> {
    let re = Regex::new(&format!(r"(?m)^(?:\s*)fn {fn_name}\s*\("))
        .expect("valid regex");

    let mut matches = re.find_iter(source);
    let fn_match = match (matches.next(), matches.next()) {
        (Some(m), None) => m,
        (None, _) => return None,        // not found
        (Some(_), Some(_)) => {
            panic!("multiple functions named '{fn_name}' found in source");
        }
    };

    // Scan forward from fn declaration to find opening `{`.
    // Track paren depth to avoid matching `{` inside parameter lists.
    let search_start = fn_match.start();
    let mut paren_depth = 0u32;
    let mut found_brace = false;
    let mut body_start = 0usize;
    let mut i = fn_match.end(); // start after `fn name(`

    // First find the opening {
    for (byte_offset, ch) in source[search_start..].char_indices() {
        let offset = search_start + byte_offset;
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '{' if paren_depth == 0 => {
                body_start = offset + ch.len_utf8(); // after the `{`
                found_brace = true;
                i = offset + ch.len_utf8();
                break;
            }
            '/' => {
                // Skip comments so braces inside comments don't confuse tracking
                let remaining = &source[offset..];
                if remaining.starts_with("//") {
                    // Skip to end of line
                    if let Some(nl) = remaining.find('\n') {
                        // Advance i to after newline
                        // Note: this is a simplified version; real impl needs
                        // to update the iterator position
                    }
                } else if remaining.starts_with("/*") {
                    // Skip to */
                    if let Some(end) = remaining.find("*/") {
                        // Advance past */
                    }
                }
            }
            _ => {}
        }
    }

    if !found_brace {
        panic!("function '{fn_name}' has no opening '{{'");
    }

    // Now find matching closing } using brace-depth tracking
    let mut brace_depth = 1u32;
    let mut body_end = source.len();
    let mut chars = source[i..].char_indices().peekable();

    while let Some((byte_offset, ch)) = chars.next() {
        let offset = i + byte_offset;
        match ch {
            '{' => brace_depth += 1,
            '}' => {
                brace_depth -= 1;
                if brace_depth == 0 {
                    body_end = offset;
                    break;
                }
            }
            '/' => {
                // Comment skipping (simplified; real impl handles both // and /*)
                let remaining = &source[offset..];
                if remaining.starts_with("//") {
                    while let Some(&(_, c)) = chars.peek() {
                        if c == '\n' { chars.next(); break; }
                        chars.next();
                    }
                } else if remaining.starts_with("/*") {
                    while let Some(&(_, c)) = chars.peek() {
                        if c == '*' && remaining.as_bytes().get(chars.peek().map(|(i,_)| i+1).unwrap_or(0)) == Some(&b'/') {
                            chars.next(); chars.next(); break;
                        }
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }

    if brace_depth != 0 {
        panic!("unterminated function body for '{fn_name}' (reached EOF with brace_depth={brace_depth})");
    }

    Some((body_start, body_end))
}
```

---

## Appendix C: Data Structures Reference

```rust
// ── Core types ──────────────────────────────────────────────────────────

/// Opaque template ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialClassId(u32);

impl MaterialClassId {
    pub const INVALID: Self = Self(u32::MAX);
    pub fn as_u32(self) -> u32 { self.0 }
}

/// Material record stored in the engine's material registry.
struct MaterialRecord {
    /// Human-readable name (must be unique at any given time).
    pub name: String,

    /// Original WGSL source provided by the material author.
    pub wgsl_source: String,

    /// Eval point names this material targets.
    pub targets: Vec<String>,

    /// Assigned class ID.
    pub class_id: MaterialClassId,

    /// Computed dispatch mask bitmask.
    pub template_id: u32,
}

/// Per-eval-point cached variant.
enum CompiledVariant {
    FragmentReady {
        module: wgpu::ShaderModule,
        pipeline: wgpu::RenderPipeline,
    },
    ComputeReady {
        module: wgpu::ShaderModule,
        pipeline: wgpu::ComputePipeline,
    },
}

/// Per-eval-point variant cache (one per eval point index).
struct PerEvalPointCache {
    /// Map: (template_id, graph_hash, feature_flags) → variant.
    pub variants: HashMap<(u32, u64, u32), CompiledVariant>,
}

// ── GPU-facing structures (reproduced for completeness) ────────────────

#[repr(C)]
struct GpuInstanceData {
    pub model: [f32; 16],
    pub normal_mat: [f32; 12],
    pub bounds: [f32; 4],
    pub mesh_id: u32,
    pub material_id: u32,
    pub flags: u32,
    pub lightmap_index: u32,
    pub template_id: u32,  // ← set from MaterialRecord.eval_point_mask
}

#[repr(C)]
struct GpuTemplateDispatch {
    pub template_id: u32,
    pub _pad: [u32; 3],
}

// ── ShaderVariantKey ────────────────────────────────────────────────────

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
#[repr(C)]
struct ShaderVariantKey {
    pub eval_point: u32,
    pub template_id: u32,
    pub graph_hash: u64,
    pub feature_flags: u32,
}

// ── Material registry ───────────────────────────────────────────────────

struct MaterialRegistry {
    /// Indexed by MaterialClassId.as_u32().
    records: Vec<Option<MaterialRecord>>,

    /// Name → class_id lookup for hot-reload.
    by_name: HashMap<String, MaterialClassId>,

    /// ID allocator/free-list.
    id_manager: MaterialIdManager,
}
```

---


> **Part of the Radiant v2 Material Pipeline Specification**  
> **File**: `spec-parts/03-instance-dispatch.md`  
> **Status**: Draft  
> **Design goal**: Eliminate CPU-side material class range iteration by embedding the eval point dispatch mask directly in per-instance GPU data, making per-pass filtering a zero-cost GPU operation.

---

## Table of Contents

1. [GpuInstanceData Changes](#1-gpuinstancedata-changes)
2. [Scene Rebuild](#2-scene-rebuild)
3. [Material Dispatch Mask Propagation](#3-material-dispatch-mask-propagation)
4. [Fragment Path: Vertex to Fragment](#4-fragment-path-vertex--fragment)
5. [Fragment Path: The Discard Pattern](#5-fragment-path-the-discard-pattern)
6. [Compute Path: Template ID texture](#6-compute-path-material-class-texture)
7. [Compute Path: Dispatch Table](#7-compute-path-dispatch-table)
8. [Comparison: Fragment vs Compute Path](#8-comparison-fragment-vs-compute-path)
9. [Indirect Draw Batching](#9-indirect-draw-batching)
10. [Vertex Processing Cost](#10-vertex-processing-cost)

---

## 1. GpuInstanceData Changes

### 1.1 The New Field

A single `u32` field — `eval_point_mask` — is appended to `GpuInstanceData`. It replaces what was previously implicit tail padding after `lightmap_index`.

### 1.2 Memory Layout — Before (v1)

```rust
#[repr(C)]
struct GpuInstanceData {              // offset     size
    model: [f32; 16],                  // 0          64
    normal_mat: [f32; 12],             // 64         48
    bounds: [f32; 4],                  // 112        16
    mesh_id: u32,                      // 128        4
    material_id: u32,                  // 132        4
    flags: u32,                        // 136        4
    lightmap_index: u32,               // 140        4
    // IMPLICIT TAIL PADDING           // 144        12     (to align struct to 16 bytes)
} // size = 156 (with 12 bytes padding → 156 + 4 = 160 ≡ 0 mod 16)
```

**Alignment analysis (v1):**

| Field | Type | Align | Offset | Size | Gap after |
|---|---|---|---|---|---|
| `model` | `[f32; 16]` | 16 (vec4) | 0 | 64 | 0 |
| `normal_mat` | `[f32; 12]` | 16 (vec4 × 3) | 64 | 48 | 0 |
| `bounds` | `[f32; 4]` | 16 (vec4) | 112 | 16 | 0 |
| `mesh_id` | `u32` | 4 | 128 | 4 | 0 |
| `material_id` | `u32` | 4 | 132 | 4 | 0 |
| `flags` | `u32` | 4 | 136 | 4 | 0 |
| `lightmap_index` | `u32` | 4 | 140 | 4 | 0 |
| *(tail padding)* | — | — | 144 | **12** | — |

Total: 140 bytes of data + 12 bytes tail padding = **152 bytes** by C ABI count. In practice, the tight packing of `u32` fields (align 4) means the struct ends at byte 144, and the struct alignment of 16 (required by `vec4<f32>` in the GPU version) forces padding to the next multiple of 16 → **160 bytes on GPU**.

### 1.3 Memory Layout — After (v2)

```rust
#[repr(C)]
struct GpuInstanceData {              // offset     size
    model: [f32; 16],                  // 0          64
    normal_mat: [f32; 12],             // 64         48
    bounds: [f32; 4],                  // 112        16
    mesh_id: u32,                      // 128        4
    material_id: u32,                  // 132        4
    flags: u32,                        // 136        4
    lightmap_index: u32,               // 140        4
    template_id:     u32,              // 144        4     ← NEW
    // IMPLICIT TAIL PADDING           // 148        12
} // size = 148 (with 12 bytes padding → 148 + 12 = 160 ≡ 0 mod 16)
```

| Field | Type | Align | Offset | Size | Gap after |
|---|---|---|---|---|---|
| `model` | `[f32; 16]` | 16 | 0 | 64 | 0 |
| `normal_mat` | `[f32; 12]` | 16 | 64 | 48 | 0 |
| `bounds` | `[f32; 4]` | 16 | 112 | 16 | 0 |
| `mesh_id` | `u32` | 4 | 128 | 4 | 0 |
| `material_id` | `u32` | 4 | 132 | 4 | 0 |
| `flags` | `u32` | 4 | 136 | 4 | 0 |
| `lightmap_index` | `u32` | 4 | 140 | 4 | 0 |
| `eval_point_mask` | `u32` | 4 | 144 | 4 | 0 |
| *(tail padding)* | — | — | 148 | **12** | — |

**Net struct size increase: 0 bytes** on GPU (still 160 bytes) because `eval_point_mask` occupies what was tail padding. The Rust-side `#[repr(C)]` struct grows from 140 → 144 bytes of actual data, but the GPU buffer stride remains 160 bytes. The new data fits entirely within the pre-existing allocation.

### 1.4 Why This Is Alignment-Safe

The key question: does inserting `u32` at position 144 require additional internal padding?

- The previous field (`lightmap_index`) ends at offset 140.
- `u32` requires alignment 4.
- 144 is divisible by 4. ✓ — no padding needed.
- The struct itself requires alignment 16 (because `vec4<f32>` fields require 16-byte alignment for the matrix rows).
- 144 bytes of data + 4 bytes for `eval_point_mask` = 148.
- 12 bytes of tail padding pushes the total to 160 ≡ 0 mod 16. ✓

**The field fits without changing any existing offsets.** Every existing field's offset is identical between v1 and v2. The only change is that 4 bytes of formerly wasted padding now carry useful data.

### 1.5 Platform Considerations

| Platform | Rust `repr(C)` struct size | GPU buffer stride | Change |
|---|---|---|---|
| x86-64 Windows/Linux/macOS | 144 bytes (was 140) | 160 bytes (unchanged) | +4 bytes data, 0 bytes stride |
| ARM64 (Apple Silicon) | 144 bytes | 160 bytes | Same |
| WASM (WebGPU) | 148 bytes (WGSL struct) | 160 bytes | GPU stride unchanged |
| 32-bit (hypothetical) | 144 bytes | 160 bytes | Same |

The case where `size_of<GpuInstanceData>()` changes between platforms is harmless because GPU buffer strides are always explicit (`wgpu::BufferBinding.size` or the stride in vertex buffer layout). The CPU-side and GPU-side structs are independently defined; alignment is handled by the #[repr(C)] + WGSL struct layout rules.

### 1.6 WGSL Struct Definition — Before

```wgsl
struct GpuInstanceData {
    model:            mat4x4<f32>,       // offset 0,   size 64
    normal_mat:       array<vec4<f32>, 3>,// offset 64,  size 48
    bounds:           vec4<f32>,          // offset 112, size 16
    mesh_id:          u32,                // offset 128, size 4
    material_id:      u32,                // offset 132, size 4
    flags:            u32,                // offset 136, size 4
    lightmap_index:   u32,                // offset 140, size 4
    // size = 144 (WGSL rounds up to 16-byte stride → 160)
}
```

### 1.7 WGSL Struct Definition — After

```wgsl
struct GpuInstanceData {
    model:            mat4x4<f32>,       // offset 0,   size 64
    normal_mat:       array<vec4<f32>, 3>,// offset 64,  size 48
    bounds:           vec4<f32>,          // offset 112, size 16
    mesh_id:          u32,                // offset 128, size 4
    material_id:      u32,                // offset 132, size 4
    flags:            u32,                // offset 136, size 4
    lightmap_index:   u32,                // offset 140, size 4
    template_id:     u32,                // offset 144, size 4 ← NEW
    // size = 148 (WGSL rounds up to 16-byte stride → 160)
}
```

**No other WGSL struct is affected.** The vertex input binding for the instance buffer includes the new field at the correct offset.

---

## 2. Scene Rebuild

### 2.1 The `rebuild_instance_buffers` Function

This is the CPU-side function that runs whenever the scene's instance list changes (entity transforms updated, materials swapped, objects added/removed). It is called from the renderer's frame update path after `world_updated` is signaled.

### 2.2 v1 Algorithm

```
fn rebuild_instance_buffers(v1):
    ┌─ 1. Collect all visible instances from scene
    ├─ 2. Build an array of (class, graph_hash, mesh_id, material_id, instance_data)
    ├─ 3. Sort by (class, graph_hash, mesh_id, material_id)
    │     └─ Primary key: class   (template ID)
    │     └─ Secondary: graph_hash (shader variant)
    │     └─ Tertiary: mesh_id    (vertex buffer binding)
    │     └─ Quaternary: material_id (material parameter binding)
    ├─ 4. Build instance buffer data (sorted array of GpuInstanceData)
    ├─ 5. Build material_class_ranges:
    │     └─ Iterate sorted array
    │     └─ For each run of identical (class, graph_hash):
    │         - Push (class, graph_hash, start_index, instance_count)
    │     └─ Result: Vec<(MaterialClass, GraphHash, u32, u32)>
    ├─ 6. Upload instance buffer to GPU
    ├─ 7. Upload class ranges buffer to GPU (for indirect dispatch)
    └─ 8. Build multi-draw indirect command list (one draw per range)
```

**Key property:** The v1 algorithm produces N draw calls (one per range), where N = number of distinct (class, graph_hash) tuples present in the scene. Each pass that wants to evaluate materials must:
1. Iterate `material_class_ranges` on CPU
2. Filter for classes that the pass cares about
3. For each matching range, bind the correct pipeline and issue a draw

### 2.3 v2 Algorithm

```
fn rebuild_instance_buffers(v2):
    ┌─ 1. Collect all visible instances from scene
    ├─ 2. Build an array of (graph_hash, mesh_id, material_id, instance_data, material_idx)
    ├─ 3. Sort by (graph_hash, mesh_id, material_id)
    │     └─ Primary key: graph_hash   (shader variant grouping)
    │     └─ Secondary: mesh_id        (vertex buffer binding)
    │     └─ Tertiary: material_id     (material parameter binding)
    │     └─ NO CLASS-BASED SORTING
    ├─ 4. Build instance buffer data:
    │     └─ For each instance:
    │         - Look up material from material_idx
    │         - Set inst.eval_point_mask = material.eval_point_mask (read from template registration)
    │         - Push GpuInstanceData
    ├─ 5. ╔══════════════════════════════════════════════════════╗
    │     ║  NO material_class_ranges built                     ║
    │     ║  NO range iteration on CPU                          ║
    │     ║  NO per-range pipeline binding on CPU               ║
    │     ╚══════════════════════════════════════════════════════╝
    ├─ 6. Upload instance buffer to GPU
    ├─ 7. Upload dispatch table (GpuTemplateDispatch) to GPU
    │     └─ Populated from material.dispatch_mask per class
    ├─ 8. Build SINGLE multi-draw indirect command
    │     └─ instance_count = total_instances (all meshes in one draw)
    └─ 9. Build template_id_tex (if compute passes exist)
          └─ Written during gbuffer pass, not here
```

### 2.4 Direct Algorithm Comparison

| Step | v1 | v2 |
|---|---|---|
| Sort keys | `(class, graph_hash, mesh_id, material_id)` | `(graph_hash, mesh_id, material_id)` |
| Sorting complexity | O(n log n), comparison includes class | O(n log n), fewer comparison ops per swap |
| Range building | O(n) scan building ranges | **Eliminated entirely** |
| Per-class dispatch | N draws = number of ranges | **1 draw** (multi-draw indexed indirect) |
| Mask population | Not present | O(n) lookup from material record (trivial) |
| Dispatch table build | Not needed | O(m) where m = number of material classes (tiny) |
| Material class tex | Not needed | Not built here (gbuffer pass handles it) |

### 2.5 CPU Time Saved — Analysis

The primary CPU saving comes from eliminating the O(ranges × passes) iteration in every render pass:

**v1 cost per frame:**

```
Scene rebuild:    O(n log n)     — sort by (class, graph_hash, mesh_id, material_id)
Range build:      O(n)            — scan to build class ranges
Per-pass cost:    O(ranges)       — iterate ranges, filter by class, bind pipeline
Total per frame:  O(n log n + P × ranges)
```

**v2 cost per frame:**

```
Scene rebuild:    O(n log n)     — sort by (graph_hash, mesh_id, material_id) only
Mask set:         O(n)            — read mask from material, write to instance
Dispatch table:   O(m)            — populate GpuTemplateDispatch (m ≪ n)
Per-pass cost:    O(1)            — single multi_draw_indexed_indirect
Total per frame:  O(n log n + n + m + P)
```

**Eliminated work:**

| Work item | v1 | v2 | Cycles saved |
|---|---|---|---|
| Range scan (`build_material_class_ranges`) | Full scan of sorted instances | None | n × ~3 cycles per instance |
| Range iteration in GBufferPass | Iterate ranges, match classes | None (check is on GPU) | ranges × ~5 cycles per range |
| Range iteration in TransparentPass | Same | None | Same |
| Range iteration in ShadowPass | Same | None | Same |
| Range iteration in SSR (compute) | Same | None | Same |
| Pipeline switching per range | Per-range `set_pipeline` | One `set_pipeline` | ~1000s of cycles per switch |
| **Total (16 classes, 4 passes)** | ~64 range iterations + pipeline binds | **0 iterations, 0 pipeline switches** | **~64× fewer range operations** |

**For a scene with 1000 instances, 16 material classes, 4 passes:**

- v1: 1000-instance sort + ~16 range builds + 4 × 16 = 64 range iterations + up to 64 pipeline binds
- v2: 1000-instance sort + 1000 mask writes + 16 dispatch table entries + 4 draws (all O(1))

The absolute CPU time saved is modest for small scenes (~50μs) but becomes significant for scenes with many classes and passes (500+ classes across 10+ passes → milliseconds saved).

---

## 3. Material Dispatch Mask Propagation

### 3.1 Data Flow Overview

```
┌──────────────────────────────────────────────────────────────┐
│                    CPU-side (per-frame)                       │
│                                                                │
│  register_material()       rebuild_instance_buffers()          │
│  ┌──────────────┐         ┌──────────────────────┐            │
│  │ Compute mask  │         │ For each instance:    │            │
│  │ from targets  │         │   Look up material    │            │
│  │ Mask = bit N  │───────►│   Read mask from      │            │
│  │  for each     │         │   material record     │            │
│  │  eval point   │         │   Write mask to       │            │
│  │ Store mask in │         │   GpuInstanceData     │            │
│  │ material_rec  │         │   .eval_point_mask    │            │
│  └──────────────┘         └──────────┬───────────┘            │
│                                      │                        │
│                                      ▼                        │
│                            ┌──────────────────┐               │
│                            │ Upload instance   │               │
│                            │ buffer to GPU     │               │
│                            └──────────────────┘               │
├──────────────────────────────────────────────────────────────┤
│                    GPU-side (per-pixel)                       │
│                                                                │
│  ┌──────────────────┐     ┌──────────────────┐                │
│  │ Vertex shader    │     │ Fragment shader   │                │
│  │ reads inst.      │────►│ Reads mask from   │                │
│  │ .eval_point_mask │     │ VertexOutput      │                │
│  │ Passes to frag   │     │ Checks bit for    │                │
│  │ via location(4)  │     │ current eval point│                │
│  └──────────────────┘     └──────────────────┘                │
└──────────────────────────────────────────────────────────────┘
```

### 3.2 Step 1: Template Registration → Material Record

When `register_material()` is called:

```rust
pub fn register_material(
    &mut self,
    name: &str,
    wgsl_source: &str,
    targets: &[&str],           // e.g. &["gbuffer", "transparent"]
) -> MaterialClass {
    let class_id = self.next_class_id();

    // ── Build mask from target list ──────────────────────────
    let mut mask: u32 = 0;
    for eval_name in targets {
        if let Some(idx) = self.eval_registry.index(eval_name) {
            mask |= 1u32 << idx;
        }
    }

    // ── Store mask in material record ────────────────────────
    self.material_records[class_id] = MaterialRecord {
        name: name.to_string(),
        eval_point_mask: mask,          // ← stored here
        templates: HashMap::new(),
        // ... other fields ...
    };

    class_id
}
```

The `MaterialRecord` is a CPU-side struct:

```rust
#[derive(Clone)]
struct MaterialRecord {
    pub name: String,
    pub template_id: u32,           // ← persisted for rebuild_instance_buffers
    pub templates: HashMap<EvalPointId, ComposedShader>,
    pub feature_flags: u32,
}
```

### 3.3 Step 2: rebuild_instance_buffers → Instance Data

During scene rebuild, each instance references a material index. The mask is read from the material record and written into `GpuInstanceData`:

```rust
fn rebuild_instance_buffers(&mut self) {
    let scene = &self.current_scene;
    let materials = &self.material_records;

    // ── Sort instances by (graph_hash, mesh_id, material_id) ─
    let mut sorted: Vec<InstanceSortEntry> = scene.visible_instances
        .iter()
        .map(|obj| {
            let mat_idx = obj.material_class_id as usize;
            let record = &materials[mat_idx];
            InstanceSortEntry {
                graph_hash: record.graph_hash(),   // shader variant grouping
                mesh_id: obj.mesh_id,
                material_id: obj.material_id,
                instance: obj.instance_data.clone(),
                material_index: mat_idx,
            }
        })
        .collect();
    sorted.sort_unstable_by(|a, b| {
        a.graph_hash.cmp(&b.graph_hash)
            .then(a.mesh_id.cmp(&b.mesh_id))
            .then(a.material_id.cmp(&b.material_id))
    });

    // ── Build instance buffer with mask ──────────────────────
    let instance_data: Vec<GpuInstanceData> = sorted
        .iter()
        .map(|entry| {
            let mut data = entry.instance;
            let record = &materials[entry.material_index];
            data.eval_point_mask = record.eval_point_mask;  // ← set from material record
            data
        })
        .collect();

    // ── Upload ───────────────────────────────────────────────
    self.gpu_scene.instance_buffer.upload(&instance_data);
    self.gpu_scene.indirect_buffer.upload_single_draw(
        0,                          // first vertex
        0,                          // first index
        instance_data.len() as u32, // instance count
    );
}
```

### 3.4 Step 3: Upload to GPU

The instance buffer upload path is **identical** to v1 — the same buffer, the same staging upload, the same `wgpu::Queue.write_buffer()` call. The only difference is that 4 more bytes of each element are meaningful. The buffer stride on the GPU was already 160 bytes; v2 simply uses 4 of the previously unused padding bytes.

**No new buffers are created.** The existing `instance_buffer` and `instance_buffer_slot` binding in `@group(0) @binding(2)` remains unchanged.

### 3.5 Dependency Chain

```
register_material()
  └─→ material_records[class_id].eval_point_mask set
       └─→ (persistent until material is re-registered or deleted)

rebuild_instance_buffers()
  └─→ reads material_records[instance.class_id].eval_point_mask
       └─→ writes to GpuInstanceData.eval_point_mask
            └─→ uploaded to GPU instance buffer
                 └─→ vertex shader reads from vertex input
                      └─→ fragment shader reads from VertexOutput

Material re-registration:
  └─→ register_material() called again for same class_id
       └─→ eval_point_mask updated in material_records
            └─→ next rebuild_instance_buffers() picks up new mask
```

There is no stale-data problem: if a material is re-registered with a different target set, the `material_records` entry is updated immediately. The next `rebuild_instance_buffers()` call reads the fresh mask. Between re-registration and rebuild, the GPU still has the old mask from the previous upload — a window of at most 1 frame (identical to how all other instance data changes propagate).

---

## 4. Fragment Path: Vertex → Fragment

### 4.1 The Full Pipeline

For fragment-based (rasterized) passes — gbuffer, transparent, shadow, depth_prepass, velocity — the eval point mask travels through the vertex-fragment pipeline as a per-vertex attribute.

### 4.2 Vertex Shader: Reading from Instance Buffer

The instance buffer is bound as a vertex buffer with `InputStepMode::Instance`. The vertex shader declares it as a per-instance input:

```wgsl
// ── Instance buffer binding (vertex buffer slot 1) ──────────
@location(5) model_row0: vec4<f32>,
@location(6) model_row1: vec4<f32>,
@location(7) model_row2: vec4<f32>,
@location(8) model_row3: vec4<f32>,
@location(9) normal_row0: vec4<f32>,
@location(10) normal_row1: vec4<f32>,
@location(11) normal_row2: vec4<f32>,
@location(12) bounds: vec4<f32>,
@location(13) mesh_id: u32,
@location(14) material_id: u32,
@location(15) flags: u32,
@location(16) lightmap_index: u32,
@location(17) template_id:     u32,         // ← NEW: 4 bytes from instance buffer
```

The vertex shader reads it from the instance buffer and copies it into the `VertexOutput` struct:

```wgsl
struct VertexInput {
    // ── Per-vertex attributes (slot 0) ──────────────────────
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) tangent: vec4<f32>,

    // ── Per-instance attributes (slot 1) ────────────────────
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(9) normal_row0: vec4<f32>,
    @location(10) normal_row1: vec4<f32>,
    @location(11) normal_row2: vec4<f32>,
    @location(12) bounds: vec4<f32>,
    @location(13) mesh_id: u32,
    @location(14) material_id: u32,
    @location(15) flags: u32,
    @location(16) lightmap_index: u32,
    @location(17) template_id:     u32,     // ← NEW
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position:  vec3<f32>,
    @location(1) world_normal:    vec3<f32>,
    @location(2) tex_coords:      vec2<f32>,
    @location(3) @interpolate(flat) material_id: u32,
    @location(4) @interpolate(flat) template_id:     u32,  // ← NEW
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    // ── Build model matrix from rows ─────────────────────
    let model = mat4x4<f32>(
        input.model_row0,
        input.model_row1,
        input.model_row2,
        input.model_row3,
    );
    let normal_mat = mat3x4<f32>(
        input.normal_row0,
        input.normal_row1,
        input.normal_row2,
    );

    // ── Standard vertex transform ────────────────────────
    let world_pos = model * vec4<f32>(input.position, 1.0);
    output.clip_position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.world_normal = normalize(normal_mat * vec4<f32>(input.normal, 0.0));
    output.tex_coords = input.tex_coord;

    // ── Pass-through the flat-interpolated values ────────
    output.material_id = input.material_id;
    output.eval_point_mask = input.eval_point_mask;  // ← passthrough

    return output;
}
```

### 4.3 Flat Interpolation

The `@interpolate(flat)` decoration is critical. It tells the GPU:

- **No perspective-correct interpolation** across the triangle
- Every fragment in the triangle receives the value from the **provoking vertex** (the leading vertex of the triangle)
- This means the fragment shader sees a **constant** `eval_point_mask` across all fragments of the same triangle
- Cost: exactly 1 register move (no interpolation hardware consumed)

Without `flat`, the GPU would interpolate the `u32` value across the triangle using perspective correction — wasting ALU cycles and producing garbage (you don't want interpolated bits).

### 4.4 Fragment Shader: Receiving the Mask

```wgsl
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // ── Single predicated instruction ────────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << GBUFFER_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation (only reached if mask matches) ───
    let surface = eval_gbuffer(input.material_id, input.world_position,
                               input.world_normal, input.tex_coords);
    // ... encode to gbuffer targets ...
}
```

### 4.5 Compile-Time Constants

The `GBUFFER_EVAL_POINT`, `TRANSPARENT_EVAL_POINT`, etc. are compile-time constants injected into the composed WGSL via string substitution.

**Injection mechanism:**

```rust
/// During base shader composition, inject the eval point index as a const.
fn inject_eval_point_index(
    base_shader: &str,
    eval_point_index: u32,
    eval_point_name: &str,
) -> String {
    let const_name = format!("{}_EVAL_POINT", eval_point_name.to_uppercase());
    let const_decl = format!("const {} = {}u;", const_name, eval_point_index);

    // ── Insert after the last existing const declaration ─────
    // (or at the top of the file if no consts exist)
    // ...
}
```

**Resulting WGSL for the gbuffer pass (eval point index = 0):**

```wgsl
const GBUFFER_EVAL_POINT = 0u;

// ...

@fragment
fn fs_main(input: VertexOutput) -> ... {
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << GBUFFER_EVAL_POINT)) == 0u {
        discard;
    }
    // ...
}
```

**For the transparent pass (eval point index = 1):**

```wgsl
const TRANSPARENT_EVAL_POINT = 1u;

// ...

@fragment
fn fs_main(input: VertexOutput) -> ... {
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << TRANSPARENT_EVAL_POINT)) == 0u {
        discard;
    }
    // ...
}
```

The `1u << N` expression is a **compile-time constant** in WGSL. The shader compiler evaluates `1u << TRANSPARENT_EVAL_POINT` to a literal integer at compile time. The resulting GPU instruction is a single `and` + `cmov` (or equivalent predicated instruction), NOT a variable shift.

### 4.6 How the Constants Are Defined

The `EvalPointRegistry` assigns sequential indices starting from 0 at registration time:

```rust
impl EvalPointRegistry {
    pub fn register(&mut self, point: EvalPoint) -> EvalPointId {
        let id = self.points.len() as u32;
        self.points.push(point);
        self.name_map.insert(point.name, id);
        id
    }
}
```

| Eval Point | Name (lowercase) | Name (uppercase const) | Assigned Index | Bit Value |
|---|---|---|---|---|
| `"gbuffer"` | `gbuffer` | `GBUFFER_EVAL_POINT` | 0 | `1u << 0` = 1 |
| `"transparent"` | `transparent` | `TRANSPARENT_EVAL_POINT` | 1 | `1u << 1` = 2 |
| `"shadow"` | `shadow` | `SHADOW_EVAL_POINT` | 2 | `1u << 1` = 4 |
| `"depth_prepass"` | `depth_prepass` | `DEPTH_PREPASS_EVAL_POINT` | 3 | `1u << 3` = 8 |
| `"ssr"` | `ssr` | `SSR_EVAL_POINT` | 4 | `1u << 4` = 16 |
| `"sss"` | `sss` | `SSS_EVAL_POINT` | 5 | `1u << 5` = 32 |
| `"decal"` | `decal` | `DECAL_EVAL_POINT` | 6 | `1u << 6` = 64 |
| `"post_process"` | `post_process` | `POST_PROCESS_EVAL_POINT` | 7 | `1u << 7` = 128 |
| `"velocity"` | `velocity` | `VELOCITY_EVAL_POINT` | 8 | `1u << 8` = 256 |

The engine guarantees ≤ 32 evaluation points (fits in `u32`). If more than 32 points are needed in the future, the mask could be promoted to `u64` at the cost of 4 more bytes of instance data.

---

## 5. Fragment Path: The Discard Pattern

### 5.1 The Fundamental Pattern

Every fragment shader that participates in the eval point system contains this exact pattern as the first non-declaration statement:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u { discard; }
```

### 5.2 Why This Is a Single Predicated Instruction

On modern GPU architectures (NVIDIA Maxwell+, AMD GCN+, Apple Metal, Intel Gen9+, Qualcomm Adreno 6+), the pattern compiles to:

**NVIDIA (PTX/SASS equivalent):**

```
// Assume:
//   R0 = input.eval_point_mask
//   PS = predicate to test: (R0 & CONST) == 0
//   CONST = pre-computed shift of THIS_EVAL_POINT

LOP.AND R1, R0, CONST            // R1 = mask & (1u << THIS_EVAL_POINT)
ISETP.EQ P0, R1, 0, PT           // predicate P0 = (R1 == 0)
@P0 EXIT                          // if P0 true, exit fragment shader (discard)
```

This is **3 instructions** in the worst case, but on architectures with predicated execution, the `and` and `setp` can fuse into a single `LOP.AND` with implicit zero test. The `EXIT` is predicated — no branch misprediction penalty.

**AMD (GCN ISA equivalent):**

```
s_and_b32 s0, s0, const          // scalar AND (mask is uniform across wave)
s_cmp_eq_i32 s0, 0               // compare to zero
s_cbranch_execz label             // if all lanes failed, skip
```

Because the mask is flat-interpolated, every lane in the wave has the same value. AMD GPUs can use scalar instructions (`s_and_b32`) instead of vector instructions, making the check essentially free — it executes once per wave, not once per lane.

### 5.3 The Never-Taken Branch (Basic PBR in Gbuffer Pass)

For basic PBR: `eval_point_mask = 1` (only bit 0 = `GBUFFER_EVAL_POINT` set).

Check: `(1 & (1u << 0)) == 0u` → `(1 & 1) == 0` → `false` → **never taken**.

- The GPU predicated-unit sees the condition is false
- The predicated `discard` instruction is issued but **masked out** by the condition
- Zero ALU cycles consumed for the masked-out path
- Zero divergence: every lane in the wave takes the same path

**Measurable cost: 0 cycles** on NVIDIA Volta/Turing/Ampere, AMD RDNA, Apple M1/M2.

### 5.4 The Always-Taken Branch (Basic PBR in Transparent Pass)

For basic PBR in the transparent pass: `eval_point_mask = 1` (only bit 0 set).

Check: `(1 & (1u << 1)) == 0u` → `(1 & 2) == 0` → `true` → **always taken**.

- Every fragment from this draw is immediately discarded
- The `discard` instruction prevents further fragment execution — no texture samples, no ALU, no ROP writes
- However, the geometry was still rasterized — the triangle setup and vertex processing already happened
- Depth testing: the `discard` instruction means the depth buffer is **not updated** for rejected fragments. This is important: it means transparent-pass writes to depth are prevented for opaque geometry, which is correct behavior (opaque geometry writes depth during the gbuffer/depth prepass, not during transparent)
- Cost: the rasterizer still generates fragments, but the fragment shader is essentially a NOP after the early discard

**Why we still draw:** Even though the geometry is immediately discarded, we cannot skip the draw entirely because:
1. The draw is a single multi-draw for ALL instances — we don't know which instances target which passes at draw-issue time
2. We could, in theory, split draws by mask, but the per-pixel check is cheaper than per-range pipeline switching (see Section 8)

### 5.5 Wavefront Divergence

Because `eval_point_mask` is flat-interpolated, ALL fragments in the same triangle have the same mask value. Within a GPU wave/warp/wavefront (which processes a group of adjacent pixels):

- **Best case**: all pixels in the wave belong to the same triangle → uniform mask → zero divergence
- **Worst case**: pixels at a triangle edge belong to different triangles → but neighboring triangles from the same instance batch have the same mask (same material) → still zero divergence
- **Edge case**: geometry with very small triangles from different materials in the same wave → possible divergence, but the predicated instruction handles this at no cost beyond the masked lanes

**The flat-interpolated attribute guarantee makes divergence essentially impossible for this check.** This is a deliberate design choice: if we used perspective-correct interpolation (non-flat), the mask value would be interpolated per-pixel and could diverge within a triangle, causing wavefront divergence. By using `flat`, we eliminate that possibility entirely.

### 5.6 Early-Z Interaction

The `discard` instruction has implications for early-Z testing:

- **Early-Z with discard**: Most GPUs can still perform early-Z before the fragment shader, but `discard` forces late-Z for fragments that pass the early-Z test. The `discard` itself occurs after the fragment shader starts but before it writes outputs.
- **Impact**: For the always-taken case (PBR in transparent pass), early-Z still culls fragments occluded by depth. Only fragments that pass early-Z reach the discard — and they are discarded immediately with no texture sampling.
- **Net effect**: The transparent pass for opaque geometry has the same cost as a depth-only test plus one predicated instruction. No texture bandwidth, no ALU, no ROP.

### 5.7 Correctness Considerations

The `discard` instruction:
1. Prevents any color output to render targets
2. Prevents depth buffer writes (for `depth_write: true` passes)
3. Does NOT prevent the stencil buffer from being updated (if stencil is enabled — but our passes typically don't use stencil for eval)
4. Does NOT prevent alpha-to-coverage from consuming the fragment (but we don't use alpha-to-coverage)

This is exactly the correct behavior: materials that don't target a pass should be invisible to that pass's output.

---

## 6. Compute Path: Template ID texture

### 6.1 Why Compute Needs a Different Approach

Compute passes (SSR, SSS, decal, post-process) do not have per-pixel instance data. A compute shader dispatches a full-screen grid of workgroups, and each thread processes one pixel (or a tile of pixels). There is no vertex shader to pass instance data through, no rasterizer to interpolate attributes.

For a compute thread to know which material is at its pixel position, it needs an explicit per-pixel material ID. This is the **Template ID texture**.

### 6.2 Texture Specification

| Property | Value |
|---|---|
| Format | `R32Uint` (32-bit unsigned integer per texel) |
| Dimensions | Full render resolution (e.g., 1920×1080) |
| Mip levels | 1 (no mips) |
| Usage | `RENDER_ATTACHMENT` + `TEXTURE_BINDING` |
| Sample count | 1 |
| Memory | 4 bytes × W × H = ~8MB at 1080p, ~33MB at 4K |

### 6.3 Writing the Texture: Gbuffer Pass Integration

The Template ID texture is written as an **additional render target** during the gbuffer pass. The gbuffer fragment shader already runs for every opaque pixel that passes the eval point check; it simply writes the template ID to the extra target alongside the existing albedo/normal/orm/emissive targets.

**Gbuffer render target layout (v2):**

```rust
let gbuffer_color_attachments = &[
    Some(attachment(&self.albedo_tex, ...)),       // RGBA8Unorm     — target 0
    Some(attachment(&self.normal_tex, ...)),        // RGBA16Float    — target 1
    Some(attachment(&self.orm_tex, ...)),           // RGBA8Unorm     — target 2
    Some(attachment(&self.emissive_tex, ...)),      // RGBA16Float    — target 3
    Some(attachment(&self.template_id_tex, ...)),// R32Uint        — target 4  ← NEW
    // (depth_stencil_attachment is separate)
];
```

**Fragment shader output struct:**

```wgsl
struct GBufferOutput {
    @location(0) albedo:    vec4<f32>,
    @location(1) normal:    vec4<f32>,
    @location(2) orm:       vec4<f32>,
    @location(3) emissive:  vec4<f32>,
    @location(4) template_id: u32,       // ← NEW: write material_class ID here
}

@fragment
fn fs_main(input: VertexOutput) -> GBufferOutput {
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << GBUFFER_EVAL_POINT)) == 0u {
        discard;
    }

    let surface = eval_gbuffer(input.material_id, input.world_position,
                               input.world_normal, input.tex_coords);

    var output: GBufferOutput;
    output.albedo = surface.albedo;
    output.normal = vec4<f32>(surface.normal * 0.5 + 0.5, surface.roughness);
    output.orm = vec4<f32>(surface.occlusion, surface.roughness, surface.metallic, 0.0);
    output.emissive = vec4<f32>(surface.emissive, surface.emissive_intensity);
    output.template_id = input.material_id;  // ← write class ID for compute passes

    return output;
}
```

### 6.4 Which Materials Get Written

Only fragments that pass the gbuffer eval point check (`mask & GBUFFER_BIT != 0`) write to the Template ID texture. Materials that don't target `"gbuffer"` (e.g., a water material targeting only `"transparent"`) do NOT write to the Template ID texture during the gbuffer pass.

**Consequence:** For compute passes that need material class data for non-gbuffer pixels (e.g., SSR evaluating water pixels that only target `"transparent"`), the Template ID texture will contain a stale value from the last frame. This is acceptable for most post-processing effects:

- SSR: glass/water surfaces that don't write gbuffer normals will show reflected artifacts, but the material itself will evaluate its SSR shader on top
- If a compute pass needs accurate material class for ALL pixels, the pass itself can write the class texture (or the material can target both gbuffer and the compute pass)

In practice, the `"gbuffer"` evaluation point is the standard entry point for surface data. Most materials that participate in any compute effect also participate in gbuffer (to write normals, roughness, etc. for those effects). Water targeting only `"transparent"` is an edge case — it wouldn't need SSR evaluation because its surface is handled by the transparent pass.

### 6.5 Reading the Texture: Compute Shader

```wgsl
@group(1) @binding(5) var template_id_tex: texture_2d<u32>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    // ── Read material class from gbuffer output ──────────────
    let template_id = textureLoad(template_id_tex, id.xy, 0).r;

    // ── Check if this material targets SSR eval point ────────
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << SSR_EVAL_POINT)) == 0u {
        return;     // early exit for non-targeting materials
    }

    // ── SSR evaluation for this material ─────────────────────
    // ... read gbuffer textures, compute ray march, etc. ...
}
```

### 6.6 Memory Bandwidth Analysis

**Writing (gbuffer pass):**

| Target | Format | Bytes/pixel | 1080p bandwidth (per frame) |
|---|---|---|---|
| Albedo | RGBA8Unorm | 4 | ~8MB |
| Normal | RGBA16Float | 8 | ~16MB |
| ORM | RGBA8Unorm | 4 | ~8MB |
| Emissive | RGBA16Float | 8 | ~16MB |
| Material class | R32Uint | 4 | **~8MB ← NEW** |
| **Total** | | **28** | **~56MB** |

The Template ID texture adds **4 bytes per pixel** of write bandwidth during the gbuffer pass. This is a 16% increase in gbuffer bandwidth (from 24 to 28 bytes/pixel). For a 1080p render at 60fps with ~50% overshading (average 1.5× pixel writes due to overdraw), this is ~84 MB/s of additional bandwidth.

**Reading (each compute pass):**

| Operation | Bytes/pixel | Notes |
|---|---|---|
| TextureLoad (1 × R32Uint) | 4 | One texel read per compute thread |
| Dispatch table read | negligible | 16 bytes per material class, cached |

Each compute pass that reads the Template ID texture adds **4 bytes per pixel** of read bandwidth. With 3 compute passes (SSR, SSS, decal) this is 12 bytes/pixel of read bandwidth — or ~24 MB/s at 1080p 60fps.

**Total new bandwidth cost: ~108 MB/s at 1080p 60fps**, which is negligible compared to the total texture bandwidth of a modern renderer (~5-20 GB/s for gbuffer textures alone).

### 6.7 Alternative: Compute Shader Material ID Buffer

An alternative design would be to have a dedicated `storage` buffer of material IDs for the gbuffer, indexed by pixel address. The render target approach is preferred because:

1. **Native hardware blending**: The R32Uint render target uses the GPU's ROP (render output unit) for writes — no explicit store in the shader
2. **Async compute**: Render target writes are queued through the graphics pipeline, which is the natural producer of the data
3. **No separate allocation**: The texture is part of the gbuffer framebuffer, sharing the same lifetime
4. **Simple read path**: `textureLoad` is a single intrinsic, no address calculation needed

---

## 7. Compute Path: Dispatch Table

### 7.1 The GpuTemplateDispatch Buffer

The dispatch table is a small GPU-visible storage buffer containing one entry per material class. It provides the eval point mask to compute shaders, indexed by template ID.

### 7.2 CPU-Side Structure

```rust
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuTemplateDispatch {
    /// Bit N = 1 ⇒ this material class targets evaluation point N.
    pub template_id: u32,
    /// Padding to align to vec4 boundary (WGSL struct alignment rules).
    pub _pad: [u32; 3],
}

const MAX_MATERIAL_CLASSES: usize = 1024;  // upper bound, allocated once

// ── GPU buffer ────────────────────────────────────────────
self.dispatch_table_buffer = device.create_buffer(&wgpu::BufferDescriptor {
    label: Some("material_dispatch_table"),
    size: (MAX_MATERIAL_CLASSES * size_of::<GpuTemplateDispatch>()) as u64,
    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    mapped_at_creation: false,
});
```

### 7.3 Buffer Specification

| Property | Value |
|---|---|
| Type | `wgpu::Buffer` with `STORAGE | COPY_DST` usage |
| Element stride | 16 bytes (align 16) |
| Element count | MAX_MATERIAL_CLASSES (1024) |
| Total size | 16 KB (trivial) |
| Upload frequency | Once per scene change (or material re-registration) |
| Bind group group | `@group(1) @binding(4)` |

### 7.4 Populating the Dispatch Table

During `rebuild_instance_buffers()` (or immediately after material re-registration), the dispatch table is populated from `material_records`:

```rust
fn update_dispatch_table(&mut self) {
    let mut table_data = vec![
        GpuTemplateDispatch {
            eval_point_mask: 0,
            _pad: [0u32; 3],
        };
        MAX_MATERIAL_CLASSES
    ];

    for (class_id, record) in self.material_records.iter().enumerate() {
        table_data[class_id].eval_point_mask = record.eval_point_mask;
    }

    // ── Upload to GPU ────────────────────────────────────
    self.device_queue.write_buffer(
        &self.dispatch_table_buffer,
        0,
        bytemuck::cast_slice(&table_data),
    );
}
```

### 7.5 GPU-Side WGSL

```wgsl
struct GpuTemplateDispatch {
    template_id:     u32,
    _pad: vec3<u32>,
}

@group(1) @binding(4) var<storage, read> template_dispatches: array<GpuTemplateDispatch>;

// ── Usage in compute shader ──────────────────────────────
fn check_eval_point(class_id: u32, eval_point_bit: u32) -> bool {
    return (template_dispatches[class_id].eval_point_mask & eval_point_bit) != 0u;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    if (!check_eval_point(template_id, 1u << SSR_EVAL_POINT)) {
        return;
    }
    // ... evaluate ...
}
```

### 7.6 Why 16 Bytes Per Element (Not 4)

WGSL storage buffer elements must be at least 16 bytes if they contain a single scalar, because `array<u32>` is the only valid single-scalar array type. A struct with a single `u32` member gets padded to align with the struct's default alignment of 16 (the WGSL default for struct members without explicit `@align`).

If memory were a concern, we could use `array<u32>` directly (which is 4 bytes per element). The 3× overhead (16 bytes vs 4 bytes) is acceptable because:

- 1024 material classes × 16 bytes = 16 KB (utterly trivial)
- The padding may be used for future metadata (e.g., feature flags, template ID)
- A separate `array<u32>` buffer would require a second binding slot

### 7.7 Upload Timing

The dispatch table is uploaded:

1. **Full upload**: During `rebuild_instance_buffers()` whenever material registrations change
2. **Partial update**: Individual entries can be updated via `write_buffer` with an offset when a single material is re-registered (optimization, not required)

---

## 8. Comparison: Fragment vs Compute Path

### 8.1 High-Level Comparison

| Aspect | Fragment Path | Compute Path |
|---|---|---|
| **Rendering model** | Rasterized (vertex + fragment shader) | Full-screen compute dispatch |
| **Per-pixel data source** | Instance buffer → vertex shader → fragment (flat-interpolated) | Template ID texture (R32Uint) written by gbuffer pass |
| **Mask storage** | `GpuInstanceData.eval_point_mask` (4 bytes per instance, embedded in existing buffer) | `GpuTemplateDispatch[]` (16 bytes per material class, separate storage buffer) |
| **Mask read cost** | **Zero extra reads** — mask is part of the instance buffer that is ALREADY fetched for vertex processing | **4 bytes/pixel** — textureLoad of Template ID texture + 4 bytes of dispatch table (from tiny, cached buffer) |
| **Mask check cost** | Single predicated `if` in fragment shader | Single predicated `if` in compute shader |
| **Setup cost (CPU)** | O(n) to write mask into instance data during rebuild | O(1) to upload dispatch table + O(1) to bind texture |
| **Setup cost (GPU)** | Single extra vertex attribute slot (location 17) | One additional render target during gbuffer pass |
| **Memory cost (GPU)** | **0 net increase** (reuses padding in instance buffer) | ~8 MB at 1080p (Template ID texture) + 16 KB (dispatch table) |
| **Divergence risk** | None — mask is flat-interpolated, uniform per triangle | Low — Template ID texture is coherent in practice (adjacent pixels tend to share materials) |
| **Applicable passes** | GBuffer, Transparent, Shadow, DepthPrepass, Velocity | SSR, SSS, Decal, PostProcess, MotionBlur |
| **Per-pixel custom materials** | Native — each instance carries its mask | Requires texture indirection |

### 8.2 Detailed Cost Breakdown

#### Fragment Path Costs

| Component | Cost | Explanation |
|---|---|---|
| Instance buffer upload | +0 bytes | Mask fits in existing stride |
| Vertex shader input | +4 bytes | One extra `@location(17)` attribute |
| Vertex shader pass-through | 1 move instruction | `output.eval_point_mask = input.eval_point_mask` |
| Fragment shader attribute | 0 extra bytes beyond `VertexOutput` | Already added to struct; flat-interpolated (no per-pixel ALU) |
| Fragment shader check | 1 predicated `and + test` | No actual branch; predicated `discard` |
| **Total per-vertex overhead** | **4 bytes of bandwidth + 1 move instruction** | ~0.01% of vertex shader cost |
| **Total per-fragment overhead** | **1 predicated instruction** | ~0.001% of fragment shader cost for basic PBR |

#### Compute Path Costs

| Component | Cost | Explanation |
|---|---|---|
| Template ID texture memory | 4 bytes × pixels | ~8MB at 1080p (shared by all compute passes) |
| Gbuffer write bandwidth | +4 bytes/pixel | Written during gbuffer pass as extra render target |
| Compute read bandwidth | +4 bytes/pixel × (# of compute passes) | Each compute pass reads the Template ID texture |
| Dispatch table memory | 16 KB | 1024 classes × 16 bytes (trivial) |
| Dispatch table upload | 16 KB per scene change | Negligible |
| Compute textureLoad | 1 texture sample | Usually cached (adjacent pixels read coherent regions) |
| Compute dispatch table read | 1 scalar load | From tiny buffer, likely in L1 or constant cache |
| Compute mask check | 1 predicated `and + test` | Same as fragment path |
| **Total per-pixel memory** | **~4-12 bytes** (depending on # of compute passes) | Larger than fragment path |

### 8.3 Why Compute Cannot Use the Instance Buffer

The instance buffer is bound as a **vertex buffer** in the graphics pipeline. Compute shaders cannot access vertex buffers directly:

1. There is no vertex processing stage in compute — no `@location` attributes
2. The rasterizer is not active in compute — no barycentric interpolation of attributes
3. Even if we mapped the instance buffer as a storage buffer, we would need to compute which instance covers each pixel, which requires a full screen-space rasterization pass (effectively reimplementing the rasterizer in software — too expensive)

The Template ID texture is the correct solution: it's a simple indirection that converts a 2D pixel coordinate into a template ID with a single `textureLoad`.

### 8.4 When Fragment Path Is Better

- Passes that already rasterize geometry (gbuffer, transparent, shadow)
- Passes that need per-instance data (world position, normal, UV — already interpolated from vertex data)
- Passes where geometry culling (frustum, occlusion) reduces pixel count
- Passes that benefit from early-Z rejection

### 8.5 When Compute Path Is Better

- Passes that process every pixel equally (SSR, post-process, bloom)
- Passes that don't need geometry processing (full-screen effects)
- Passes that need random access to pixel data (not limited to the visible surface)
- Passes that are bandwidth-bound and would be bottlenecked by geometry throughput

### 8.6 Hybrid Path (Notionally Possible)

A future optimization could combine both paths: use the fragment path to write material data per-pixel when rasterizing, then use a compute pass to read it. This is essentially what happens with the Template ID texture — the gbuffer fragment shader writes it, compute passes read it. The eval point mask transit is split accordingly:

```
Fragment gbuffer writes:
    template_id_tex[pixel] = material_class_id

Compute SSR reads:
    class = template_id_tex[pixel]
    mask = template_dispatches[class]
    if (mask & SSR_BIT) { evaluate_ssr(); }
```

---

## 9. Indirect Draw Batching

### 9.1 Single Multi-Draw for All Instances

In v2, every fragment pass issues a **single** `multi_draw_indexed_indirect` command that draws ALL instances, regardless of material class. Compare to v1, where each range of `(class, graph_hash)` required a separate draw call with a different pipeline.

### 9.2 Indirect Buffer Layout (Unchanged from v1)

```rust
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawIndexedIndirect {
    pub vertex_count: u32,       // index count per mesh
    pub instance_count: u32,     // total instances in batch
    pub first_index: u32,        // base index into index buffer
    pub vertex_offset: i32,      // base vertex offset
    pub first_instance: u32,     // base instance (always 0 for multi-draw)
}
```

**v2 single draw command:**

```rust
DrawIndexedIndirect {
    vertex_count: mesh_index_count,  // from the single combined mesh or per-mesh
    instance_count: total_visible_instances,
    first_index: 0,
    vertex_offset: 0,
    first_instance: 0,
}
```

**v1 per-range draw commands (N of these):**

```rust
DrawIndexedIndirect {
    vertex_count: per_mesh_count,
    instance_count: range_instance_count,
    first_index: mesh_first_index,
    vertex_offset: mesh_vertex_offset,
    first_instance: range_first_instance,
}
```

### 9.3 Why a Single Draw Works

The v2 approach relies on:

1. **Same vertex/index buffers**: All instances use the same vertex layout (positions, normals, UVs, tangents). Different meshes are batched into a single vertex/index buffer during scene setup (same as v1).
2. **No pipeline switching per class**: The shader variant is compiled for ALL materials because the material's eval function is parameterized by `material_id` (which is read from the instance data as `@location(14)`). The fragment shader dispatches to the correct material evaluation based on `input.material_id`, not based on which draw call it came from.
3. **Per-pixel discard handles masking**: Instances that don't target the current eval point are discarded at the pixel level. The GPU spends rasterization + discard cost instead of CPU pipeline-switch cost.

### 9.4 Performance: Per-Pixel Discard vs Per-Range Pipeline Switching

| Aspect | v1 (per-range pipeline switch) | v2 (single draw + discard) |
|---|---|---|
| **Draw calls issued** | N (one per range) | 1 |
| **Pipeline binds (CPU)** | N × (set_pipeline + bind_groups) | 1 × (set_pipeline + bind_groups) |
| **Pipeline binds (GPU)** | N barrier checks | 1 barrier check |
| **Rasterization** | Same — all fragments rasterized | Same — all fragments rasterized |
| **Fragment processing** | All fragments run shader | Some fragments run shader, some discard (predicated) |
| **Overdraw** | Same | Same |
| **Depth testing** | Same | Same |

**The tradeoff:**

- **v1 wins** when: the scene has many instances that DON'T target the pass, so the CPU can skip entire ranges → but this requires the CPU to KNOW which classes target which passes, which means iterating ranges and checking per-class → O(ranges) CPU work
- **v2 wins** when: the CPU cost of range iteration exceeds the GPU cost of rasterizing + discarding non-targeting fragments → which is most of the time, because discarding is cheap (predicated) and range iteration is expensive (cache misses, branch mispredictions, pipeline barriers)

**Quantitative comparison for a typical scene (1000 instances, 16 material classes, 4 passes):**

```
v1 per-pass CPU cost:
  iterate 16 ranges                         ~200 cycles
  filter classes for THIS pass              ~100 cycles
  bind pipeline + vertex buffers per match  ~2000 cycles (8 pipeline binds × ~250 cycles)
  TOTAL per pass:                           ~2300 cycles
  TOTAL × 4 passes:                         ~9200 cycles

v2 per-pass CPU cost:
  bind pipeline once                        ~250 cycles
  issue single multi_draw                   ~50 cycles
  TOTAL per pass:                           ~300 cycles
  TOTAL × 4 passes:                         ~1200 cycles

CPU time saved: ~8000 cycles per frame = ~2.7μs at 3GHz
```

### 9.5 GPU Cost of Discard in the Single Draw

For the case where most instances DON'T target a pass (e.g., opaque geometry in the transparent pass):

- **v1**: The CPU iterates ranges, determines that only 1 of 16 classes targets transparent, binds the pipeline once, and issues 1 draw for glass instances only. Opaque instances are never drawn. **GPU cost**: only glass fragments processed.
- **v2**: The CPU issues 1 draw for ALL 1000 instances. The GPU rasterizes all 1000 instances, then discards 99% of fragments after the predicated check. **GPU cost**: all fragments rasterized, all vertex shaders run, all triangle setup done.

**This is the worst case for v2.** However:

- The vertex shader runs for every vertex regardless — same in both v1 and v2 (the vertex shader is not the bottleneck; it's ALU-bound and the additional draw counts don't change total vertices processed)
- Triangle setup is cheap on modern GPUs (fixed-function, processes triangles at hundreds of millions/sec)
- The rasterizer generates fragments, but early-Z quickly rejects fragments occluded by opaque geometry
- The remaining fragments execute the predicated check → discard
- **Net cost**: ~10-20% more GPU time for the transparent pass in the worst case (all opaque, no transparent materials), but this is offset by the CPU time saved and the simplicity of the code path

### 9.6 Future Optimization: Multi-Draw Splitting by Mask

If profiling shows that per-pixel discard is too expensive for a specific pass (e.g., scenes with hundreds of thousands of instances where 99% are opaque and 1% is transparent), an optimization pass could split the multi-draw into sub-ranges by mask:

```rust
// ── Optional optimization: split draws by mask ────────────
fn build_multi_draw_indirect_optimized(&self, instances: &[GpuInstanceData]) -> Vec<DrawIndirectCommand> {
    // Group consecutive instances by eval_point_mask
    // Issue one draw per unique mask value
    // Fragments within each draw always pass (or always fail) the mask check
    // → zero wasted fragment processing
}
```

This optimization is **not part of v2** — it's a future enhancement if needed. The simple single-draw approach is correct, maintainable, and fast enough for the vast majority of scenes.

### 9.7 Indirect Command Buffer Layout (WGSL)

```wgsl
// ── Indirect buffer used by multi_draw_indexed_indirect ─────
// This is the same struct as v1, unchanged.

struct DrawIndexedIndirect {
    vertex_count:    u32,
    instance_count:  u32,
    first_index:     u32,
    vertex_offset:   i32,
    first_instance:  u32,
}
```

No changes needed to the indirect buffer layout. The only difference is that v2 typically issues 1 draw command instead of N.

---

## 10. Vertex Processing Cost

### 10.1 What Changed in the Vertex Shader

**v1 vertex output:**

```wgsl
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,    // 16 bytes
    @location(0) world_position:  vec3<f32>,          // 12 bytes
    @location(1) world_normal:    vec3<f32>,          // 12 bytes
    @location(2) tex_coords:      vec2<f32>,          // 8 bytes
    @location(3) @interpolate(flat) material_id: u32, // 4 bytes
    // Total: 52 bytes (52 + 12 padding → 64 bytes aligned)
}
```

**v2 vertex output:**

```wgsl
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,    // 16 bytes
    @location(0) world_position:  vec3<f32>,          // 12 bytes
    @location(1) world_normal:    vec3<f32>,          // 12 bytes
    @location(2) tex_coords:      vec2<f32>,          // 8 bytes
    @location(3) @interpolate(flat) material_id: u32, // 4 bytes
    @location(4) @interpolate(flat) template_id:     u32, // 4 bytes ← NEW
    // Total: 56 bytes (56 + 8 padding → 64 bytes aligned)
}
```

**Net change: +4 bytes** in the vertex output struct (from 52 to 56 bytes of actual data; 64 bytes aligned in both cases).

### 10.2 Bandwidth Impact

The vertex shader output bandwidth is determined by the number of vertices emitted and the size of the `VertexOutput` struct written to the VGPR (vector general-purpose register) file and the parameter cache (the crossbar that passes data from vertex to fragment shader).

| Resolution | Visible triangles | Vertices per frame | v1 bandwidth (52B) | v2 bandwidth (56B) | Δ |
|---|---|---|---|---|---|
| 1080p | ~500K | ~1.5M | ~78 MB | ~84 MB | +6 MB |
| 1440p | ~800K | ~2.4M | ~125 MB | ~134 MB | +9 MB |
| 4K | ~2M | ~6M | ~312 MB | ~336 MB | +24 MB |

**The increase is ~7.7% of vertex output bandwidth**, which itself is typically <5% of the total GPU bandwidth budget (texture sampling dominates at 10-100 GB/s).

**For a modern GPU with 200+ GB/s memory bandwidth**, the 6-24 MB/s increase is **below 0.01% of total bandwidth** — well below measurement noise.

### 10.3 Why Flat-Interpolated Attributes Are Cheaper

The `@interpolate(flat)` decoration tells the GPU:

1. No perspective-correct interpolation is needed
2. The value is taken directly from the provoking vertex — no barycentric calculation
3. The attribute bypasses the interpolation hardware entirely

**Cost of different interpolation modes:**

| Mode | ALU cost per attribute | Register pressure | Bandwidth |
|---|---|---|---|
| Perspective (default) | 1 MAD + perspective divide | 1 VGPR | 16 bytes |
| Linear (`@interpolate(perspective=false)`) | 1 MAD | 1 VGPR | 16 bytes |
| **Flat** (`@interpolate(flat)`) | **0** | 1 scalar register | **4 bytes (u32)** |

Flat-interpolated attributes are the cheapest possible vertex output — they use scalar registers (not vector), require no interpolation hardware, and consume minimal bandwidth.

### 10.4 Vertex Shader Instruction Count Impact

The only new instruction in the vertex shader:

```wgsl
output.eval_point_mask = input.eval_point_mask;  // 1 move instruction
```

This is a single scalar move — no ALU, no memory access, no texture sample. On most GPU ISAs, this is either:
- A no-op (the register assignment is optimized out by the compiler — the input attribute is mapped directly to the output attribute)
- A single `mov` instruction (1 cycle, no pipeline stall)

**Adding the mask to the vertex shader costs 0 or 1 instruction.** For a vertex shader that typically has 100-500 instructions (matrix multiply, normal transform, UV transform, tangent frame, displacement mapping, etc.), this is **≤1% instruction count increase**.

### 10.5 Vertex Input Slot Impact

The instance buffer gains one vertex attribute slot (`@location(17)`). This has no real cost because:

- Vertex attribute slots are free on modern hardware (up to 32 available)
- The vertex buffer stride doesn't change (160 bytes in both v1 and v2)
- No additional vertex buffer binding needed — the same instance buffer is used
- No additional fetch bandwidth — the instance buffer is already fetched as part of normal instance processing

### 10.6 Conclusion: The Mask Is Free

| Resource | Impact | Magnitude |
|---|---|---|
| Vertex shader instructions | 0-1 scalar move | ≤1% of vertex shader |
| Vertex output bandwidth | +4 bytes per vertex | ~7.7% of vertex output, ≤0.01% of total bandwidth |
| Vertex attribute slots | +1 (location 17) | Free (32 available) |
| Instance buffer stride | Unchanged (160 bytes) | 0% |
| Fragment shader input | +4 bytes in VertexOutput | Flat-interpolated, 0 ALU for interpolation |
| Fragment shader check | 1 predicated instruction | ~0.001% of fragment shader for basic PBR |
| Memory (GPU) | 0 net bytes per instance | Reuses padding |

**The `eval_point_mask` field adds zero measurable cost to vertex processing.** It rides on data that was already being read (the instance buffer), adds no new buffer bindings, adds at most 1 move instruction to the vertex shader, and uses the cheapest possible attribute interpolation mode.

---


> **Part of**: Helio Radiant v2 Material Pipeline Specification  
> **Supersedes**: `GBufferPass::template_registry`, `TransparentPass` fixed shader, per-pass `RadiantShaderCache`, CPU-side `material_class_ranges` iteration in every fragment pass  
> **Status**: Draft  
> **Design goal**: Every fragment pass that evaluates materials declares exactly one eval point, issues a single `multi_draw_indexed_indirect` for all instances, and lets the per-pixel dispatch mask filter irrelevant geometry at zero cost.

---

## Table of Contents

1. [GBuffer Pass](#1-gbuffer-pass)
2. [Transparent Pass](#2-transparent-pass)
3. [Shadow Pass](#3-shadow-pass)
4. [Depth Prepass](#4-depth-prepass)
5. [Velocity Pass](#5-velocity-pass)
6. [Common Pattern Across All Fragment Passes](#6-common-pattern-across-all-fragment-passes)
7. [Depth Occlusion Consideration](#7-depth-occlusion-consideration)
8. [PSO Compilation](#8-pso-compilation)

---

## 1. GBuffer Pass

### 1.1 Eval Point Declaration

The `GBufferPass` declares a single eval point `"gbuffer"` during graph construction. The `register_eval_points` implementation is the canonical example for all fragment passes:

```rust
// crates/helio/passes/gbuffer/src/lib.rs

impl RenderPass for GBufferPass {
    fn name(&self) -> &'static str {
        "GBufferPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "gbuffer",
            index: 0, // assigned by registry
            entry_fn_signature:
                "fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData",
            entry_fn_name: "", // derived from signature
            base_shader: include_str!("../../shaders/gbuffer_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::Opaque,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }
}
```

**Key details:**

| Field | Value | Rationale |
|---|---|---|
| `name` | `"gbuffer"` | Matches convention: eval point name is the string after `eval_` in the entry function |
| `entry_fn_signature` | Returns `SurfaceData` | Carries albedo, normal, roughness, metallic, F0, emissive, subsurface, flags |
| `blend_mode` | `Opaque` | Gbuffer targets are written directly — no blending between materials |
| `depth_state.depth_write` | `true` | GBuffer pass writes depth (or relies on depth prepass; see §4) |
| `depth_state.depth_compare` | `Less` | Standard depth-test: frontmost fragments pass |
| `pipeline_layout` | `Some(...)` | Created once during pass construction from the base shader's bind group layouts |

### 1.2 Base Shader Structure

The base shader `gbuffer_base.wgsl` is the template into which material-specific `eval_gbuffer` bodies are spliced. It defines:

1. **Bindings** — all `@group(N) @binding(M)` the GBuffer pass provides
2. **Vertex shader** — `vs_main` transforms positions and passes instance data through
3. **Default `eval_gbuffer`** — a standard PBR surface evaluation using the material's textures
4. **Fragment shader** — `fs_main` with the eval point mask discard pattern and multi-target output

```wgsl
// ── gbuffer_base.wgsl ──────────────────────────────────────────────────
// Base shader for the "gbuffer" evaluation point.
// Material overrides only replace the body of fn eval_gbuffer().
// The eval_point_mask discard + template_id_tex write are part of the
// fixed plumbing and are never replaced.

// ── Constants ──────────────────────────────────────────────────────────
// Injected by the composition system:
//   const GBUFFER_EVAL_POINT = 0u;
//   const THIS_EVAL_POINT = GBUFFER_EVAL_POINT;

// ── Bindings ───────────────────────────────────────────────────────────

// Group 0: Per-frame data
@group(0) @binding(0) var<uniform> camera: CameraData;
@group(0) @binding(1) var<uniform> globals: GlobalsData;

// Group 1: Instance & material data
@group(1) @binding(0) var<storage, read> instance_buffer: array<GpuInstanceData>;
@group(1) @binding(1) var<storage, read> material_buffer: array<GpuMaterialData>;
@group(1) @binding(2) var<storage, read> mesh_buffer: array<GpuMeshData>;

// Group 1: Texture arrays
@group(1) @binding(3) var albedo_tex_array: texture_2d_array<f32>;
@group(1) @binding(4) var normal_tex_array: texture_2d_array<f32>;
@group(1) @binding(5) var orm_tex_array: texture_2d_array<f32>;
@group(1) @binding(6) var emissive_tex_array: texture_2d_array<f32>;
@group(1) @binding(7) var sampler_linear: sampler;

// ── Vertex input — per-vertex attributes ───────────────────────────────
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) tangent: vec4<f32>,

    // Per-instance attributes (InputStepMode::Instance)
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(9) normal_row0: vec4<f32>,
    @location(10) normal_row1: vec4<f32>,
    @location(11) normal_row2: vec4<f32>,
    @location(12) bounds: vec4<f32>,
    @location(13) mesh_id: u32,
    @location(14) material_id: u32,
    @location(15) flags: u32,
    @location(16) lightmap_index: u32,
    @location(17) template_id:     u32,  // ← from GpuInstanceData (§1.1)
}

// ── Vertex output (passed to fragment shader) ─────────────────────────
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position:  vec3<f32>,
    @location(1) world_normal:    vec3<f32>,
    @location(2) tex_coords:      vec2<f32>,
    @location(3) @interpolate(flat) material_id: u32,
    @location(4) @interpolate(flat) template_id:     u32,  // ← flat-interpolated
}

// ── GBuffer output targets ────────────────────────────────────────────
struct GBufferOutput {
    @location(0) albedo:    vec4<f32>,   // RGBA8Unorm
    @location(1) normal:    vec4<f32>,   // RGBA16Float
    @location(2) orm:       vec4<f32>,   // RGBA8Unorm
    @location(3) emissive:  vec4<f32>,   // RGBA16Float
    @location(4) template_id: u32,    // R32Uint  ← NEW: for compute passes
}

// ── Vertex shader ────────────────────────────────────────────────────
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let normal_mat = mat3x4<f32>(
        input.normal_row0, input.normal_row1, input.normal_row2,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    output.clip_position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.world_normal = normalize(normal_mat * vec4<f32>(input.normal, 0.0));
    output.tex_coords = input.tex_coord;
    output.material_id = input.material_id;
    output.eval_point_mask = input.eval_point_mask;  // ← passthrough

    return output;
}

// ── Default surface evaluation (PBR) ─────────────────────────────────
// This is the function body that materials override.
// The default implementation performs a standard PBR texture lookup.
fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData {
    let mat = material_buffer[material_id];
    var s: SurfaceData;

    // Albedo
    let albedo_tex = textureLoad(albedo_tex_array, vec2i(uv * vec2f(textureDimensions(albedo_tex_array))), i32(mat.albedo_index), 0);
    s.albedo = albedo_tex;

    // Normal map
    let nm_tex = textureLoad(normal_tex_array, vec2i(uv * vec2f(textureDimensions(normal_tex_array))), i32(mat.normal_index), 0);
    let tangent_nml = nm_tex.xyz * 2.0 - 1.0;
    // ... TBN transform ...
    s.normal = normalize(normal + tangent_nml * mat.normal_strength);

    // ORM: occlusion, roughness, metallic
    let orm_tex = textureLoad(orm_tex_array, vec2i(uv * vec2f(textureDimensions(orm_tex_array))), i32(mat.orm_index), 0);
    s.occlusion = orm_tex.r;
    s.roughness = orm_tex.g;
    s.metallic = orm_tex.b;

    // Specular F0 from metallic workflow
    let f0_dielectric = vec3f(0.04);
    s.specular_f0 = mix(f0_dielectric, s.albedo.rgb, s.metallic);

    // Emissive
    let em_tex = textureLoad(emissive_tex_array, vec2i(uv * vec2f(textureDimensions(emissive_tex_array))), i32(mat.emissive_index), 0);
    s.emissive = em_tex.rgb;
    s.emissive_intensity = mat.emissive_intensity;

    return s;
}

// ── Fragment shader ──────────────────────────────────────────────────
@fragment
fn fs_main(input: VertexOutput) -> GBufferOutput {
    // ── Eval point mask check (see §6.2) ──────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation RADIANT_OVERRIDE_START ────────────
    let surface = eval_gbuffer(input.material_id, input.world_position,
                               input.world_normal, input.tex_coords);
    // ── RADIANT_OVERRIDE_END ──────────────────────────────────

    // ── Pack to gbuffer targets ──────────────────────────────
    var output: GBufferOutput;
    output.albedo = surface.albedo;
    // Normal encode: view-space normal in XY, roughness in Z, flag in W
    output.normal = vec4<f32>(surface.normal * 0.5 + 0.5,
                              surface.roughness);
    output.orm = vec4<f32>(surface.occlusion,
                           surface.roughness,
                           surface.metallic,
                           0.0);
    output.emissive = vec4<f32>(surface.emissive * surface.emissive_intensity,
                                0.0);
    output.template_id = input.material_id;  // ← write for compute passes

    return output;
}
```

### 1.3 Default `eval_gbuffer` Behavior

The default `eval_gbuffer` performs a **full PBR surface evaluation**:

1. **Albedo**: samples `albedo_tex_array` at the interpolated UV, using `material.albedo_index` to select the array layer
2. **Normal**: samples `normal_tex_array`, decodes tangent-space normal, transforms to world space via the interpolated `world_normal` and a TBN construction
3. **ORM**: samples `orm_tex_array` — occlusion in R, roughness in G, metallic in B
4. **F0**: computed from the metallic workflow — `mix(0.04, albedo, metallic)`
5. **Emissive**: samples `emissive_tex_array`, multiplied by `material.emissive_intensity`

Materials that override `eval_gbuffer` can replace any or all of this logic. For example, a clear-coat material might sample additional textures, a skin material might blend subsurface color, and a debug material might output flat colors for visualization.

### 1.4 Eval Point Mask Check Integration

The mask check sits **before** the material evaluation, at the top of `fs_main`:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
    discard;
}
```

**Placement is critical.** It must appear:
- After `VertexOutput` is received (mask is in `input`)
- Before any texture samples or ALU work
- Before the `RADIANT_OVERRIDE_START` marker

Because it is outside the override region, **no material can accidentally remove the check**. The check is part of the base shader's fixed plumbing, not the material's eval function. Materials cannot control whether the check happens — they can only control what happens after the check passes.

**How it interacts with the existing gbuffer write logic:**

1. If `mask & GBUFFER_BIT == 0` → `discard` → no gbuffer writes, no `SurfaceData` evaluation, no `template_id_tex` write
2. If `mask & GBUFFER_BIT != 0` → proceed to `eval_gbuffer()` → pack result → write all 5 render targets including `material_class`

This means materials that do NOT target `"gbuffer"` (e.g., a water material targeting only `"transparent"`) will:
- Be rasterized (to write depth for occlusion — see §7)
- Hit the `discard` before any texture sampling
- Write nothing to any gbuffer target
- Contribute 4 bytes per pixel of bandwidth (the mask read) + the discard instruction

Materials that DO target `"gbuffer"` pay exactly the same cost as v1 — the mask check is a never-taken branch for them (§6.2).

### 1.5 Template ID texture Write (R32Uint Target)

The Template ID texture is written at `@location(4)` of the gbuffer fragment shader output:

```wgsl
struct GBufferOutput {
    @location(0) albedo:    vec4<f32>,   // 4 bytes/pixel
    @location(1) normal:    vec4<f32>,   // 8 bytes/pixel (RGBA16Float)
    @location(2) orm:       vec4<f32>,   // 4 bytes/pixel
    @location(3) emissive:  vec4<f32>,   // 8 bytes/pixel (RGBA16Float)
    @location(4) template_id: u32,    // 4 bytes/pixel (R32Uint)
}
```

**Texture specification:**

| Property | Value |
|---|---|
| Format | `R32Uint` |
| Dimensions | `config.width × config.height` |
| Mips | 1 |
| Samples | 1 |
| Usage | `RENDER_ATTACHMENT \| TEXTURE_BINDING` |
| Clear value | `(0, 0, 0, 0)` — material class 0 is "unassigned" |

**When is the material class written?** Only when the fragment passes the mask check AND the material evaluation succeeds. If the material discards internally (e.g., alpha-test clip), the material class is NOT written — the pixel retains whatever was there from the clear (unassigned).

**Compute pass reads:** Downstream compute passes (SSR, SSS, decal, post-process) read this texture via `textureLoad` to determine which material class is at each pixel:

```wgsl
// In a compute pass:
@group(N) @binding(M) var template_id_tex: texture_2d<u32>;

fn get_material_class(pixel: vec2<u32>) -> u32 {
    return textureLoad(template_id_tex, pixel, 0).r;
}
```

### 1.6 GBuffer Output Format — What Stays the Same

The v2 GBuffer pass keeps the **exact same 4 render targets** as v1, plus the new material class target:

| Target | Index | Format | v1 | v2 | Change |
|---|---|---|---|---|---|
| Albedo | 0 | `RGBA8Unorm` | ✅ | ✅ | None |
| Normal+ORM | 1 | `RGBA16Float` | ✅ | ✅ | None (XYZ = view-space normal, W = roughness) |
| ORM+ | 2 | `RGBA8Unorm` | ✅ | ✅ | None (R=occlusion, G=roughness, B=metallic, A=unused) |
| Emissive | 3 | `RGBA16Float` | ✅ | ✅ | None (RGB=emissive, A=intensity) |
| **Material class** | **4** | **`R32Uint`** | ❌ | **✅** | **NEW — 4 bytes/pixel** |

**Total gbuffer memory:** v1 = 24 bytes/pixel, v2 = 28 bytes/pixel (+16%).

**Backward compatibility:** Existing shaders that write to targets 0-3 are unchanged. The only new requirement is that `fs_main` must have a 5th output slot. If a material doesn't target `"gbuffer"`, it never reaches the write — the `discard` handles everything.

---

## 2. Transparent Pass

### 2.1 Redesign from v1

In v1, the transparent pass used a **fixed shader** with no material customization. Glass, water, and other transparent materials were forced into the gbuffer's opaque pipeline or required a separate custom pass.

In v2, the transparent pass is a full eval point pass. Materials targeting `"transparent"` provide `fn eval_transparent(...) -> vec4f`, and the pass composes their shader just like the gbuffer pass does. The fixed-function overhead (blending mode, depth state) is specified in the eval point declaration, not hardcoded in the shader.

### 2.2 Eval Point Declaration

```rust
impl RenderPass for TransparentPass {
    fn name(&self) -> &'static str {
        "TransparentPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "transparent",
            index: 0,
            entry_fn_signature:
                "fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f",
            entry_fn_name: "",
            base_shader: include_str!("../../shaders/transparent_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::AlphaBlend,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write: false,       // ← read-only depth
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }
}
```

**Key differences from the gbuffer declaration:**

| Field | GBuffer | Transparent | Rationale |
|---|---|---|---|
| `blend_mode` | `Opaque` | `AlphaBlend` | Transparent surfaces composite over the existing framebuffer |
| `depth_state.depth_write` | `true` | `false` | Gbuffer already wrote depth; transparent pass reads only |
| `depth_state.depth_compare` | `Less` | `LessEqual` | See note below |
| `entry_fn_signature` | Returns `SurfaceData` | Returns `vec4f` | Transparent pass writes a single RGBA output, not a surface struct |

**Depth comparison: `LessEqual` vs `Less`**. Transparent pass uses `LessEqual` because the gbuffer pass already wrote depth for opaque geometry at the same positions. If the transparent pass uses `Less`, coplanar transparent geometry (e.g., a glass pane exactly at an opaque surface) would fail the depth test. `LessEqual` allows transparent surfaces at the same depth as opaque surfaces to still be rendered (and blended over them).

### 2.3 Base Shader Structure

The transparent base shader is simpler than the gbuffer base shader — it has no texture array bindings (materials can declare their own via material parameter buffers if needed), writes a single color target, and has alpha blending configured by the fixed-function pipeline state.

```wgsl
// ── transparent_base.wgsl ─────────────────────────────────────────────
// Base shader for the "transparent" evaluation point.
// No texture arrays are needed — materials access textures through their
// own material parameter bindings or the global material buffer.

// ── Constants ──────────────────────────────────────────────────────────
// Injected: const TRANSPARENT_EVAL_POINT = 1u;
// Injected: const THIS_EVAL_POINT = TRANSPARENT_EVAL_POINT;

// ── Bindings ───────────────────────────────────────────────────────────

// Group 0: Per-frame data
@group(0) @binding(0) var<uniform> camera: CameraData;
@group(0) @binding(1) var<uniform> globals: GlobalsData;

// Group 1: Instance & material data
@group(1) @binding(0) var<storage, read> instance_buffer: array<GpuInstanceData>;
@group(1) @binding(1) var<storage, read> material_buffer: array<GpuMaterialData>;

// ── Vertex input (same layout as GBuffer) ────────────────────────────
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    // Per-instance attributes (identical to gbuffer)
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(9) normal_row0: vec4<f32>,
    @location(10) normal_row1: vec4<f32>,
    @location(11) normal_row2: vec4<f32>,
    @location(12) bounds: vec4<f32>,
    @location(13) mesh_id: u32,
    @location(14) material_id: u32,
    @location(15) flags: u32,
    @location(16) lightmap_index: u32,
    @location(17) template_id:     u32,
}

// ── Vertex output ────────────────────────────────────────────────────
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position:  vec3<f32>,
    @location(1) world_normal:    vec3<f32>,
    @location(2) tex_coords:      vec2<f32>,
    @location(3) @interpolate(flat) material_id: u32,
    @location(4) @interpolate(flat) template_id:     u32,
}

// ── Vertex shader (identical to gbuffer's) ───────────────────────────
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let normal_mat = mat3x4<f32>(
        input.normal_row0, input.normal_row1, input.normal_row2,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    output.clip_position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.world_normal = normalize(normal_mat * vec4<f32>(input.normal, 0.0));
    output.tex_coords = input.tex_coord;
    output.material_id = input.material_id;
    output.eval_point_mask = input.eval_point_mask;

    return output;
}

// ── Default transparent evaluation ───────────────────────────────────
// Simple semi-transparent tint — materials override this.
fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f {
    let mat = material_buffer[material_id];
    // Default: use the material's base color with a fixed alpha
    return vec4f(mat.base_color.rgb, 0.5);
}

// ── Fragment shader (single color output) ────────────────────────────
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // ── Eval point mask check ──────────────────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation RADIANT_OVERRIDE_START ────────────
    let color = eval_transparent(input.material_id, input.world_position,
                                 input.world_normal, input.tex_coords);
    // ── RADIANT_OVERRIDE_END ──────────────────────────────────

    return color;
}
```

### 2.4 Alpha Blending Configuration

The transparent pass uses **SrcAlpha / OneMinusSrcAlpha** blending, configured in the eval point's pipeline state, not in the shader:

```rust
// Pipeline descriptor for the transparent pass:
let blend_state = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,       // keep source alpha
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};
```

**Result:** `final_color = src.rgb * src.a + dst.rgb * (1 - src.a)`

**Important:** The material's `eval_transparent` must return **pre-multiplied alpha** values. The base shader does NOT pre-multiply for the material. If the material returns `vec4f(r, g, b, a)`, the GPU blend unit applies `src_factor = SrcAlpha`, which multiplies the color by alpha again. To get correct results, materials should either:
1. Return non-pre-multiplied colors (the GPU does the multiplication), OR
2. Use `BlendFactor::One` for the color source factor (for pre-multiplied alpha inputs)

**Current choice: Non-pre-multiplied.** The engine uses `SrcAlpha` as the color source factor, and materials return straight (non-pre-multiplied) alpha colors. This is the most intuitive for material authors.

### 2.5 Draw Iteration

The transparent pass issues a **single `multi_draw_indexed_indirect`** for ALL instances, exactly like the gbuffer pass:

```rust
fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
    let mut rpass = ctx.begin_render_pass("TransparentPass", &self.color_attachment, ...);

    // ── Single draw for ALL instances ──────────────────────────
    // No CPU-side iteration over material_class_ranges.
    // No per-range pipeline switching.
    // The GPU-side eval_point_mask handles per-pixel filtering.
    rpass.set_pipeline(&self.pipeline);
    rpass.set_bind_group(0, &self.frame_bind_group, &[]);
    rpass.set_bind_group(1, &self.scene_bind_group, &[]);
    rpass.set_vertex_buffer(0, self.vertex_buffer.slice(...));
    rpass.set_index_buffer(self.index_buffer, ...);
    rpass.multi_draw_indexed_indirect(&self.indirect_buffer, 0, 1);
    //                                              ^^^^^^^ — ONE draw, all instances

    Ok(())
}
```

**Compare to v1:**

- v1: iterate `material_class_ranges`, for each range that contains transparent materials, bind the fixed transparent shader pipeline, issue a draw
- v2: one draw, per-pixel filter via `eval_point_mask`, per-pixel material eval via the composed shader variant

The performance argument: v1 had fewer draw calls but no material customization. v2 has one draw call with per-pixel branching. The branching is uniform within each draw group (same mask value per triangle), so divergence is zero. The material evaluation path is the same price as v1's fixed shader — both evaluate one function per pixel.

### 2.6 The Discard Pattern for Non-Transparent Materials

In the transparent pass, materials that do NOT target `"transparent"` hit the discard:

```wgsl
// For a basic PBR material (mask = 0x01):
// (0x01 & (1u << TRANSPARENT_EVAL_POINT))
// = (0x01 & 0x02) = 0x00 → discard

// For a glass material (mask = 0x03 = gbuffer + transparent):
// (0x03 & (1u << TRANSPARENT_EVAL_POINT))
// = (0x03 & 0x02) = 0x02 → pass → evaluate eval_transparent
```

The discard in the transparent pass:
- Prevents any color output (fragment is killed before ROP)
- Does NOT update the depth buffer (`depth_write = false` in the eval point)
- Happens after early-Z (fragments occluded by the gbuffer depth are culled before ever reaching the discard)
- Cost: 3 scalar instructions (`and`, `test`, predicated `discard`)

**Why not skip drawing non-transparent geometry entirely?** Two reasons:
1. **Single draw simplicity:** The engine issues one draw for all instances. There is no CPU-side filtering to determine which instances target which pass. The GPU handles it.
2. **Depth occlusion:** Even though non-transparent materials don't write color in the transparent pass, their geometry still occludes fragments behind them. The depth buffer (written in the gbuffer or depth prepass) is used for early-Z culling. If we didn't draw the geometry at all, fragments that should be occluded by opaque geometry would "bleed through" in the transparent pass. However, since the depth buffer already has the correct opaque depth, early-Z handles occlusion before the fragment shader runs — the geometry still needs to be rasterized for the depth test to work (see §7).

**Optimization note:** In the future, if profiling shows a benefit, the engine could split the indirect draw into groups by mask (e.g., one draw for non-transparent, one for transparent). This would let non-transparent geometry skip the transparent pass entirely. But this is an optimization, not a correctness requirement.

### 2.7 Depth Sorting

Transparent surfaces must be rendered in **back-to-front order** for correct alpha blending. The engine handles this via **CPU-side sorting** before the transparent pass.

#### 2.7.1 Sort Location

Sorting happens during `rebuild_instance_buffers()` or immediately before the transparent pass's `execute()`. The engine maintains a **separate sorted instance buffer** for the transparent pass (or sorts in-place if the original order is not needed — which it is, for the gbuffer pass).

```rust
fn prepare_transparent_instances(&mut self) {
    // ── Collect only instances targeting transparent ─────────────
    // (This is the only CPU-side pass-specific filtering in v2)
    let transparent_instances: Vec<SortEntry> = self.scene
        .visible_instances
        .iter()
        .filter(|inst| {
            let mat = self.material_records[inst.material_class_id as usize];
            (mat.eval_point_mask & (1u << TRANSPARENT_EVAL_POINT_BIT)) != 0
        })
        .map(|inst| {
            let view_pos = camera_position(inst.world_bounds_center);
            SortEntry {
                depth: view_pos.z,           // camera-space Z distance
                // ... or view-space distance: distance(camera_pos, center)
                instance: inst.to_gpu_data(),
                material_class_id: inst.material_class_id,
            }
        })
        .collect();

    // ── Sort back-to-front (descending depth) ───────────────────
    transparent_instances.sort_unstable_by(|a, b| {
        b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Build indirect draw for sorted instances ────────────────
    self.transparent_indirect_buffer = build_indirect(&transparent_instances);
    self.transparent_instance_buffer.upload(&transparent_instances);
}
```

**Performance analysis:** For a scene with 1000 visible instances, of which ~100 are transparent, this is:
- 1000 iterations to filter (linear scan — fast, cache-friendly)
- 100 entries sorted (O(100 log 100) ≈ 700 comparisons — negligible)
- 100 instances uploaded to the GPU instance buffer (~16 KB)

**Alternative: GPU-side sort.** If depth sorting becomes a bottleneck (unlikely for typical scenes), the engine could use a GPU-based sort (radix sort on depth). This is a future optimization.

#### 2.7.2 Sorting Granularity

Instances are sorted by their **camera-space depth** (Z coordinate after view transform). For single-mesh objects, this is the bounding sphere center in view space. For multi-mesh objects, each mesh is sorted independently.

**Trade-off: instance-level sort vs. mesh-level sort.**

| Granularity | Correctness | Cost | Complexity |
|---|---|---|---|
| Per-instance (bounding center) | Good enough for most cases | O(N log N) sort | Simple |
| Per-mesh (each mesh within an instance separately) | Better for overlapping sub-meshes | O(M log M), M ≥ N | More indirect draws |
| Per-triangle / per-pixel (depth sort) | Perfect | GPU-side A-buffer + resolve | Significant complexity |

**v2 uses per-instance sorting**, which matches v1's approach. Per-mesh or per-triangle sorting is a future enhancement.

#### 2.7.3 Interaction with Eval Point Mask

Note the filtering step: we only include instances whose material targets the transparent eval point. This means:
- Opaque-only materials are NOT in the transparent instance buffer
- They are not drawn in the transparent pass at all
- They do NOT contribute to depth occlusion in the transparent pass (but their depth from the gbuffer pass is already in the depth buffer)

This is an optimization over the "draw everything, discard in shader" approach described in §2.6. The transparent pass specifically **opts out** non-transparent materials at the draw level because:
- Not drawing = less vertex processing, less rasterization, less bandwidth
- Depth occlusion is already handled by the depth buffer from the gbuffer pass (early-Z)

**The cost of this optimization:** O(N) filter on CPU, plus a separate instance buffer upload. This is acceptable because the transparent pass already pays a sorting cost — the filter is essentially free in comparison.

#### 2.7.4 Sorting Stability

If two transparent instances have the same depth (identical camera-space Z), their relative order is undefined (the sort is not stable). This is acceptable:
- Overlapping coplanar transparent surfaces are rare
- When they do occur, any order produces a correct blend (both surfaces are visible through each other)
- The visual difference between the two orders is usually imperceptible

---

## 3. Shadow Pass

### 3.1 Eval Point Declaration

```rust
impl RenderPass for ShadowPass {
    fn name(&self) -> &'static str {
        "ShadowPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "shadow",
            index: 0,
            entry_fn_signature:
                "fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool",
            entry_fn_name: "",
            base_shader: include_str!("../../shaders/shadow_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::None,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float, // or Depth24PlusStencil8
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }
}
```

| Field | Value | Rationale |
|---|---|---|
| `blend_mode` | `None` | Shadow pass writes to a depth-only/depth-stencil texture — no color output |
| `depth_write` | `true` | Shadow maps need depth for light-space visibility testing |
| `depth_compare` | `Less` | Standard shadow map depth test (frontmost occluder wins) |
| `entry_fn_signature` | Returns `bool` | `true` = this fragment casts shadow; `false` = clip (discard) |

### 3.2 Base Shader Structure

The shadow base shader is the **most minimal** of all fragment eval points. It has only the bindings necessary to transform geometry into light space and optionally alpha-test the shadow:

```wgsl
// ── shadow_base.wgsl ──────────────────────────────────────────────────
// Minimal base shader for the "shadow" evaluation point.
// Depth-only rendering into the shadow atlas.

// ── Constants ──────────────────────────────────────────────────────────
// Injected: const SHADOW_EVAL_POINT = 2u;
// Injected: const THIS_EVAL_POINT = SHADOW_EVAL_POINT;

// ── Bindings ───────────────────────────────────────────────────────────
// Group 0: Light camera data (unique per shadow cascade / atlas tile)
@group(0) @binding(0) var<uniform> light_camera: LightCameraData;

// Group 1: Instance data (only what we need for transform + mask)
@group(1) @binding(0) var<storage, read> instance_buffer: array<GpuShadowInstanceData>;

// ── Vertex input ──────────────────────────────────────────────────────
struct VertexInput {
    @location(0) position: vec3<f32>,

    // Per-instance — position-only transform is enough
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(17) template_id:     u32,  // mask only, no material_id needed
}

// ── Vertex output ────────────────────────────────────────────────────
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) @interpolate(flat) template_id:     u32,
}

// ── Vertex shader (light-space transform only) ────────────────────────
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    // Transform directly into light clip space
    output.clip_position = light_camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.eval_point_mask = input.eval_point_mask;

    return output;
}

// ── Default shadow evaluation ────────────────────────────────────────
// Returns true (casts shadow) by default.
// Materials override to implement alpha-test shadow casting.
fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool {
    return true;  // default: always cast shadow
}

// ── Fragment shader (depth-only output) ──────────────────────────────
@fragment
fn fs_main(input: VertexOutput) {
    // ── Eval point mask check ──────────────────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation RADIANT_OVERRIDE_START ────────────
    if (!eval_shadow(input.material_id, input.world_position)) {
        discard;  // material alpha-test failed / doesn't cast shadow
    }
    // ── RADIANT_OVERRIDE_END ──────────────────────────────────

    // No color output — depth is written automatically by the
    // fixed-function depth buffer based on clip_position.z
}
```

### 3.3 `eval_shadow` Returns bool

The `eval_shadow` function returns a **bool** — `true` means "cast shadow at this fragment," `false` means "discard this fragment from the shadow map."

**Default behavior:** `true` — every solid fragment casts a shadow.

**Override use cases:**

1. **Alpha-tested shadows (foliage, fences):**
   ```wgsl
   fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool {
       let uv = compute_world_uv(world_pos);
       let alpha = textureSampleLevel(alpha_tex, sampler_linear, uv, 0.0).r;
       return alpha > 0.5;  // discard transparent/alpha-clipped regions
   }
   ```

2. **Shadow-only materials (invisible collision proxies):**
   ```wgsl
   fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool {
       return true;  // always casts shadow, invisible in all other passes
   }
   ```

3. **Transparent shadow rejection (glass, water):**
   ```wgsl
   fn eval_shadow(material_id: u32, world_pos: vec3f) -> bool {
       return false;  // transparent materials don't cast opaque shadows
   }
   ```

**Interaction with `discard`:** When `eval_shadow` returns `false`, the fragment shader calls `discard`, which:
- Prevents the depth write for this fragment
- The shadow map retains whatever was there (previous occluder or cleared value)
- Net effect: this fragment does NOT cast a shadow

### 3.4 Working with the Existing Shadow Atlas System

The shadow atlas system remains unchanged from v1. The shadow pass writes into a multi-tile atlas where each tile represents a shadow cascade or spotlight shadow map.

**What changes:**
- Instead of iterating `material_class_ranges` and drawing each class individually, the pass issues a single multi-draw for each atlas tile
- The `eval_point_mask` handles per-pixel shadow casting
- Materials can opt in/out of shadow casting at the eval point level

**What stays the same:**
- The shadow atlas texture format (`Depth32Float` or `Depth24PlusStencil8`)
- The cascade/spotlight view-projection matrices per tile
- The atlas packing and resolution assignment
- The clear-before-draw per tile

```rust
fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
    for (tile_index, cascade) in self.shadow_cascades.iter().enumerate() {
        let mut rpass = ctx.begin_render_pass(
            &format!("ShadowCascade_{}", tile_index),
            &RenderPassDescriptor {
                color_attachments: &[],  // no color output
                depth_stencil_attachment: Some(self.atlas_target(tile_index)),
            },
        );

        // ── Light camera for this cascade ────────────────────
        rpass.set_bind_group(0, &cascade.bind_group, &[]);
        rpass.set_bind_group(1, &self.scene_bind_group, &[]);
        rpass.set_pipeline(&self.pipeline);
        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(...));
        rpass.set_index_buffer(self.index_buffer, ...);
        rpass.multi_draw_indexed_indirect(&self.indirect_buffer, 0, 1);

        // (v1: iterated ranges, drew each class separately)
        // (v2: one draw, per-pixel discard handles filtering)
    }
    Ok(())
}
```

---

## 4. Depth Prepass

### 4.1 Purpose

The depth prepass runs **before** the gbuffer pass to establish a depth buffer for **Early-Z culling**. By writing depth first, the gbuffer pass can skip fragment shading for occluded fragments, reducing pixel shader invocations by 30-90% depending on overdraw.

In v1, the depth prepass iterated `material_class_ranges` and wrote depth for all opaque geometry. In v2, it is a full eval point pass with the same single-draw, per-pixel mask pattern.

### 4.2 Eval Point Declaration

```rust
impl RenderPass for DepthPrepass {
    fn name(&self) -> &'static str {
        "DepthPrepass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "depth_prepass",
            index: 0,
            entry_fn_signature:
                "fn eval_depth(world_pos: vec3f) -> f32",
            entry_fn_name: "",
            base_shader: include_str!("../../shaders/depth_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::None,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }
}
```

| Field | Value | Rationale |
|---|---|---|
| `blend_mode` | `None` | No color output — depth-only pass |
| `depth_write` | `true` | This is the primary depth-writing pass |
| `depth_compare` | `Less` | Standard: closest fragment survives |
| `entry_fn_signature` | Returns `f32` | Custom depth value (default: identity) |

### 4.3 Minimal Shader

The depth prepass base shader is the most minimal possible fragment shader:

```wgsl
// ── depth_base.wgsl ───────────────────────────────────────────────────
// Minimal base shader for the "depth_prepass" evaluation point.
// Writes depth only — no color output, no texture sampling by default.

// ── Constants ──────────────────────────────────────────────────────────
// Injected: const DEPTH_PREPASS_EVAL_POINT = 3u;
// Injected: const THIS_EVAL_POINT = DEPTH_PREPASS_EVAL_POINT;

// ── Bindings ───────────────────────────────────────────────────────────
// Group 0: Camera (needed for view-proj matrix)
@group(0) @binding(0) var<uniform> camera: CameraData;

// Group 1: Instances
@group(1) @binding(0) var<storage, read> instance_buffer: array<GpuInstanceData>;

// ── Vertex input ──────────────────────────────────────────────────────
struct VertexInput {
    @location(0) position: vec3<f32>,

    // Per-instance — only the model matrix + mask
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(17) template_id:     u32,
}

// ── Vertex output ────────────────────────────────────────────────────
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) @interpolate(flat) template_id:     u32,
}

// ── Vertex shader ────────────────────────────────────────────────────
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    output.clip_position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.eval_point_mask = input.eval_point_mask;

    return output;
}

// ── Default depth evaluation ─────────────────────────────────────────
// Returns the clip-space Z from the world position.
// The default implementation just passes through — the depth is whatever
// clip_position.z / clip_position.w produces.
fn eval_depth(world_pos: vec3f) -> f32 {
    // Default: identity — no depth modification.
    // The return value is ignored by the default implementation;
    // the depth is written automatically by the GPU based on
    // clip_position.z in the vertex shader output.
    //
    // Materials that override this can return a different depth
    // value (e.g., for parallax occlusion mapping).
    return 0.0; // ignored by default path
}

// ── Fragment shader ──────────────────────────────────────────────────
@fragment
fn fs_main(input: VertexOutput) {
    // ── Eval point mask check ──────────────────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation RADIANT_OVERRIDE_START ────────────
    // Note: for the default case, this function does nothing.
    // The depth write is handled by the fixed-function rasterizer
    // from clip_position.z. The eval_depth function is only needed
    // when a material wants to modify the depth (e.g., parallax).
    let custom_depth = eval_depth(input.world_position);

    // ── Override the fragment's depth if material requested it ─
    // (The default eval_depth returns 0.0, and the check below
    //  ensures we only override when explicitly requested.)
    if (custom_depth != 0.0) {
        // Write custom depth by modifying the fragment's depth value.
        // This requires the @builtin(frag_depth) output which WGSL
        // allows in the fragment shader's return type.
    }
    // ── RADIANT_OVERRIDE_END ──────────────────────────────────

    // No color output — depth is written automatically
}
```

### 4.4 `eval_depth` Returns f32

The `eval_depth` function returns an `f32` that represents a **custom depth value** for the fragment. The default implementation returns `0.0` (meaning "use the standard clip-space depth, don't override").

**Use cases for custom depth:**

| Use Case | Description | `eval_depth` Implementation |
|---|---|---|
| **Default** | Use the GPU's automatic depth from clip_position.z | `return 0.0;` |
| **Parallax Occlusion Mapping** | Offset the fragment's depth based on a heightmap | `return sample_heightmap(uv) * 0.1;` |
| **Procedural Displacement** | Modify depth based on a noise function | `return noise(world_pos.xz) * 0.05;` |
| **Hair/Thin Geometry** | Adjust depth to match the fur shell's base | `return fur_layer_depth * shell_index;` |

**Implementation note:** Writing `@builtin(frag_depth)` requires declaring it in the fragment shader output:

```wgsl
// Fragment shader that writes custom depth:
struct DepthOutput {
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs_main(input: VertexOutput) -> DepthOutput {
    // ... mask check ...
    let custom_depth = eval_depth(input.world_position);

    var output: DepthOutput;
    if (custom_depth != 0.0) {
        // Convert custom depth from world-space offset to clip-space depth
        // This is material-specific — the material provides the formula
        output.depth = compute_parallax_depth(input.clip_position, custom_depth);
    } else {
        output.depth = input.clip_position.z / input.clip_position.w;
    }

    return output;
}
```

**Performance:** The depth prepass is intentionally minimal — no texture sampling, no complex math, no color writes. On most GPUs, the throughput is limited by vertex transform (geometry throughput) rather than fragment shading. The depth prepass is typically 2-3× faster than the gbuffer pass purely because it avoids texture reads and multiple render target writes.

### 4.5 Early-Z Optimization for the GBuffer Pass

The depth prepass enables the gbuffer pass to use **Early-Z culling** (also called Hi-Z culling). When the depth buffer is written before the gbuffer pass:

1. **Depth prepass**: writes depth for all opaque geometry
2. **GBuffer pass**: draws the same geometry, but now every fragment first checks against the already-written depth buffer
3. **Fragments occluded by the depth prepass** are culled before the fragment shader runs — no `eval_gbuffer` call, no gbuffer write
4. **Fragments that pass the depth test** proceed to evaluate the gbuffer material as normal

**Interaction with eval_point_mask in the depth prepass:** The depth prepass writes depth for ALL instances that target `"depth_prepass"`. Most materials target this by default (it's in the default set — see §7). Materials that do NOT target `"depth_prepass"` will not have depth written for them during the prepass — they must write their own depth during the gbuffer pass (which will be less efficient).

---

## 5. Velocity Pass

### 5.1 Purpose

The velocity pass writes per-pixel **motion vectors** for Temporal Anti-Aliasing (TAA) and motion blur post-processing. Each pixel stores the 2D screen-space displacement (in UV coordinates) from the previous frame's position to the current frame's position.

In v1, the velocity pass was either absent or handled by a separate fixed-shader pass. In v2, it is a full eval point pass.

### 5.2 Eval Point Declaration

```rust
impl RenderPass for VelocityPass {
    fn name(&self) -> &'static str {
        "VelocityPass"
    }

    fn register_eval_points(&self, registry: &mut EvalPointRegistry) {
        registry.register(EvalPoint {
            name: "velocity",
            index: 0,
            entry_fn_signature:
                "fn eval_velocity(world_pos_prev: vec3f) -> vec2f",
            entry_fn_name: "",
            base_shader: include_str!("../../shaders/velocity_base.wgsl"),
            pipeline_layout: Some(self.pipeline_layout.clone()),
            compute_layout: None,
            blend_mode: BlendMode::None,
            depth_state: Some(DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: None,
            }),
            threading: ThreadingModel::Fragment,
        });
    }
}
```

| Field | Value | Rationale |
|---|---|---|
| `blend_mode` | `None` | Overwrite velocity per-pixel — no blending |
| `depth_write` | `false` | Depth is already written by gbuffer/depth prepass |
| `depth_compare` | `LessEqual` | Write velocity for the same visible surfaces |
| `entry_fn_signature` | Returns `vec2f` | 2D screen-space velocity in UV space |
| Output | Single `Rg16Float` target | Velocity stored as 2-component 16-bit float |

### 5.3 Output Texture

The velocity pass writes to a single render target:

| Property | Value |
|---|---|
| Format | `RG16Float` |
| Components | R = U velocity, G = V velocity |
| Range | [-1, 1] in UV space (clamped) |
| Usage | `RENDER_ATTACHMENT \| TEXTURE_BINDING` |
| Clear value | `(0, 0, 0, 0)` — zero velocity (static) |

**Encoding:** Velocity is stored as `(current_uv - previous_uv)` in UV space, where UV coordinates range from 0 to 1. A value of `(0.5, 0.0)` means the pixel moved by half the screen width to the right. TAA and motion blur passes use this directly for reprojection and blur direction.

### 5.4 Base Shader Using Previous Frame's View-Projection Matrix

The key difference from other passes: the velocity shader needs **both** the current frame's and previous frame's view-projection matrices. The previous frame's matrix is stored in the camera uniform buffer and updated every frame.

```wgsl
// ── velocity_base.wgsl ────────────────────────────────────────────────
// Base shader for the "velocity" evaluation point.
// Writes per-pixel screen-space motion vectors.

// ── Constants ──────────────────────────────────────────────────────────
// Injected: const VELOCITY_EVAL_POINT = 8u;
// Injected: const THIS_EVAL_POINT = VELOCITY_EVAL_POINT;

// ── Bindings ───────────────────────────────────────────────────────────
@group(0) @binding(0) var<uniform> camera: CameraData;
// CameraData contains:
//   view_proj: mat4x4<f32>,       // current frame
//   prev_view_proj: mat4x4<f32>,  // previous frame
//   ... (other camera data)

// Group 1: Instance data (only what we need)
@group(1) @binding(0) var<storage, read> instance_buffer: array<GpuInstanceData>;

// ── Vertex input ──────────────────────────────────────────────────────
struct VertexInput {
    @location(0) position: vec3<f32>,

    // Per-instance
    @location(5) model_row0: vec4<f32>,
    @location(6) model_row1: vec4<f32>,
    @location(7) model_row2: vec4<f32>,
    @location(8) model_row3: vec4<f32>,
    @location(9) normal_row0: vec4<f32>,
    @location(10) normal_row1: vec4<f32>,
    @location(11) normal_row2: vec4<f32>,
    // ... (other instance data up to location 17 for mask)
    @location(17) template_id:     u32,
}

// ── Vertex output ────────────────────────────────────────────────────
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position:       vec3<f32>,
    @location(1) world_position_prev:  vec3<f32>,  // ← world position in previous frame
    @location(2) @interpolate(flat) template_id:     u32,
}

// ── Vertex shader ────────────────────────────────────────────────────
// Computes both current and previous world positions.
// Note: this assumes a static model matrix for simplicity.
// For skinned/animated meshes, the previous frame's model matrix
// would come from a separate per-instance buffer.
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    // Current frame: standard transform
    output.clip_position = camera.view_proj * world_pos;

    // Previous frame: use the same world position (for static geometry)
    // For skinned meshes, this would use the skeleton's previous-frame
    // bone matrices.
    output.world_position = world_pos.xyz;
    output.world_position_prev = world_pos.xyz;  // ← same for static
    output.eval_point_mask = input.eval_point_mask;

    return output;
}

// ── Default velocity evaluation ──────────────────────────────────────
// Computes screen-space velocity by reprojecting the previous world
// position through the previous frame's view-projection matrix.
fn eval_velocity(world_pos_prev: vec3f) -> vec2f {
    // Project previous world position into previous clip space
    let prev_clip = camera.prev_view_proj * vec4<f32>(world_pos_prev, 1.0);
    let prev_ndc = prev_clip.xy / prev_clip.w;

    // Current NDC position is from the vertex shader's clip_position
    // (passed implicitly via builtin(position))
    // We compute it from the current view-proj for clarity
    let current_clip = camera.view_proj * vec4<f32>(world_pos_prev, 1.0);
    let current_ndc = current_clip.xy / current_clip.w;

    // Convert from NDC [-1, 1] to UV [0, 1] velocity
    // (current_uv - prev_uv) where uv = ndc * 0.5 + 0.5
    return (current_ndc - prev_ndc) * 0.5;
}

// ── Fragment shader ──────────────────────────────────────────────────
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec2<f32> {
    // ── Eval point mask check ──────────────────────────────────
    let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
        discard;
    }

    // ── Material evaluation RADIANT_OVERRIDE_START ────────────
    let velocity = eval_velocity(input.world_position_prev);
    // ── RADIANT_OVERRIDE_END ──────────────────────────────────

    // Clamp to reasonable range to avoid extreme blur artifacts
    return clamp(velocity, vec2f(-1.0), vec2f(1.0));
}
```

### 5.5 Default `eval_velocity` Behavior

The default implementation:
1. Takes the **previous frame's world position** (passed from the vertex shader)
2. Projects it through `camera.prev_view_proj` to get previous-frame NDC
3. Computes current NDC from the same world position through `camera.view_proj`
4. Returns `(current_ndc - prev_ndc) * 0.5` to convert from [-1, 1] NDC difference to [0, 1] UV velocity

**For static geometry:** `world_position == world_position_prev` (same in vertex shader), so the velocity reflects camera movement only.

**For animated/skinned geometry:** The vertex shader would need access to previous-frame bone matrices to compute `world_position_prev` correctly. This requires a per-instance "previous model matrix" buffer, which is a future extension. In v2, skinned meshes can override `eval_velocity` to compute their own velocity from material parameters (e.g., storing the previous frame's position in a vertex attribute).

### 5.6 Material Override Examples

**Static object (default):**
```wgsl
fn eval_velocity(world_pos_prev: vec3f) -> vec2f {
    // Use default reprojection
    let prev_clip = camera.prev_view_proj * vec4f(world_pos_prev, 1.0);
    let prev_ndc = prev_clip.xy / prev_clip.w;
    let current_clip = camera.view_proj * vec4f(world_pos_prev, 1.0);
    let current_ndc = current_clip.xy / current_clip.w;
    return (current_ndc - prev_ndc) * 0.5;
}
```

**Animated object (custom velocity from material parameter):**
```wgsl
fn eval_velocity(world_pos_prev: vec3f) -> vec2f {
    // Read custom velocity from a material texture or parameter
    // (e.g., a particle system stores the velocity in a vertex buffer)
    let custom_velocity = material_buffer[material_id].custom_velocity;
    return custom_velocity.xy;  // pre-computed screen-space velocity
}
```

**Zero velocity (probes, static decals):**
```wgsl
fn eval_velocity(world_pos_prev: vec3f) -> vec2f {
    return vec2f(0.0, 0.0);  // no motion for this material
}
```

---

## 6. Common Pattern Across All Fragment Passes

### 6.1 Single Multi-Draw Indexed Indirect

Every fragment pass that evaluates materials issues exactly **one** `multi_draw_indexed_indirect` call that covers ALL instances in the scene (for that pass's sorted instance buffer).

```rust
// Every fragment pass's execute() follows this pattern:
fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
    let mut rpass = ctx.begin_render_pass(/* ... */);

    // ── Set bind groups once ──────────────────────────────────
    rpass.set_bind_group(0, &self.pass_bind_group, &[]);    // per-pass data
    rpass.set_bind_group(1, &self.scene_bind_group, &[]);   // shared scene data

    // ── Set pipeline once (per pass, not per range) ──────────
    rpass.set_pipeline(&self.pipeline);

    // ── Set vertex/index buffers once ────────────────────────
    rpass.set_vertex_buffer(0, self.merged_vertex_buffer.slice(...));
    rpass.set_index_buffer(self.merged_index_buffer, ...);

    // ── Issue ONE indirect multi-draw for ALL instances ──────
    rpass.multi_draw_indexed_indirect(&self.indirect_buffer, 0, 1);

    Ok(())
}
```

**The indirect buffer** contains a single `DrawIndexedIndirect` command:

```rust
struct DrawIndexedIndirect {
    index_count: u32,       // total indices across all meshes (PackedVertexBuffer)
    instance_count: u32,    // total instances across all meshes
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,    // 0 (start from the first instance)
}
```

**How merged vertex/index buffers work:** The engine packs all meshes into a single vertex buffer and index buffer during scene load. The `multi_draw_indexed_indirect` command covers the entire packed buffer. Each instance's `mesh_id` field in `GpuInstanceData` determines which index range and vertex range to use (via the mesh buffer lookup in the vertex shader — though in v2, this is typically baked into the indirect draw parameters).

**This replaces CPU-side range splitting entirely.** In v1, each pass would:
1. Iterate `material_class_ranges` (a Vec of (class, start, count) tuples)
2. For each range, bind the appropriate pipeline
3. Issue a single draw for that range

In v2, all of that is eliminated. The single multi-draw covers all instances. The per-pixel mask check in the fragment shader handles filtering.

### 6.2 The Discard Pattern

The discard pattern is **identical** across all fragment eval point base shaders:

```wgsl
// Top of every fs_main:
let mask = template_dispatches[input.template_id].eval_point_mask;
    if (mask & (1u << THIS_EVAL_POINT)) == 0u {
    discard;
}
```

Where `THIS_EVAL_POINT` is a compile-time constant injected by the composition system:

```rust
// Injected prefix for every composed shader:
format!(
    "const THIS_EVAL_POINT = {}u;\n",
    eval_point_index
)
```

**Result for each built-in eval point:**

| Eval Point | Index | `THIS_EVAL_POINT` | Generated Check |
|---|---|---|---|
| `gbuffer` | 0 | `0u` | `(mask & (1u << 0u)) == 0u` → `(mask & 1u) == 0u` |
| `transparent` | 1 | `1u` | `(mask & (1u << 1u)) == 0u` → `(mask & 2u) == 0u` |
| `shadow` | 2 | `2u` | `(mask & (1u << 2u)) == 0u` → `(mask & 4u) == 0u` |
| `depth_prepass` | 3 | `3u` | `(mask & (1u << 3u)) == 0u` → `(mask & 8u) == 0u` |
| `velocity` | 8 | `8u` | `(mask & (1u << 8u)) == 0u` → `(mask & 256u) == 0u` |

**Why `(mask & (1u << index)) == 0u` and not `(mask >> index) & 1u == 0u`?** Both compile to the same instruction on modern GPUs (a bit-test), but the left-hand form is more descriptive of the intent: "is the bit for this eval point not set?"

### 6.3 How This Replaces `material_class_ranges` Entirely

The `material_class_ranges` system in v1 served two purposes:
1. **CPU-side filtering:** Each pass iterated ranges to find materials relevant to it
2. **Pipeline binding:** Each range required a pipeline bind for that material class

In v2, both are eliminated:

| v1 Mechanism | v2 Replacement |
|---|---|
| `build_material_class_ranges()` during scene rebuild | **Eliminated.** Instance data carries `eval_point_mask`, no range building needed. |
| Per-pass iteration of ranges on CPU | **Eliminated.** Single multi-draw for all instances. |
| Per-range pipeline bind | **Eliminated.** One pipeline per pass (the composed variant). |
| Per-class shader selection (which shader to use for this class in this pass) | **Inherited from composition.** Each material has a composed variant per eval point, compiled at registration time. |
| Fixing `material_class_ranges` when materials change | **Eliminated.** Instance data is rebuilt from material records, not from ranges. |

**What is NOT eliminated:**
- The per-eval-point pipeline variant (still one per `(eval_point, template_id, graph_hash, flags)` — but this was also one per class in v1)
- The instance buffer upload (same as v1, just with an extra 4-byte field)

### 6.4 Vertex Shader is Identical Across All Fragment Eval Points

All five fragment eval points (gbuffer, transparent, shadow, depth_prepass, velocity) share the **same vertex shader structure**. The `vs_main` function performs:

1. **Model matrix reconstruction** from per-instance rows
2. **World-space transform** of position and normal
3. **Clip-space transform** via `camera.view_proj`
4. **Passthrough** of `eval_point_mask` and `material_id`

The differences between passes are:
- **Bind group 0**: Different per-pass uniforms (e.g., shadow uses `light_camera`, velocity has `prev_view_proj`)
- **Vertex output**: Some passes omit certain fields (e.g., shadow doesn't need `world_normal` or `tex_coords`)
- **Material ID**: Shadow and depth_prepass may not need material_id (shadow uses it for texture lookup in alpha test, depth_prepass doesn't need it)

**Standardized vertex shader** (used by gbuffer and transparent):

```wgsl
// This is the canonical vertex shader for fragment eval points.
// Shadow and depth_prepass use a simplified version (fewer outputs).
// Velocity uses an expanded version (extra world_position_prev output).
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let model = mat4x4<f32>(
        input.model_row0, input.model_row1,
        input.model_row2, input.model_row3,
    );
    let normal_mat = mat3x4<f32>(
        input.normal_row0, input.normal_row1, input.normal_row2,
    );
    let world_pos = model * vec4<f32>(input.position, 1.0);

    output.clip_position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.world_normal = normalize(normal_mat * vec4<f32>(input.normal, 0.0));
    output.tex_coords = input.tex_coord;
    output.material_id = input.material_id;
    output.eval_point_mask = input.eval_point_mask;

    return output;
}
```

**Cross-pass vertex shader reuse:** Because the vertex shader is the same across passes, the engine could in theory share compiled vertex shader code. In practice, each pass compiles its own full shader module (vertex + fragment), and the vertex code is duplicated. This is acceptable — WGSL compilation is fast, and the GPU driver caches the compiled binary internally anyway.

---

## 7. Depth Occlusion Consideration

### 7.1 The Core Problem

Consider a scene with:
- An opaque wall (material: PBR, targets: `"gbuffer"`)
- A glass sphere behind the wall (material: glass, targets: `"gbuffer"`, `"transparent"`)
- A waterfall behind the glass (material: water, targets: `"transparent"` only)

If we draw only `"gbuffer"`-targeting materials in the gbuffer pass and only `"transparent"`-targeting materials in the transparent pass, what happens?

1. **GBuffer pass**: draws wall + glass (both target `"gbuffer"`). Water does NOT draw.
2. **Transparent pass**: draws glass + water (both target `"transparent"`). Wall does NOT draw.

But the wall (opaque) should occlude the glass and water behind it. The depth buffer has the wall's depth from the gbuffer pass, so when the transparent pass draws glass behind the wall, early-Z culls those fragments. **This works correctly.**

**The problem case:** What if there is opaque geometry that does NOT target `"gbuffer"`? For example, a special "shadow-only" material that only targets `"shadow"`. In the gbuffer pass, this geometry is not drawn. In the transparent pass, it's also not drawn. So its depth is never written, and objects behind it will be visible through it in both passes.

### 7.2 Solution: All Geometry Writes Depth Somewhere

The invariant: **every visible instance must write its depth in at least one pass that runs before any color-blending pass.**

This means:
- Opaque materials: write depth in `"depth_prepass"` or `"gbuffer"`
- Transparent materials: MUST write depth in `"depth_prepass"` to occlude behind them
- Shadow-only materials: depth is NOT needed for camera visibility (they are invisible), but if they need to occlude, they should target `"depth_prepass"`

**The default set of eval points for common material types:**

| Material Type | `depth_prepass` | `gbuffer` | `transparent` | `shadow` | `velocity` | Rationale |
|---|---|---|---|---|---|---|
| Standard Opaque PBR | ✅ | ✅ | ❌ | ✅ | ✅ | Full participation |
| Transparent (glass) | ✅ | ✅ | ✅ | ❌ | ✅ | Depth for occlusion, normals for SSR, transparent color |
| Pure Transparent (water) | ✅ | ❌ | ✅ | ❌ | ✅ | Depth for occlusion, transparent color only |
| Alpha-Tested (foliage) | ✅ | ✅ | ❌ | ✅ | ✅ | Full + custom shadow casting |
| Shadow-Only (proxy) | ❌ | ❌ | ❌ | ✅ | ❌ | Invisible, casts shadow only |
| Decal | ❌ | ❌ | ✅ | ❌ | ❌ | Overlay on transparent pass |
| Editor Ghost | ❌ | ❌ | ✅ | ❌ | ❌ | Semi-transparent editor overlay |
| Post-Process | ❌ | ❌ | ❌ | ❌ | ❌ | Full-screen compute, no geometry |

**Key insight: Transparent materials must still write depth.** If water only targets `"transparent"`, it won't write depth in the gbuffer pass. To occlude objects behind it, it must target `"depth_prepass"` as well. The `eval_depth` function for water can be a no-op (return `0.0` — use standard clip-space depth), but the depth prepass will run the vertex shader for the water geometry and write its depth.

### 7.3 How `eval_point_mask` Handles This

The mask-based system makes this straightforward. A material targeting `"depth_prepass"`, `"gbuffer"`, and `"transparent"` has:

```
mask = (1 << DEPTH_PREPASS_BIT) | (1 << GBUFFER_BIT) | (1 << TRANSPARENT_BIT)
     = 0x01 | 0x02 | 0x08
     = 0x0B  (binary: 0000 1011)
```

In the depth prepass: check `mask & (1 << 3)` → `0x0B & 0x08` → true → write depth
In the gbuffer pass: check `mask & (1 << 0)` → `0x0B & 0x01` → true → write gbuffer
In the transparent pass: check `mask & (1 << 1)` → `0x0B & 0x02` → true → write transparent color

A water-only material targeting `"depth_prepass"` and `"transparent"`:

```
mask = 0x08 | 0x02
     = 0x0A  (binary: 0000 1010)
```

In the depth prepass: writes depth (occludes behind it)
In the gbuffer pass: `0x0A & 0x01` → false → discard (no gbuffer write)
In the transparent pass: writes transparent color

**The water material pays the cost of vertex transform in the gbuffer pass but discards in the fragment shader (after the predicated mask check).** This is acceptable:
- Vertex transform is cheap (no texture sampling, just matrix multiply)
- The discard happens before any texture read or ALU in `eval_gbuffer`
- The fragment shader executes only the mask check + discard — about 3 instructions

**Optimization:** If profiling shows that many materials skip gbuffer, the engine could build a separate instance buffer for the gbuffer pass that excludes non-targeting instances. This is a trade-off between CPU filter cost and GPU vertex transform cost. For now, the "draw everything, filter per-pixel" approach is simpler and correct.

### 7.4 Depth Prepass Participation

**Why all geometry should target `"depth_prepass"`:**

1. **Early-Z optimization:** The gbuffer pass can skip fragment shading for occluded pixels when depth is already written
2. **Transparent occlusion:** Transparent objects behind opaque objects are culled before reaching the transparent pass
3. **Consistency:** Every instance that writes to any color target should also write depth for correct occlusion ordering

**Exceptions:**
- Shadow-only proxies: invisible, no camera depth needed
- Decals: projected onto existing surfaces, not independent geometry
- Full-screen effects: no geometry to process

**Default material template:** All engine-provided material templates (PBR, glass, water, foliage, etc.) include `"depth_prepass"` by default. The editor automatically adds it to new materials.

### 7.5 Material Type Table — Eval Points

| # | Material Type | Example | `depth_prepass` | `gbuffer` | `transparent` | `shadow` | `velocity` | `ssr` | `sss` | Mask (hex) |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | **Standard Opaque** | Concrete, metal, wood | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | ❌ | `0x0000_001D` |
| 2 | **Alpha-Tested** | Foliage, chain fence | ✅ | ✅ | ❌ | ✅* | ✅ | ❌ | ❌ | `0x0000_001D` |
| 3 | **Simple Transparent** | Window glass | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | `0x0000_0113` |
| 4 | **Water** | Lake, river | ✅ | ❌ | ✅ | ❌ | ✅ | ❌ | ❌ | `0x0000_010A` |
| 5 | **Skin** | Character body | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | ✅ | `0x0000_002D` |
| 6 | **Glass** | Wine glass | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | `0x0000_0113` |
| 7 | **Mirror** | Reflection surface | ✅ | ✅ | ❌ | ✅ | ❌ | ✅ | ❌ | `0x0000_0015` |
| 8 | **Hair** | Character hair | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ | ❌ | `0x0000_001D` |
| 9 | **Shadow-Only** | Invisible geometry | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | `0x0000_0004` |
| 10 | **Decal** | Bullet hole | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | `0x0000_0002` |
| 11 | **Editor Ghost** | Selection overlay | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | `0x0000_0002` |
| 12 | **Post-Process FX** | Full-screen material | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | `0x0000_0000` |

\* Alpha-tested shadows require custom `eval_shadow` returning `bool` based on alpha test.

**Notes on the table:**
- Mask = `(depth_prepass << 3) \| (gbuffer << 0) \| (transparent << 1) \| (shadow << 2) \| (velocity << 8) \| (ssr << 4) \| (sss << 5)`
- All materials include `depth_prepass` unless they are intentionally invisible or overlay-only
- `velocity` is included by default for all geometry that moves (TAA needs velocity for every visible surface)
- `shadow` is included for all solid geometry (alpha-tested materials need custom eval_shadow)

---

## 8. PSO Compilation

### 8.1 Per-Eval-Point Pipeline Management

Each eval point has its own **pipeline layout** and **pipeline state** because:
- Different blend modes (gbuffer = Opaque, transparent = AlphaBlend, shadow = None)
- Different depth states (gbuffer = write, transparent = read-only, shadow = write)
- Different color target formats (gbuffer = 5 targets, transparent = 1 target, shadow = 0 targets)
- Different vertex shader outputs (some passes omit normal/UV, velocity adds previous position)
- Different fragment shader output structures

These differences are captured in the `EvalPoint` struct:

```rust
pub struct EvalPoint {
    // ...
    pub pipeline_layout: Option<wgpu::PipelineLayout>,
    pub blend_mode: BlendMode,
    pub depth_state: Option<DepthStencilState>,
    // ...
}
```

### 8.2 Pipeline Layout Creation

Each eval point's `pipeline_layout` is created from its `bind_group_layouts` during pass initialization:

```rust
fn create_pipeline_layout_for_eval_point(
    device: &wgpu::Device,
    eval_point: &EvalPoint,
    bind_group_layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("eval_point_{}_layout", eval_point.name)),
        bind_group_layouts,
        push_constant_ranges: &[],  // not used
    })
}
```

**The bind group layouts are:**

| Group | Contents | Shared Across Eval Points? |
|---|---|---|
| 0 | Per-pass uniforms (camera, light matrices, globals) | **No** — each pass has different uniforms |
| 1 | Scene instance buffer, material buffer, textures | **Yes** — shared by all fragment eval points |

**Why group 0 is NOT shared:**
- GBuffer: `camera`, `globals`, plus texture arrays
- Transparent: `camera`, `globals` (no texture arrays)
- Shadow: `light_camera` (different from camera)
- Depth: `camera` only
- Velocity: `camera` (with `prev_view_proj`)

**Why group 1 CAN be shared:**
- All fragment eval points read `instance_buffer` (same layout)
- All fragment eval points read `material_buffer` (same layout) — but not all bind it
- Textures are either in group 1 arrays or in material-specific bind groups

In practice, each eval point has its own `PipelineLayout` created from its own set of `BindGroupLayout`s.

### 8.3 Pipeline Cache Keyed by `ShaderVariantKey`

The pipeline cache maps `ShaderVariantKey` to `wgpu::RenderPipeline`:

```rust
/// Per-eval-point pipeline cache.
struct EvalPointPipelineCache {
    /// The layout for all pipelines in this eval point.
    pipeline_layout: wgpu::PipelineLayout,

    /// Compiled render pipelines, keyed by ShaderVariantKey.
    /// Key excludes eval_point (implicit from which cache this is).
    pipelines: HashMap<(u32, u64, u32), wgpu::RenderPipeline>,
    //           ^^^^^^^^^^^^^^^
    //           (template_id, graph_hash, feature_flags)
}
```

**Why the key excludes `eval_point`:** The pipeline cache is per-eval-point. If you're looking up a pipeline in the gbuffer cache, it's implicitly for eval point 0.

**Full lookup path:**

```rust
fn get_or_create_pipeline(
    &mut self,
    key: ShaderVariantKey,
    composed_wgsl: &str,
    eval_point: &EvalPoint,
    device: &wgpu::Device,
    config: &RenderConfig,
) -> &wgpu::RenderPipeline {
    let cache = &mut self.per_eval_caches[key.eval_point as usize];

    let inner_key = (key.template_id, key.graph_hash, key.feature_flags);

    // ── Cache hit? ────────────────────────────────────────────
    if let Some(pipeline) = cache.pipelines.get(&inner_key) {
        return pipeline;
    }

    // ── Cache miss — compile new pipeline ─────────────────────
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!(
            "material-{}:eval-{}",
            key.template_id, key.eval_point
        )),
        source: wgpu::ShaderSource::Wgsl(composed_wgsl.into()),
    });

    // ── Vertex state ───────────────────────────────────────────
    // (same vertex buffer layout for all fragment eval points)
    let vertex_state = wgpu::VertexState {
        module: &module,
        entry_point: "vs_main",
        buffers: &[
            // Per-vertex attributes (slot 0): position, normal, texcoord, tangent
            vertex_buffer_layout(),
            // Per-instance attributes (slot 1): model matrix rows, normal rows, ... mask
            instance_buffer_layout(),
        ],
    };

    // ── Fragment state ─────────────────────────────────────────
    let fragment_state = eval_point.fragment_state(&module, config);

    // ── Depth/stencil state ────────────────────────────────────
    let depth_stencil = eval_point.depth_state
        .as_ref()
        .map(|ds| ds.to_wgpu_depth_stencil_state());

    // ── Create pipeline ────────────────────────────────────────
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!(
            "pso-{}:{}", eval_point.name, key.template_id
        )),
        layout: Some(&cache.pipeline_layout),
        vertex: vertex_state,
        fragment: Some(fragment_state),
        depth_stencil,
        multisample: wgpu::MultisampleState {
            count: config.msaa_samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        // ... other fields ...
    });

    cache.pipelines.insert(inner_key, pipeline);
    cache.pipelines.get(&inner_key).unwrap()
}
```

### 8.4 Fragment State Construction from `EvalPoint`

The fragment state (color targets + blend modes) is derived from the eval point's `blend_mode` and the pass's render target configuration:

```rust
impl EvalPoint {
    fn fragment_state<'a>(
        &self,
        module: &'a wgpu::ShaderModule,
        config: &RenderConfig,
    ) -> wgpu::FragmentState<'a> {
        let blend = match self.blend_mode {
            BlendMode::Opaque => Some(wgpu::BlendState::REPLACE),
            BlendMode::AlphaBlend => Some(wgpu::BlendState::ALPHA_BLENDING),
            BlendMode::Add => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            BlendMode::Multiply => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::SrcColor,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::SrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            BlendMode::None => None,  // no color output (shadow, depth)
            BlendMode::Custom(bs) => Some(bs),
        };

        let targets: Vec<wgpu::ColorTargetState> = self.target_formats(config)
            .iter()
            .map(|&format| wgpu::ColorTargetState {
                format,
                blend: blend.clone(),
                write_mask: wgpu::ColorWrites::ALL,
            })
            .collect();

        wgpu::FragmentState {
            module,
            entry_point: "fs_main",
            targets: &targets,
        }
    }

    /// Returns the color target formats for this eval point.
    fn target_formats(&self, config: &RenderConfig) -> Vec<wgpu::TextureFormat> {
        match self.name {
            "gbuffer" => vec![
                wgpu::TextureFormat::Rgba8Unorm,     // target 0: albedo
                wgpu::TextureFormat::Rgba16Float,    // target 1: normal
                wgpu::TextureFormat::Rgba8Unorm,     // target 2: orm
                wgpu::TextureFormat::Rgba16Float,    // target 3: emissive
                wgpu::TextureFormat::R32Uint,        // target 4: material_class
            ],
            "transparent" => vec![
                wgpu::TextureFormat::Rgba16Float,    // target 0: color
            ],
            "shadow" | "depth_prepass" => vec![],     // no color targets
            "velocity" => vec![
                wgpu::TextureFormat::Rg16Float,       // target 0: velocity
            ],
            _ => vec![wgpu::TextureFormat::Rgba16Float], // default: single target
        }
    }
}
```

### 8.5 Lazy Compilation on First Use

Pipeline compilation follows this strategy:

| Compilation Timing | Eval Point | Rationale |
|---|---|---|
| **At material registration** | gbuffer, transparent | These passes always draw every frame; the compilation cost is upfront |
| **At material registration** | shadow | Shadow rendering happens every frame (or every Nth frame) — compile eagerly |
| **At material registration** | depth_prepass | Depth prepass runs every frame — compile eagerly |
| **Lazy (first draw)** | velocity | Optional pass (may be disabled if TAA is off) |

**Implementation:** The `register_material()` function eagerly compiles variants for all eval points except those explicitly marked as lazy:

```rust
fn register_material(..., targets: &[&str], ...) -> MaterialClassId {
    for target_name in targets {
        let eval_point = registry.get(target_name).unwrap();
        let composed = compose_fn_override(...);

        // Eagerly compile for fragment passes (not lazy)
        if matches!(eval_point.threading, ThreadingModel::Fragment) {
            let pipeline = compile_pipeline(device, &composed, eval_point, config);
            cache.insert(key, pipeline);
        }
        // Compute passes are compiled lazily (future)
    }
}
```

**Lazy compilation for velocity (if desired):**

```rust
fn get_or_create_velocity_pipeline(
    &mut self,
    material_id: u32,
) -> &wgpu::RenderPipeline {
    let key = ShaderVariantKey {
        eval_point: VELOCITY_EVAL_POINT_INDEX,
        template_id: material_id,
        graph_hash: 0,
        feature_flags: 0,
    };

    // Check cache first, compile on miss
    if !self.velocity_cache.contains_key(&key) {
        // Compile now — first frame that TAA/motion blur is enabled
        let composed = self.get_composed_shader(key);
        let pipeline = compile_pipeline(device, &composed, &self.velocity_eval_point, config);
        self.velocity_cache.insert(key, pipeline);
    }

    &self.velocity_cache[&key]
}
```

### 8.6 Pipeline Layout from Bind Group Layouts

Each eval point's pipeline layout is created from its `bind_group_layouts` during pass construction:

```rust
fn create_eval_point_pipeline_layouts(
    device: &wgpu::Device,
    eval_point_registry: &EvalPointRegistry,
) -> Vec<wgpu::PipelineLayout> {
    let mut layouts = Vec::with_capacity(eval_point_registry.count() as usize);

    for eval_point in eval_point_registry.points() {
        match eval_point.threading {
            ThreadingModel::Fragment => {
                // The pass creates its bind group layouts and stores them
                // during its own initialization. The eval point's pipeline
                // layout is derived from those layouts.
                //
                // In practice, each pass constructs its pipeline layout
                // inside its ::new() or ::on_resize() and stores it in
                // the EvalPoint struct before registration.
                layouts.push(eval_point.pipeline_layout.clone().unwrap());
            }
            ThreadingModel::Compute { .. } => {
                layouts.push(eval_point.compute_layout.clone().unwrap());
            }
        }
    }

    layouts
}
```

**Binding model for fragment eval points:**

```
Group 0: Per-pass bindings (unique to each eval point)
  ├── binding 0: uniform camera data (or light_camera for shadow)
  ├── binding 1: uniform globals (frame count, time, etc.)
  └── binding 2+: pass-specific (texture arrays for gbuffer, etc.)

Group 1: Scene bindings (shared across all fragment eval points)
  ├── binding 0: storage instance_buffer[]
  ├── binding 1: storage material_buffer[]
  ├── binding 2+: texture arrays, samplers (not all used by all passes)
```

**Why not share bind group 1 across all passes?** Theoretically, yes — all fragment eval points use the same instance buffer layout. In practice, each pass has its own bind group 1 because some passes bind additional resources (e.g., gbuffer binds 4 texture arrays + sampler in group 1). Sharing the bind group would require all passes to bind all resources, which wastes API calls.

**A future optimization:** Create a single "scene" bind group that contains all scene-level resources. All passes share this bind group. Pass-specific resources go in group 0. This reduces bind group changes but requires a fixed binding layout that all passes agree on.

### 8.7 Pipeline State Comparison Table

| Property | GBuffer | Transparent | Shadow | Depth Prepass | Velocity |
|---|---|---|---|---|---|
| **Color targets** | 5 (RGBA8, RGBA16F, RGBA8, RGBA16F, R32U) | 1 (RGBA16F) | 0 | 0 | 1 (RG16F) |
| **Blend mode** | Opaque | AlphaBlend (SrcAlpha/1-SrcAlpha) | None | None | None |
| **Depth write** | true | false | true | true | false |
| **Depth compare** | Less | LessEqual | Less | Less | LessEqual |
| **Vertex shader** | Full (pos+nml+uv+tan) | Full (pos+nml+uv+tan) | Minimal (pos only) | Minimal (pos only) | Full + prev_pos |
| **Fragment shader output** | GBufferOutput | vec4f | void (depth-only) | void (depth-only) | vec2f |
| **MSAA** | configurable | configurable | 1 (shadow atlas) | configurable | 1 (velocity) |
| **Eval function** | `eval_gbuffer → SurfaceData` | `eval_transparent → vec4f` | `eval_shadow → bool` | `eval_depth → f32` | `eval_velocity → vec2f` |

### 8.8 Pipeline Compilation Error Handling

If a pipeline compilation fails (invalid WGSL, bind group mismatch, etc.), the error must propagate clearly:

```rust
fn compile_pipeline(
    device: &wgpu::Device,
    composed_wgsl: &str,
    eval_point: &EvalPoint,
    config: &RenderConfig,
) -> Result<wgpu::RenderPipeline, PipelineCompileError> {
    // ── Create shader module ─────────────────────────────────
    let module = device.create_shader_module(
        wgpu::ShaderModuleDescriptor {
            label: Some(&format!(
                "pso-{}:material-{}",
                eval_point.name,
                "temporary"
            )),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(composed_wgsl)),
        }
    );

    // ── Check for compilation errors ─────────────────────────
    // WGPU does not expose shader compilation errors directly
    // from create_shader_module in the default path. The error
    // surfaces as a WGPU log message or via the async pipeline
    // compilation API.
    //
    // In practice, WGSL compilation errors are caught during
    // development by testing shader compilation before shipping.
    // At runtime, pipeline creation is expected to succeed if
    // the material was validated at registration time.

    // ── Build pipeline descriptor ────────────────────────────
    // ... (same as §8.3) ...

    // ── Create render pipeline ───────────────────────────────
    device.create_render_pipeline(&descriptor);

    // Success — return the pipeline
}
```

**Error reporting strategy:** If a pipeline creation fails (which manifests as a `wgpu::DeviceError` or a panic from WGPU), the engine should:
1. Log the composed WGSL to the error output (for debugging)
2. Fall back to a default pipeline (e.g., magenta error color for gbuffer)
3. Continue rendering (the missing material appears as an error color, but the rest of the scene renders)

This is future work and not specified further here.

---

## Appendix: Index Table for Built-in Fragment Eval Points

| Index | Name | Pass | Base Shader | Entry Function | Return Type | Color Targets | Blend | Depth Write | Depth Compare |
|---|---|---|---|---|---|---|---|---|---|
| 0 | `gbuffer` | GBufferPass | `gbuffer_base.wgsl` | `eval_gbuffer` | `SurfaceData` | 5 | Opaque | true | Less |
| 1 | `transparent` | TransparentPass | `transparent_base.wgsl` | `eval_transparent` | `vec4f` | 1 | AlphaBlend | false | LessEqual |
| 2 | `shadow` | ShadowPass | `shadow_base.wgsl` | `eval_shadow` | `bool` | 0 | None | true | Less |
| 3 | `depth_prepass` | DepthPrepass | `depth_base.wgsl` | `eval_depth` | `f32` | 0 | None | true | Less |
| 8 | `velocity` | VelocityPass | `velocity_base.wgsl` | `eval_velocity` | `vec2f` | 1 (RG16F) | None | false | LessEqual |

**Total fragment eval points:** 5 (indices 0, 1, 2, 3, 8) — indices 4-7 are reserved for compute eval points (ssr, sss, decal, post_process).

---

## Appendix: Vertex Shader Variants Cheat Sheet

| Pass | `VertexOutput` fields | `VertexInput` locations used | Notes |
|---|---|---|---|
| GBuffer | pos, world_pos, world_normal, tex_coords, material_id, mask | 0,1,2,3,5..17 | Full output |
| Transparent | pos, world_pos, world_normal, tex_coords, material_id, mask | 0,1,2,3,5..17 | Identical to gbuffer |
| Shadow | pos, world_pos, mask | 0,5..8,17 | Minimal (just position + model matrix + mask) |
| Depth Prepass | pos, world_pos, mask | 0,5..8,17 | Minimal (same as shadow) |
| Velocity | pos, world_pos, world_pos_prev, mask | 0,1,5..17 | Adds `world_pos_prev` location |

---


> **Helio Radiant v2** — Compute-based eval-point dispatch for all per-pixel
> post-gbuffer effects (SSR, SSS, decals, per-pixel post-processing).
>
> **Design principle:** One `R32Uint` texture written by the gbuffer pass
> provides per-pixel material-class IDs. A storage-buffer dispatch table maps
> each class to a bitmask of eval points. Every compute shader that implements
> an eval point follows an identical early-return pattern:
>
> ```
> read material_class → index dispatch table → check bit → return if unset → run
> ```

---

## Table of Contents

1. [Template ID texture](#1-material-class-texture)
2. [GpuTemplateDispatch Table](#2-GpuTemplateDispatch-table)
3. [Compute Shader Pattern](#3-compute-shader-pattern)
4. [SSR Pass](#4-ssr-pass)
5. [SSS Pass](#5-sss-pass)
6. [Decal Pass](#6-decal-pass)
7. [Post-Process Pass](#7-post-process-pass)
8. [Workgroup Divergence Mitigation](#8-workgroup-divergence-mitigation)
9. [Template ID texture Resolution](#9-material-class-texture-resolution)
10. [Multiple Passes Sharing the Same Texture](#10-multiple-passes-sharing-the-same-texture)

---

## 1. Template ID texture

### 1.1 Purpose

The gbuffer fragment shader writes a material-class ID into an additional
render target (`R32Uint`). Every subsequent compute pass reads this texture to
decide whether the current pixel needs to execute its eval function.

### 1.2 Texture Properties

| Property | Value |
|---|---|
| Format | `R32Uint` |
| Resolution | Internal render resolution (width × height) |
| Mip levels | 1 |
| Sample count | 1 |
| Binding | `@group(0) @binding(0) var template_id_tex: texture_2d<u32>;` |

### 1.3 Writing in the Gbuffer Pass

The gbuffer fragment shader includes an additional output:

```wgsl
// === Fragment output structure ===
struct GbufferOutput {
    @location(0) albedo : vec4<f32>,       // RGBA — albedo + metalness in alpha
    @location(1) normal : vec4<f32>,       // view-space normal (packed)
    @location(2) pbr    : vec4<f32>,       // roughness, ao, etc.
    @location(3) emissive : vec4<f32>,     // emissive color + intensity
    @location(4) @interpolate(flat) material_class : u32,
};

// === Vertex output (passed through to fragment) ===
struct VertexOutput {
    @builtin(position) position : vec4<f32>,
    // ... standard varyings (uv, normal, tangent, etc.) ...
    @location(4) @interpolate(flat) material_class : u32,
};
```

The vertex shader obtains `material_class` from the draw call's material
instance data. The fragment shader forwards it verbatim:

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> GbufferOutput {
    var output: GbufferOutput;
    // ... standard gbuffer calculations ...

    output.template_id = in.material_class;
    return output;
}
```

**Important:** `@interpolate(flat)` is mandatory. Without flat interpolation
the GPU would try to perspectively interpolate a `u32`, which is illegal (or
worse, silently reinterpreted). Flat interpolation ensures every fragment
covered by the triangle receives the same value from the provoking vertex,
which is exactly what we need — material class is uniform across the triangle.

### 1.4 Undefined Pixels

Pixels that are **discarded** in the gbuffer (e.g., alpha-tested foliage that
fails the alpha test, or pixels written by a non-gbuffer material path) will
have an **undefined** value in `template_id_tex`. This happens because:

- The gbuffer render pass clears the texture to `0` at the start of the frame.
- Discarded fragments do not write to any render target, leaving the cleared
  value `0` in place.
- Material class `0` is reserved as the **null/invalid** class. Its entry in
  `GpuTemplateDispatch` has `eval_point_mask = 0`, so all compute passes
  early-return for these pixels.

This is correct by construction as long as class `0` is never assigned to a
real material. The `Renderer` ensures this when building the dispatch table.

### 1.5 GPU Memory Cost

```
memory = 4 bytes × width × height
```

| Resolution | Memory |
|---|---|
| 1920 × 1080 (1080p) | 8.0 MB |
| 2560 × 1440 (1440p) | 14.1 MB |
| 3840 × 2160 (4K) | 31.7 MB |
| 1920 × 1080 × 0.5 (half-res) | 2.0 MB |

This is a trivially small texture. The primary concern is not memory but
**bandwidth**: every compute pass reads the full texture once.

---

## 2. GpuTemplateDispatch Table

### 2.1 Purpose

A GPU-readable storage buffer that maps template IDs to a bitmask of
enabled eval points. The Renderer populates this buffer during scene rebuild
(see [Crates & Subsystems](03-crates.md) for the rebuild lifecycle).

### 2.2 GPU Layout

```wgsl
struct GpuTemplateDispatch {
    eval_point_mask : u32,         // bitmask of enabled eval points
    _pad : vec3<u32>,              // 12 bytes padding → 16-byte aligned
}
```

Each entry is **16 bytes** (matching `std140`-style alignment for storage
buffers on all modern GPUs). The `_pad` field ensures the stride is a power
of two, which helps with buffer alignment and potential future expansion.

**Total buffer size:** `16 bytes × max_material_classes`.

### 2.3 Binding

```wgsl
@group(1) @binding(N) var<storage, read> template_dispatches : array<GpuTemplateDispatch>;
```

The binding slot `N` varies per compute pass but is consistent within a given
pipeline layout. All compute passes in Helio Radiant v2 share `group(1)` for
per-pass read-only data.

### 2.4 Eval Point Bit Assignments

| Bit | Eval Point | Shader |
|---|---|---|
| 0 | `ssr` | SSR compute pass |
| 1 | `sss` | SSS compute pass |
| 2 | `decal` | Decal compute pass |
| 3 | `post_process` | Post-process compute pass |
| 4–31 | *reserved* | Future eval points |

### 2.5 CPU-side Population

The `Renderer` subsystem iterates all visible materials during scene rebuild
and builds the dispatch table. Materials that reference the same class ID are
combined with a bitwise OR of their eval-point masks.

```rust
// Pseudocode for CPU-side population
fn rebuild_template_dispatches(materials: &[MaterialInstance]) -> Vec<GpuTemplateDispatch> {
    let mut table = vec![GpuTemplateDispatch::default(); MAX_MATERIAL_CLASSES];
    for mat in materials {
        let mut mask = 0u32;
        if mat.has_ssr()  { mask |= 1 << 0; }
        if mat.has_sss()  { mask |= 1 << 1; }
        if mat.has_decal_mask() { mask |= 1 << 2; }
        if mat.has_post_effect() { mask |= 1 << 3; }
        table[mat.class_id as usize].eval_point_mask |= mask;
    }
    staging_buf.write_slice(&table);
    upload_to_gpu(&staging_buf, &gpu_buf);
}
```

---

## 3. Compute Shader Pattern

### 3.1 Universal Skeleton

Every compute pass that implements an eval point follows this exact pattern:

```wgsl
// === Imports (example, varies by pass) ===
@group(0) @binding(0) var template_id_tex : texture_2d<u32>;
@group(1) @binding(0) var template_dispatches : array<GpuTemplateDispatch>;

struct GpuTemplateDispatch {
    eval_point_mask : u32,
    _pad : vec3<u32>,
};

// === Push constant or specialization constant for the eval-point bit ===
// (Conceptual — WGSL doesn't have push constants natively yet)
// const EVAL_BIT = 0u;  // overridden per-pass (0=ssr, 1=sss, etc.)

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id : vec3<u32>) {
    let dimensions = textureDimensions(template_id_tex);
    if (any(id.xy >= dimensions.xy)) { return; }

    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;

    // Early return if this pixel's material does not target this eval point
    if (mask & (1u << SSR_BIT)) == 0u { return; }

    // ... eval function specific to this pass ...
}
```

### 3.2 Workgroup Size Considerations

| Size | Threads | Pros | Cons |
|---|---|---|---|
| 4×4 | 16 | Minimal divergence; fits small workloads | High dispatch overhead; poor utilization of large textures |
| 8×8 | 64 | **Good balance** — recommended default | Moderate divergence |
| 16×16 | 256 | Low dispatch overhead; good for uniform workloads | High divergence cost; can exceed per-wavefront limits on older HW |
| 32×8 | 256 | Wide for horizontal coherence | Uneven x/y; complex boundary logic |
| 32×32 | 1024 | Maximum occupancy potential | Very high divergence; may exceed threadgroup memory limits |

**Recommendation: 8×8** for all compute eval-point passes. This is the default
throughout Helio Radiant v2.

### 3.3 Why Early-Return is Cheap

Modern GPUs execute shader invocations in **wavefronts** (AMD) or **warps**
(NVIDIA) — groups of 32 or 64 threads that execute in lockstep. When some
threads in a wavefront take the early return while others continue, the GPU:

1. Executes both paths within the wavefront (masked execution).
2. Threads that took the early-return branch are **masked out** for the
   remainder of the shader body.
3. There is **no branch misprediction penalty** because there's no branch
   predictor — both sides execute, but only active lanes write results.

The cost is that non-active lanes still consume **issue slots** for the
instructions they skip (via the mask). In practice, for a shader dominated by
texture sampling and arithmetic, the masked-out lanes complete quickly because
they take the trivial early-return path (a few scalar ops). The overall
impact on occupancy is negligible.

### 3.4 Divergence Analysis

When adjacent pixels belong to different material classes:

- If *all* pixels in the wavefront have the eval-point bit set → **no divergence**.
- If *none* of the pixels have the bit set → **no divergence** (all return early).
- If **some** pixels have the bit and others don't → **divergence**.

During divergence, the wavefront executes:

1. First, the **early-return path** (masked: threads without the bit are
   active → they jump past the body).
2. Then, the **eval-function body** (masked: threads with the bit are active →
   they execute the full shader).

Because the early-return path is a single `if` check (a few scalar ALU
instructions), the cost of divergence is dominated by whether the eval-function
body is expensive or cheap. SSR, for example, has a long loop for Hi-Z ray
marching, which makes divergence costly if only a few pixels in a wavefront
need SSR. **Section 8** covers mitigation strategies.

---

## 4. SSR Pass (Bit 0)

### 4.1 Purpose

Screen-space reflections computed per-pixel for materials that have the `ssr`
eval point enabled. Each pixel receives SSR only if its material class's mask
has bit 0 set.

### 4.2 Binding Layout

```wgsl
@group(0) @binding(0) var template_id_tex : texture_2d<u32>;
@group(0) @binding(1) var gbuffer_albedo    : texture_2d<f32>;
@group(0) @binding(2) var gbuffer_normal    : texture_2d<f32>;
@group(0) @binding(3) var gbuffer_pbr       : texture_2d<f32>;
@group(0) @binding(4) var depth_tex         : texture_2d<f32>;
@group(0) @binding(5) var ssr_output        : texture_storage_2d<rgba16f, write>;

@group(1) @binding(0) var template_dispatches : array<GpuTemplateDispatch>;
@group(1) @binding(1) var ssr_params : SsrUniforms;

struct SsrUniforms {
    projection_matrix : mat4x4<f32>,    // inverse projection for ray construction
    step_count : u32,                   // max Hi-Z steps
    ray_length : f32,                   // max ray distance in world units
    depth_thickness : f32,              // thickness threshold for self-intersection
    pixel_stride : f32,                 // initial stride in pixels
};
```

### 4.3 Eval Function

```wgsl
const SSR_BIT : u32 = 0u;

struct SsrParams {
    roughness : f32,
    f0 : f32,                // Fresnel reflectance at normal incidence
    normal : vec3<f32>,      // view-space normal
    position : vec3<f32>,    // view-space position
    depth : f32,             // linear depth
};

// === Per-material SSR parameter extraction ===
fn eval_ssr(material_class : u32, uv : vec2<f32>) -> SsrParams {
    let pbr_texel  = textureLoad(gbuffer_pbr, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_pbr))), 0);
    let normal_tex = textureLoad(gbuffer_normal, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_normal))), 0);
    let depth_val  = textureLoad(depth_tex, ivec2(uv * vec2<f32>(textureDimensions(depth_tex))), 0).r;

    var params : SsrParams;
    params.roughness = pbr_texel.r;             // roughness from gbuffer PBR channel
    params.f0        = pbr_texel.g;             // f0 from gbuffer PBR channel (packed)
    params.normal    = decode_normal(normal_tex);
    params.depth     = linearize_depth(depth_val, ssr_params.projection_matrix);
    params.position  = reconstruct_position(uv, params.depth, ssr_params.projection_matrix);
    return params;
}
```

### 4.4 Main Shader Body

```wgsl
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id : vec3<u32>) {
    let dimensions = textureDimensions(template_id_tex);
    if (any(id.xy >= dimensions.xy)) { return; }

    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << SSR_BIT)) == 0u { return; }

    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(dimensions.xy);
    let ssr_params = eval_ssr(material_class, uv);

    // === Hi-Z screen-space ray march ===
    let ray_start = ssr_params.position;
    let ray_dir   = reflect(normalize(ray_start), ssr_params.normal);
    let reflected_uv = hi_z_ray_march(ray_start, ray_dir,
                                       ssr_params.projection_matrix,
                                       ssr_params.step_count,
                                       ssr_params.ray_length,
                                       ssr_params.depth_thickness);

    // === Resolve ===
    var result : vec4<f32>;
    if (reflected_uv.x >= 0.0) {
        // Sample the scene color at the reflected UV coordinate
        let reflected_color = textureSampleLevel(scene_color, scene_sampler, reflected_uv, 0.0);

        // Fresnel term (Schlick approximation)
        let fresnel = ssr_params.f0 + (1.0 - ssr_params.f0) * pow(1.0 - max(dot(ssr_params.normal, -ray_dir), 0.0), 5.0);

        // Roughness-based mip bias for blurring rough reflections
        let mip_level = ssr_params.roughness * ssr_params.roughness * 8.0;
        let rough_color = textureSampleLevel(scene_color, scene_sampler, reflected_uv, mip_level);

        result = vec4<f32>(mix(rough_color.rgb, reflected_color.rgb, 1.0 - ssr_params.roughness) * fresnel, 1.0);
    }

    textureStore(ssr_output, id.xy, result);
}
```

### 4.5 What Happens When SSR is Not Targeted

For pixels whose material class does NOT have the SSR bit set, the shader
returns before any texture loads beyond `template_id_tex` and
`template_dispatches`. This means:

- No gbuffer texture reads for those pixels.
- No Hi-Z ray march.
- No `textureStore` to `ssr_output` — the output texture retains its
  **cleared value** (typically `vec4<f32>(0.0)` for multiplication-based
  blending, or `vec4<f32>(1.0, 0.0, 0.0, 0.0)` for additive).

The SSR output texture is composited later during the lighting resolve.

### 4.6 Hi-Z Ray March Integration

The existing `hi_z_ray_march` function operates on the Hi-Z depth mip chain
(a pre-built hierarchy of the depth buffer). The function signature is:

```wgsl
fn hi_z_ray_march(
    ray_origin  : vec3<f32>,     // view-space ray origin
    ray_dir     : vec3<f32>,     // view-space ray direction (normalized)
    proj        : mat4x4<f32>,   // projection matrix
    max_steps   : u32,           // maximum number of Hi-Z steps
    max_dist    : f32,           // maximum ray distance in view-space units
    thickness   : f32,           // depth thickness threshold
) -> vec2<f32>  // Returns reflected screen UV, or vec2<f32>(-1.0) if no hit
```

This function is defined in `crates/subsystems/renderer/src/ssr/hiz.wgsl` and
shared across all SSR-related shaders. It is not redefined in the compute pass.

---

## 5. SSS Pass (Bit 1)

### 5.1 Purpose

Separable subsurface scattering applied per-pixel for materials that have the
`sss` eval point enabled. The SSS pass runs a separable blur (horizontal then
vertical) modulated by subsurface color and radius.

### 5.2 Binding Layout

```wgsl
@group(0) @binding(0) var template_id_tex : texture_2d<u32>;
@group(0) @binding(1) var gbuffer_albedo    : texture_2d<f32>;
@group(0) @binding(2) var depth_tex         : texture_2d<f32>;
@group(0) @binding(3) var irradiance_tex    : texture_2d<f32>;    // input lit scene
@group(0) @binding(4) var sss_output        : texture_storage_2d<rgba16f, write>;

@group(1) @binding(0) var template_dispatches : array<GpuTemplateDispatch>;
@group(1) @binding(1) var sss_params : SssUniforms;

struct SssUniforms {
    kernel_radius : u32,             // blur kernel half-size (e.g. 8)
    depth_scale   : f32,             // depth-aware blur strength
    falloff       : vec3<f32>,       // subsurface scattering falloff (RGB channels)
};
```

### 5.3 Eval Function

```wgsl
const SSS_BIT : u32 = 1u;

struct SssParams {
    subsurface_color : vec3<f32>,
    subsurface_radius : f32,        // blur radius multiplier
    thickness : f32,                // screen-space thickness approximation
    depth : f32,                    // linear depth for depth-aware blur
};

fn eval_sss(material_class : u32, uv : vec2<f32>) -> SssParams {
    let albedo_texel = textureLoad(gbuffer_albedo, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_albedo))), 0);
    let depth_val    = textureLoad(depth_tex, ivec2(uv * vec2<f32>(textureDimensions(depth_tex))), 0).r;

    var params : SssParams;
    // Material class 0 is reserved (no material). Real SSS materials pack these
    // in the albedo alpha or a dedicated material buffer.
    params.subsurface_color = albedo_texel.rgb;       // for prototyping; real impl uses material buffer
    params.subsurface_radius = albedo_texel.a * 5.0;  // packed in albedo alpha (0–1 → 0–5 pixel radius)
    params.thickness = 1.0;                             // placeholder; real impl uses precomputed thickness map
    params.depth = linearize_depth(depth_val, sss_params.projection_matrix);
    return params;
}
```

### 5.4 Main Shader Body (Horizontal Pass shown)

**Note:** SSS is implemented as two separate compute dispatches — one
horizontal, one vertical — each reading `template_id_tex` to determine
which pixels participate.

```wgsl
@compute @workgroup_size(8, 8, 1)
fn cs_main_horizontal(@builtin(global_invocation_id) id : vec3<u32>) {
    let dimensions = textureDimensions(template_id_tex);
    if (any(id.xy >= dimensions.xy)) { return; }

    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << SSS_BIT)) == 0u { return; }

    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(dimensions.xy);
    let sss = eval_sss(material_class, uv);

    // === Separable horizontal SSS blur ===
    var total_weight : f32 = 0.0;
    var sum : vec3<f32> = vec3<f32>(0.0);
    let radius = u32(sss.subsurface_radius * f32(sss_params.kernel_radius));

    for (var i = i32(-radius); i <= i32(radius); i++) {
        let sample_uv = uv + vec2<f32>(f32(i) / f32(dimensions.x), 0.0);
        let sample_depth = linearize_depth(
            textureLoad(depth_tex, ivec2(sample_uv * vec2<f32>(dimensions.xy)), 0).r,
            sss_params.projection_matrix
        );

        // Depth-aware weight: penalize samples at different depths
        let depth_diff = abs(sample_depth - sss.depth);
        let depth_weight = exp(-depth_diff * sss_params.depth_scale);

        // Gaussian weight
        let gauss_weight = exp(-f32(i * i) / (2.0 * sss.subsurface_radius * sss.subsurface_radius));

        let color = textureLoad(irradiance_tex, ivec2(sample_uv * vec2<f32>(dimensions.xy)), 0).rgb;
        sum += color * gauss_weight * depth_weight;
        total_weight += gauss_weight * depth_weight;
    }

    let result = sum / max(total_weight, 0.0001);
    textureStore(sss_output, id.xy, vec4<f32>(result, 1.0));
}
```

The vertical pass is identical except sampling in the y-direction.

### 5.5 What Happens When SSS is Not Targeted

Pixels without the SSS bit return early. The `sss_output` texture retains its
cleared value (typically `vec4<f32>(0.0)`). The final composite skips SSS for
those pixels.

---

## 6. Decal Pass (Bit 2)

### 6.1 Purpose

Per-pixel decal blending applied to materials that have the `decal` eval point
enabled. Decals are pre-sorted into a per-tile list during scene rebuild; this
compute pass performs a lookup into that list for each pixel and blends the
decal properties into a temporary output.

### 6.2 Binding Layout

```wgsl
@group(0) @binding(0) var template_id_tex : texture_2d<u32>;
@group(0) @binding(1) var gbuffer_albedo    : texture_2d<f32>;
@group(0) @binding(2) var gbuffer_normal    : texture_2d<f32>;
@group(0) @binding(3) var gbuffer_pbr       : texture_2d<f32>;
@group(0) @binding(4) var depth_tex         : texture_2d<f32>;
@group(0) @binding(5) var decal_output_albedo  : texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(6) var decal_output_normal  : texture_storage_2d<rgba8snorm, write>;
@group(0) @binding(7) var decal_output_pbr     : texture_storage_2d<rgba8unorm, write>;

@group(1) @binding(0) var template_dispatches : array<GpuTemplateDispatch>;
@group(1) @binding(1) var decal_tile_list  : storage_buffer;  // tile-indexed decal list
@group(1) @binding(2) var decal_data_buf   : storage_buffer;  // decal instance data

struct DecalInstance {
    transform : mat4x4<f32>,         // decal projection matrix
    albedo : vec4<f32>,               // RGBA (alpha is blend factor)
    normal : vec3<f32>,               // decal normal in world space
    roughness : f32,                  // roughness override
    metalness : f32,                  // metalness override
    blend_mode : u32,                 // 0=replace, 1=multiply, 2=lerp
};
```

### 6.3 Eval Function

```wgsl
const DECAL_BIT : u32 = 2u;

struct DecalOutput {
    albedo : vec4<f32>,           // blended albedo
    normal : vec3<f32>,           // blended normal
    roughness : f32,              // blended roughness
    metalness : f32,              // blended metalness
    has_decal : bool,             // whether any decal was applied
};

fn eval_decal(material_class : u32, uv : vec2<f32>, linear_depth : f32) -> DecalOutput {
    var result : DecalOutput;
    result.has_decal = false;

    // === Fetch gbuffer values first (fallback if no decal) ===
    result.albedo    = textureLoad(gbuffer_albedo, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_albedo))), 0);
    result.normal    = decode_normal(
        textureLoad(gbuffer_normal, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_normal))), 0)
    );
    let pbr_texel    = textureLoad(gbuffer_pbr, ivec2(uv * vec2<f32>(textureDimensions(gbuffer_pbr))), 0);
    result.roughness = pbr_texel.r;
    result.metalness = pbr_texel.g;

    // === Tile-based decal lookup ===
    let tile_x = u32(uv.x * f32(TILE_GRID_X));
    let tile_y = u32(uv.y * f32(TILE_GRID_Y));
    let tile_index = tile_y * TILE_GRID_X + tile_x;

    let decal_count = decal_tile_list[tile_index].count;
    let decal_start = decal_tile_list[tile_index].start;

    for (var i = 0u; i < decal_count; i++) {
        let decal_idx = decal_tile_list.decal_indices[decal_start + i];
        let decal = decal_data_buf[decal_idx];

        // === Decal projection ===
        let world_pos = reconstruct_world_position(uv, linear_depth, inverse_view_proj);
        let decal_uv = (decal.transform * vec4<f32>(world_pos, 1.0)).xy;

        // Discard if outside decal volume
        if (any(abs(decal_uv) > vec2<f32>(1.0))) { continue; }

        // === Sample decal textures (placeholder — sample decal atlas) ===
        let decal_albedo = decal.albedo;
        let blend = decal_albedo.a;

        // === Blend according to decal.blend_mode ===
        switch decal.blend_mode {
            case 0u { // Replace
                result.albedo.rgb = mix(result.albedo.rgb, decal_albedo.rgb, blend);
                result.normal     = normalize(mix(result.normal, decal.normal, blend));
                result.roughness  = mix(result.roughness, decal.roughness, blend);
                result.metalness  = mix(result.metalness, decal.metalness, blend);
            }
            case 1u { // Multiply
                result.albedo.rgb *= mix(vec3<f32>(1.0), decal_albedo.rgb, blend);
            }
            case 2u { // Lerp (overlay)
                result.albedo.rgb = mix(result.albedo.rgb, decal_albedo.rgb, blend);
            }
            default { }
        }

        result.has_decal = true;
    }

    return result;
}
```

### 6.4 Main Shader Body

```wgsl
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id : vec3<u32>) {
    let dimensions = textureDimensions(template_id_tex);
    if (any(id.xy >= dimensions.xy)) { return; }

    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << DECAL_BIT)) == 0u {
        // For pixels without decal support, we still need to copy gbuffer
        // to the decal output textures so the rest of the pipeline works.
        copy_gbuffer_passthrough(id.xy);
        return;
    }

    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(dimensions.xy);
    let depth_val = textureLoad(depth_tex, id.xy, 0).r;
    let linear_depth = linearize_depth(depth_val, proj_matrix);
    let decal = eval_decal(material_class, uv, linear_depth);

    // === Write decal-blended values to output textures ===
    textureStore(decal_output_albedo, id.xy, decal.albedo);
    textureStore(decal_output_normal, id.xy, encode_normal(decal.normal));
    textureStore(decal_output_pbr, id.xy, vec4<f32>(decal.roughness, decal.metalness, 0.0, 1.0));
}

fn copy_gbuffer_passthrough(pixel : vec2<u32>) {
    textureStore(decal_output_albedo, pixel, textureLoad(gbuffer_albedo, ivec2(pixel), 0));
    textureStore(decal_output_normal, pixel, textureLoad(gbuffer_normal, ivec2(pixel), 0));
    textureStore(decal_output_pbr, pixel, textureLoad(gbuffer_pbr, ivec2(pixel), 0));
}
```

### 6.5 What Happens When a Pixel Doesn't Have a Decal

Two cases:

1. **Material class lacks the `decal` bit entirely** — the shader calls
   `copy_gbuffer_passthrough` which copies the original gbuffer values to the
   decal output textures unchanged. This ensures that downstream passes can
   always read from the decal output textures regardless of whether decals
   were active.

2. **Material class has the `decal` bit, but the tile lookup finds zero
   decals** — `has_decal` remains `false`, and the gbuffer fallback values
   (loaded at the top of `eval_decal`) are written as-is.

In both cases, the decal output textures contain the unmodified gbuffer values
for non-decal pixels, making them a drop-in replacement for the original
gbuffer in subsequent lighting passes.

---

## 7. Post-Process Pass (Bit 3)

### 7.1 Purpose

Per-pixel post-processing effects that are tied to specific materials (as
opposed to full-screen filters like bloom or tone-mapping which run on every
pixel unconditionally). Examples include:

- Scanline effects on holographic materials.
- Vignette intensity that varies per material.
- Chromatic aberration applied only to certain objects (e.g., force-field
  effects).
- Custom color grading LUTs indexed by material class.

### 7.2 Binding Layout

```wgsl
@group(0) @binding(0) var template_id_tex : texture_2d<u32>;
@group(0) @binding(1) var scene_color        : texture_2d<f32>;     // fully-lit scene color
@group(0) @binding(2) var depth_tex          : texture_2d<f32>;
@group(0) @binding(3) var post_output        : texture_storage_2d<rgba16f, write>;

@group(1) @binding(0) var template_dispatches : array<GpuTemplateDispatch>;
@group(1) @binding(1) var post_params : PostUniforms;

struct PostUniforms {
    time : f32,                       // elapsed time (for animated effects)
    frame_index : u32,                // frame counter
    aspect_ratio : f32,
    // Per-class post-effect parameters can be in a separate buffer
};
```

### 7.3 Eval Function

```wgsl
const POST_BIT : u32 = 3u;

struct PostProcessOutput {
    color : vec4<f32>,                // modified color
    output_mask : u32,                // bitmask of effects applied (for debugging)
};

fn eval_post(material_class : u32, uv : vec2<f32>, color_in : vec4<f32>, linear_depth : f32) -> PostProcessOutput {
    var out : PostProcessOutput;
    out.color = color_in;
    out.output_mask = 0u;

    // === Dispatch to material-class-specific post functions ===
    // In practice, the dispatch could be a jump table loaded from a buffer.
    // For clarity, we show a switch on material class.

    switch material_class {
        case HOLOGRAM_CLASS_ID {
            // Scanline effect
            let scanline = sin(uv.y * f32(textureDimensions(scene_color).y) * 3.14159 * 2.0);
            let intensity = 0.1 + 0.9 * abs(scanline);
            out.color.rgb *= intensity;
            out.output_mask |= 1u;

            // Pulsing emission
            let pulse = 0.5 + 0.5 * sin(post_params.time * 4.0 + uv.x * 10.0);
            out.color.rgb += vec3<f32>(0.0, 0.2, 0.5) * pulse * 0.3;
            out.output_mask |= 2u;
        }
        case FORCE_FIELD_CLASS_ID {
            // Edge glow based on depth derivative
            let ddepth = fwidth(linear_depth);
            let edge = 1.0 - smoothstep(0.0, 0.01, ddepth);
            out.color.rgb += vec3<f32>(0.0, 0.5, 1.0) * edge * 0.8;
            out.output_mask |= 4u;

            // Chromatic aberration at edges
            let ca_offset = edge * 0.005;
            let r = textureSampleLevel(scene_color, scene_sampler, uv + vec2<f32>(ca_offset, 0.0), 0.0).r;
            let b = textureSampleLevel(scene_color, scene_sampler, uv - vec2<f32>(ca_offset, 0.0), 0.0).b;
            out.color.r = mix(out.color.r, r, edge * 0.5);
            out.color.b = mix(out.color.b, b, edge * 0.5);
            out.output_mask |= 8u;
        }
        default {
            // No per-pixel post effect — pass through unchanged
        }
    }

    return out;
}
```

### 7.4 Main Shader Body

```wgsl
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id : vec3<u32>) {
    let dimensions = textureDimensions(template_id_tex);
    if (any(id.xy >= dimensions.xy)) { return; }

    let template_id = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[template_id].eval_point_mask;
    if (mask & (1u << POST_BIT)) == 0u {
        // For pixels without post-processing, copy scene color through
        textureStore(post_output, id.xy, textureLoad(scene_color, id.xy, 0));
        return;
    }

    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(dimensions.xy);
    let scene_col = textureLoad(scene_color, id.xy, 0);
    let depth_val = textureLoad(depth_tex, id.xy, 0).r;
    let linear_depth = linearize_depth(depth_val, proj_matrix);

    let post = eval_post(material_class, uv, scene_col, linear_depth);
    textureStore(post_output, id.xy, post.color);
}
```

### 7.5 What Happens When a Pixel Doesn't Target Post-Process

The shader copies the scene color through to `post_output` unchanged. This
makes `post_output` a valid HDR color target for all pixels.

---

## 8. Workgroup Divergence Mitigation

### 8.1 The Problem

When adjacent pixels in the same workgroup belong to different material
classes with different eval-point masks, wavefront divergence occurs. The
concern is especially acute when:

- A **16×16 workgroup** (256 threads) covers a region where two or more
  material classes interleave (e.g., a chessboard pattern of hologram and
  metal).
- The expensive eval function (e.g., SSR ray march) executes for all threads
  even when only a fraction of the wavefront needs it.

### 8.2 Why Large Workgroups Increase Divergence Cost

A 16×16 workgroup = 256 threads = 4–8 wavefronts (32–64 threads each,
depending on GPU architecture). If only 25% of threads need the SSR eval:

- **4×4 workgroup** (16 threads) → 1 wavefront → 4 threads need SSR →
  ~25% divergence per wavefront (acceptable).
- **16×16 workgroup** (256 threads) → 8 wavefronts of 32 → each wavefront
  may have 2–8 threads needing SSR → up to 75% of lanes masked → large
  wasted execution.

With 16×16, the GPU spends 75% of its SSR compute time on masked-out threads.

### 8.3 Why Small Workgroups Increase Dispatch Overhead

A 4×4 workgroup = 16 threads:

- At 1920×1080 = ~16,200 workgroups (vs ~32,400 for 8×8, vs ~8,100 for 16×16).
- The GPU's command processor must dispatch and retire all workgroups.
- Workgroup launch overhead is not free: each dispatch carries state setup
  cost. Too many small workgroups can bottleneck the front-end.

### 8.4 Recommended Workgroup Size: 8×8

**8×8 = 64 threads (1–2 wavefronts)**.

- 64 threads is the typical wavefront size on AMD RDNA; on NVIDIA it spans
  two 32-thread warps.
- Divergence within a single wavefront is bounded.
- Dispatch overhead is reasonable: ~32,400 workgroups at 1080p.
- Empirically found to be the best balance across GCN, RDNA, Turing, and
  Ada Lovelace architectures during Helio Radiant v1 profiling.

**All compute eval-point passes use 8×8.**

### 8.5 Future Optimization: Re-Dispatch by Material Class

The most divergence-resistant solution is to eliminate the early-return
pattern entirely and instead dispatch workgroups based on which pixels need
which eval point. This is a **material-histogram approach**:

```
Phase 1 (compute):  Pixel → material_class histogram (atomic add in a
                    workgroup-shared buffer, then global prefix sum).

Phase 2 (dispatch): For each class C with eval points [E0, E1, ...]:
                    Dispatch a compute shader that only processes pixels
                    of class C, reading from a compacted pixel list.

Phase 3 (per-pixel): Each dispatch runs without any early-return check;
                     every thread executes the eval function.
```

**Benefits:**

- Zero wavefront divergence.
- Only the necessary eval functions execute.

**Costs:**

- Additional histogram + compaction passes (GPU time + bandwidth).
- More complex pipeline management.
- Potentially many small dispatches (one per material class).

**Decision (Helio Radiant v2):** Not implemented in v2. The early-return
pattern with 8×8 workgroups is sufficient for the expected scene complexity.
Re-dispatch optimization is tracked as a post-v2 performance target.

---

## 9. Template ID texture Resolution

### 9.1 Full Resolution (1:1 with render target)

| Aspect | Detail |
|---|---|
| Resolution | Same as internal render resolution (e.g., 1920×1080) |
| Memory | 8 MB at 1080p, 32 MB at 4K |
| Accuracy | **Exact per-pixel material class — no aliasing** |
| Sampling | Always `textureLoad` with integer coordinates (point sampling) |

**Recommendation for Helio Radiant v2: Full resolution.**

The Template ID texture is used for **selection**, not for blending. Every
pixel's class ID must be exact — there is no valid interpolation between class
5 and class 7 at a boundary. Using lower resolution would produce:

- Jagged material boundaries (if nearest-neighbor downsampled).
- Incorrect class IDs at edges (if any filtering is applied).

Since the texture is `R32Uint`, bilinear filtering is semantically meaningless
(you cannot interpolate integer template IDs). The only correct sampling
mode is `textureLoad` with integer pixel coordinates, which directly indexes
the texel.

### 9.2 Half Resolution

| Aspect | Detail |
|---|---|
| Resolution | width/2 × height/2 |
| Memory | 2 MB at 1080p, 8 MB at 4K |
| Accuracy | **4 pixels share one material class** — causes artifacts at boundaries |
| Sampling | Must use nearest-neighbor (not bilinear!) if sampled with `textureSampleLevel` |

In half-resolution mode, a 2×2 block of render-target pixels all read the
same material class from the downsampled texture. This means:

- A 2×2 block that straddles a material boundary will get **one class ID for
  the whole block**.
- Unless the class ID is the same for both materials, 2–3 pixels per block
  will execute the wrong eval function (and early-return, wasting compute).

**Recommendation against half-resolution** for any quality-sensitive pipeline.

### 9.3 Why Bilinear Filtering is Wrong

`textureSampleLevel` with bilinear filtering on a `R32Uint` texture is **not
supported** in WGSL (the `textureSampleLevel` function for integer textures
does not exist). The only way to read an `R32Uint` texture with filtering
would be to reinterpret it as `R32Float` and round, which would:

- Produce nonsensical interpolated floats for adjacent integer class IDs.
- Rounding would snap to one class ID, but the boundary position would be
  shifted by the filter kernel.

**Always use `textureLoad` with integer coordinates.** There is no scenario
where filtering an integer class ID texture is correct.

---

## 10. Multiple Passes Sharing the Same Texture

### 10.1 Written Once, Read Many

`template_id_tex` is:

- **Written** by the gbuffer pass (as `@location(4) @interpolate(flat)`).
- **Read** by every compute eval-point pass (SSR, SSS, decal, post-process).

### 10.2 No Synchronization Needed

The gbuffer pass is a full-screen render pass that writes `template_id_tex`
as a render target. It completes before any compute eval-point pass runs
because:

1. The gbuffer pass is a **render pass** (using `beginRenderPass` /
   `endRenderPass`).
2. All compute passes are **compute passes** that depend on the gbuffer
   outputs.
3. The graph execution order (see [Engine Lifecycle](02-engine-lifecycle.md))
   guarantees a **complete pipeline barrier** between the gbuffer render pass
   and the first compute eval-point pass.

Specifically, the command recording follows this pattern:

```
// Layout transition: template_id_tex (UNDEFINED → COLOR_ATTACHMENT_OPTIMAL)
render_pass: gbuffer
    // GPU writes template_id_tex via render target
end render_pass

// Implicit barrier: all render-target writes complete

// Layout transition: template_id_tex (COLOR_ATTACHMENT_OPTIMAL → GENERAL)
compute_pass: SSR
    // GPU reads template_id_tex (read-only, no hazard)
end compute_pass

compute_pass: SSS
    // GPU reads template_id_tex
end compute_pass

compute_pass: decal
    // GPU reads template_id_tex
end compute_pass

compute_pass: post_process
    // GPU reads template_id_tex
end compute_pass
```

### 10.3 Read-Only Access

All compute passes bind `template_id_tex` as `texture_2d<u32>` with read
semantics. No compute pass writes to it. This means:

- No aliasing hazards between compute passes.
- The texture can remain in the `GENERAL` layout (or `SHADER_READ_ONLY_OPTIMAL`
  on Vulkan) for the entire compute phase.
- Cache coherence: modern GPUs will keep the texture in L2/L1 read caches
  after the first compute pass reads it, benefiting subsequent passes.

### 10.4 Resource Lifetime

`template_id_tex` is allocated once at startup (or on resolution change)
and reused every frame. Its contents are valid from the moment the gbuffer
pass finishes until the end of the frame. At the start of the next frame, the
gbuffer pass clears and rewrites it.

---

## Appendix A: WGSL Shader Index

| File | Purpose |
|---|---|
| `shaders/compute/ssr.wgsl` | SSR compute pass (eval point bit 0) |
| `shaders/compute/sss.wgsl` | SSS separable blur passes (eval point bit 1) |
| `shaders/compute/decal.wgsl` | Decal blending pass (eval point bit 2) |
| `shaders/compute/post_process.wgsl` | Per-pixel post-process pass (eval point bit 3) |
| `shaders/common/dispatch_types.wgsl` | `GpuTemplateDispatch` struct and eval-point bit constants |
| `shaders/gbuffer/gbuffer.wgsl` | Gbuffer fragment shader (writes `template_id_tex`) |

## Appendix B: Eval Point Bit Constants

Defined in `shaders/common/dispatch_types.wgsl`:

```wgsl
const SSR_BIT         : u32 = 0u;
const SSS_BIT         : u32 = 1u;
const DECAL_BIT       : u32 = 2u;
const POST_PROCESS_BIT : u32 = 3u;

const EVAL_POINT_COUNT : u32 = 4u;

struct GpuTemplateDispatch {
    eval_point_mask : u32,
    _pad : vec3<u32>,
};
// Size = 16 bytes
```

## Appendix C: Maximum Material Classes

The maximum number of material classes is a device-dependent constant set at
engine initialization. The default for Helio Radiant v2 is:

```rust
pub const MAX_MATERIAL_CLASSES: u32 = 256;
```

This limits the `GpuTemplateDispatch` buffer to `256 × 16 = 4,096 bytes` —
trivially small. Class IDs 1–255 are available for materials; class ID 0 is
reserved (null / invalid / discarded pixels).

---


> **Helio Radiant v2** — How the editor queries, configures, previews, compiles,
> exports, and hot-reloads materials within the eval-point dispatch system.
>
> **Design principle:** The editor is a first-class consumer of the eval point
> registry. Every material graph is a visual composition of eval functions.
> Artists choose which passes a material participates in via a multi-select
> checklist, not by writing low-level WGSL plumbing. The editor compiles the
> graph into a single `.pulsar-material` file (JSON + embedded WGSL), and the
> engine loads it at runtime with zero manual configuration.

---

## Table of Contents

1. [Querying Available Eval Points](#1-querying-available-eval-points)
2. [Material Graph → Eval Functions](#2-material-graph--eval-functions)
3. [Preset System](#3-preset-system)
4. [Performance Warnings](#4-performance-warnings)
5. [Material Preview](#5-material-preview)
6. [Shader Template System](#6-shader-template-system)
7. [Export / Compilation](#7-export--compilation)
8. [Hot-Reload Support](#8-hot-reload-support)
9. [Appendix: UI Data Flow](#9-appendix-ui-data-flow)
10. [Appendix: .pulsar-material Schema](#10-appendix-pulsar-material-schema)

---

## 1. Querying Available Eval Points

### 1.1 Editor <-> Renderer Bridge

The editor accesses the frozen `EvalPointRegistry` through the renderer. During
editor startup (after graph construction is complete), the renderer publishes
its registry:

```rust
// Editor-side singleton or service
struct MaterialEditorBridge {
    renderer: Arc<Mutex<Renderer>>,
}

impl MaterialEditorBridge {
    /// Fetch all currently registered eval points from the render graph.
    ///
    /// Called once on editor launch and again whenever the render graph is
    /// rebuilt (resolution change, pipeline config change, plugin hot-reload).
    /// Returns a snapshot suitable for the UI layer.
    pub fn available_eval_points(&self) -> Vec<EvalPointUiInfo> {
        let renderer = self.renderer.lock().unwrap();
        let registry = renderer.eval_registry();

        registry.points().iter().map(|ep| {
            EvalPointUiInfo {
                name: ep.name.to_string(),
                index: ep.index,
                signature: ep.entry_fn_signature.to_string(),
                description: Self::builtin_description(ep.name)
                    .unwrap_or_else(|| "Custom evaluation point from a third-party pass.".into()),
                threading: match ep.threading {
                    ThreadingModel::Fragment => ThreadingUi::Fragment,
                    ThreadingModel::Compute { group_size } => ThreadingUi::Compute {
                        group_x: group_size.0,
                        group_y: group_size.1,
                        group_z: group_size.2,
                    },
                },
                blend_mode: format!("{:?}", ep.blend_mode),
                has_depth: ep.depth_state.is_some(),
                registered_by: Self::registered_by(ep.name),  // heuristic or explicit registry
            }
        }).collect()
    }
}
```

### 1.2 UI Component: Eval Point Checklist

The editor presents eval points as a **multi-select checklist** grouped by
threading model, with an icon per row indicating Fragment (rasterization glyph)
vs Compute (grid/thread glyph).

```
┌─ Target Evaluation Points ──────────────────────────────────────┐
│                                                                  │
│  Fragment:                                                        │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │ ☑ ◇ gbuffer       │ fn eval_gbuffer(...) → SurfaceData      │ │
│  │ ☐ ◇ transparent   │ fn eval_transparent(...) → vec4f        │ │
│  │ ☐ ◇ shadow        │ fn eval_shadow(...) → bool              │ │
│  │ ☐ ◇ depth_prepass │ fn eval_depth(...) → f32                │ │
│  │ ☐ ◇ velocity      │ fn eval_velocity(...) → vec2f           │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  Compute:                                                         │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │ ☐ ⊞ ssr           │ fn eval_ssr(...) → SsrParams            │ │
│  │ ☐ ⊞ sss           │ fn eval_sss(...) → SssParams            │ │
│  │ ☐ ⊞ decal         │ fn eval_decal(...) → DecalOutput        │ │
│  │ ☐ ⊞ post_process  │ fn eval_post(...) → vec4f               │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  Targets selected: 1  (gbuffer)                                  │
└──────────────────────────────────────────────────────────────────┘
```

**Design rules:**
- Fragment points are listed first (they're the common case).
- Compute points are visually separated with a different icon.
- Each row shows: checkbox, icon, eval point name, a truncated entry function
  signature, and a hover tooltip with the full description.
- The "Targets selected: N" counter updates in real-time.
- A search/filter box at the top lets the user filter by name or threading
  model (e.g., type "compute" to show only compute eval points).

### 1.3 Iconography

| Threading Model | Icon Glyph | Meaning |
|---|---|---|
| Fragment | `◇` (diamond) | Rasterization — triangles transformed through vertex shader |
| Compute | `⊞` (grid) | Grid dispatch — workgroups on a 2D thread grid |

Icons are rendered in the editor's icon font at 16×16 pixels. The icon color
matches the editor theme (accent color for selected points, muted gray for
unselected).

### 1.4 Eval Point Metadata

The editor maintains a supplementary metadata table (compiled into the editor
binary) that provides human-readable descriptions for built-in eval points:

```rust
/// Human-readable metadata for the UI layer.
struct EvalPointUiInfo {
    pub name: String,
    pub index: u32,
    pub signature: String,
    pub description: String,
    pub threading: ThreadingUi,
    pub blend_mode: String,
    pub has_depth: bool,
    pub registered_by: String,  // e.g. "GBufferPass", "ThirdParty: motion_blur"
}

impl MaterialEditorBridge {
    fn builtin_description(name: &str) -> Option<String> {
        match name {
            "gbuffer" => Some(
                "Writes surface data (albedo, normal, ORM, emissive) to the G-buffer. \
                 Required for all deferred shading. Every opaque PBR material targets this."
                    .into()
            ),
            "transparent" => Some(
                "Composite the material into the transparent color buffer with alpha blending. \
                 Use for glass, water, fire, holograms, and anything semi-transparent."
                    .into()
            ),
            "shadow" => Some(
                "Controls per-pixel shadow casting. By default all materials cast shadows. \
                 Override for alpha-tested shadows (foliage, chain-link fences) or to opt out."
                    .into()
            ),
            "depth_prepass" => Some(
                "Writes early depth before the G-buffer pass. Override only if you need \
                 procedural displacement or parallax occlusion mapping to write correct depth."
                    .into()
            ),
            "ssr" => Some(
                "Screen-space reflections. Controls reflectivity, ray length, and roughness \
                 scale per pixel. Expensive compute pass — use sparingly."
                    .into()
            ),
            "sss" => Some(
                "Subsurface scattering. Controls subsurface color, blur radius, and strength. \
                 Use for skin, wax, jade, and translucent materials."
                    .into()
            ),
            "decal" => Some(
                "Project decals onto surfaces. Each material that targets 'decal' can receive \
                 decal projection. Typically used by decal actor materials."
                    .into()
            ),
            "post_process" => Some(
                "Per-pixel post-processing. Apply custom color grading, scanline effects, \
                 chromatic aberration, or stylized filters on a per-material basis."
                    .into()
            ),
            "velocity" => Some(
                "Writes per-pixel screen-space velocity for TAA and motion blur. \
                 Override for animated meshes or particle systems with custom motion."
                    .into()
            ),
            _ => None,
        }
    }

    fn registered_by(name: &str) -> String {
        match name {
            "gbuffer" => "GBufferPass".into(),
            "transparent" => "TransparentPass".into(),
            "shadow" => "ShadowPass".into(),
            "depth_prepass" => "DepthPrepass".into(),
            "ssr" => "SsrPass".into(),
            "sss" => "SssBlurPass".into(),
            "decal" => "DecalPass".into(),
            "post_process" => "PostProcessPass".into(),
            "velocity" => "VelocityPass".into(),
            _ => "Third-party / Custom".into(),
        }
    }
}
```

### 1.5 Refresh on Graph Rebuild

If the render graph is rebuilt (e.g., a third-party plugin registers a new pass
with a new eval point), the editor must refresh its checklist:

```rust
impl MaterialEditorBridge {
    /// Called by the editor's render graph observer when graph.rebuild() completes.
    pub fn on_graph_rebuilt(&mut self) {
        let new_points = self.available_eval_points();
        let old_names: HashSet<String> = self.current_points.iter().map(|p| p.name.clone()).collect();
        let new_names: HashSet<String> = new_points.iter().map(|p| p.name.clone()).collect();

        // If the eval point set changed, invalidate any material that references
        // a now-removed point and add the new points (unchecked by default).
        let removed: Vec<&str> = old_names.difference(&new_names).map(|s| s.as_str()).collect();
        let added: Vec<&str> = new_names.difference(&old_names).map(|s| s.as_str()).collect();

        if !removed.is_empty() || !added.is_empty() {
            self.maybe_invalidate_materials(&removed, &added);
        }

        self.current_points = new_points;
        self.notify_ui();
    }
}
```

---

## 2. Material Graph → Eval Functions

### 2.1 Output Node Annotation

The editor's material graph is a node-based shader authoring tool. Each graph
must have at least one **output node**. Output nodes are annotated with the
eval point they feed:

| Output Node Type | Eval Point | Produces |
|---|---|---|
| `GBuffer Output` | `gbuffer` | `fn eval_gbuffer(...) -> SurfaceData` |
| `Transparent Output` | `transparent` | `fn eval_transparent(...) -> vec4f` |
| `Shadow Output` | `shadow` | `fn eval_shadow(...) -> bool` |
| `Depth Output` | `depth_prepass` | `fn eval_depth(...) -> f32` |
| `SSR Output` | `ssr` | `fn eval_ssr(...) -> SsrParams` |
| `SSS Output` | `sss` | `fn eval_sss(...) -> SssParams` |
| `Decal Output` | `decal` | `fn eval_decal(...) -> DecalOutput` |
| `Post-Process Output` | `post_process` | `fn eval_post(...) -> vec4f` |
| `Velocity Output` | `velocity` | `fn eval_velocity(...) -> vec2f` |

### 2.2 Graph Compiler: Collection Phase

When the user clicks **Compile**, the material graph compiler:

1. **Walks the graph** from each output node backward through all connected
   nodes, tracing the full data-flow DAG.
2. **Identifies target eval points** by collecting the output node types.
   A graph with a `GBuffer Output` and a `Transparent Output` targets
   `["gbuffer", "transparent"]`.
3. **Validates connectivity** — every output node must have a complete path
   to all required inputs (e.g., `GBuffer Output` needs albedo, normal,
   roughness, metallic, emissive). Unconnected inputs use default values.
4. **Generates WGSL** — the compiler emits one WGSL `fn` per eval point,
   named according to the output node type.

```rust
/// Result of compiling a material graph.
struct MaterialCompilationResult {
    /// The generated WGSL source containing all eval functions.
    pub wgsl_source: String,
    /// The eval points this material targets (derived from output nodes).
    pub targets: Vec<String>,
    /// List of material parameters extracted from the graph's parameter nodes.
    pub parameters: Vec<MaterialParameter>,
    /// Any warnings or errors encountered during compilation.
    pub diagnostics: Vec<CompilationDiagnostic>,
}

impl MaterialGraphCompiler {
    /// Compile a material graph into WGSL.
    pub fn compile(&self, graph: &MaterialGraph) -> Result<MaterialCompilationResult, CompileError> {
        // 1. Find all output nodes
        let outputs = graph.nodes()
            .filter(|n| n.node_type().is_output())
            .collect::<Vec<_>>();

        if outputs.is_empty() {
            return Err(CompileError::NoOutputNode);
        }

        // 2. Collect eval point targets from output node types
        let targets: Vec<String> = outputs.iter()
            .map(|n| output_node_to_eval_point(n.node_type()))
            .collect();

        // 3. Validate target uniqueness (no duplicates)
        let mut seen = HashSet::new();
        for t in &targets {
            if !seen.insert(t.clone()) {
                return Err(CompileError::DuplicateEvalPoint(t.clone()));
            }
        }

        // 4. Trace each output and generate the body for its eval function
        let mut wgsl_parts: Vec<String> = Vec::new();
        wgsl_parts.push(self.generate_header(graph));

        for output in &outputs {
            let eval_fn_name = output_node_to_fn_name(output.node_type());
            let body = self.trace_graph_to_body(output, graph)?;
            let signature = self.eval_point_signature(output.node_type());

            wgsl_parts.push(format!(
                "{} {{\n{}\n}}\n",
                signature,
                indent(&body, 4)
            ));
        }

        // 5. Generate parameter structs and uniform blocks
        let parameters = self.extract_parameters(graph);
        if !parameters.is_empty() {
            wgsl_parts.push(self.generate_parameter_block(&parameters));
        }

        let wgsl_source = wgsl_parts.join("\n");

        Ok(MaterialCompilationResult {
            wgsl_source,
            targets,
            parameters,
            diagnostics: self.diagnostics.clone(),
        })
    }
}
```

### 2.3 Function Naming Convention

The compiler derives eval function names from output node types using a
fixed mapping:

```rust
fn output_node_to_fn_name(node_type: OutputNodeType) -> &'static str {
    match node_type {
        OutputNodeType::GBuffer => "eval_gbuffer",
        OutputNodeType::Transparent => "eval_transparent",
        OutputNodeType::Shadow => "eval_shadow",
        OutputNodeType::Depth => "eval_depth",
        OutputNodeType::SSR => "eval_ssr",
        OutputNodeType::SSS => "eval_sss",
        OutputNodeType::Decal => "eval_decal",
        OutputNodeType::PostProcess => "eval_post",
        OutputNodeType::Velocity => "eval_velocity",
    }
}

fn output_node_to_eval_point(node_type: OutputNodeType) -> &'static str {
    match node_type {
        OutputNodeType::GBuffer => "gbuffer",
        OutputNodeType::Transparent => "transparent",
        OutputNodeType::Shadow => "shadow",
        OutputNodeType::Depth => "depth_prepass",
        OutputNodeType::SSR => "ssr",
        OutputNodeType::SSS => "sss",
        OutputNodeType::Decal => "decal",
        OutputNodeType::PostProcess => "post_process",
        OutputNodeType::Velocity => "velocity",
    }
}
```

### 2.4 Generated WGSL Example

A glass material graph with GBuffer Output + Transparent Output + SSR Output
produces:

```wgsl
// Auto-generated by Helio Radiant v2 Material Graph Compiler
// Material: "Wine Glass"
// Targets: gbuffer, transparent, ssr

struct MaterialParams_Glass {
    tint: vec4f,
    ior: f32,
    roughness: f32,
    metalness: f32,
}

@group(2) @binding(0) var<uniform> params: MaterialParams_Glass;

// ── G-buffer eval ────────────────────────────────────────────────────
fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData {
    var s: SurfaceData;
    s.albedo = params.tint;
    s.normal = normal;
    s.roughness = params.roughness;
    s.metallic = params.metalness;
    s.specular_f0 = vec3f(pow((1.0 - params.ior) / (1.0 + params.ior), 2.0));
    s.emissive = vec3f(0.0);
    s.subsurface_color = vec3f(0.0);
    s.flags = 0u;
    return s;
}

// ── Transparent eval ──────────────────────────────────────────────────
fn eval_transparent(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> vec4f {
    let V = normalize(camera_pos - world_pos);
    let NdV = max(dot(normal, V), 0.001);
    let fresnel = pow(1.0 - NdV, 4.0);
    let col = mix(params.tint.rgb, vec3f(1.0), fresnel * 0.3);
    let alpha = mix(0.4, 0.8, fresnel);
    return vec4f(col, alpha);
}

// ── SSR eval ──────────────────────────────────────────────────────────
fn eval_ssr(material_id: u32, world_pos: vec3f, normal: vec3f, roughness: f32, f0: vec3f) -> SsrParams {
    var p: SsrParams;
    p.reflectivity = 0.95;
    p.ray_length = 100.0;
    p.roughness_scale = 0.0;
    p.enable_ssr = 1u;
    return p;
}
```

### 2.5 Single WGSL File Output

The compiler always produces a **single `.wgsl` string** containing all eval
functions for this material. This string is:

1. Displayed in the editor's shader source panel (read-only by default, but
   advanced users can directly edit).
2. Passed to `renderer.register_material(name, &wgsl, &targets)` when the
   material is saved or the preview is updated.
3. Embedded in the `.pulsar-material` export file (see §7).

**Why a single file instead of one per eval point?**

- Simpler graph compilation — one pass, one output.
- The engine's `register_material` API already extracts per-eval-point
  functions via `compose_fn_override` (see spec part 02, §2).
- Easier for artists to inspect — they see the full material logic in one
  tab, with clear function boundaries.
- Hot-reload (see §8) replaces the entire WGSL source atomically.

### 2.6 Graph Validation Against the Registry

Before compilation, the editor cross-references the graph's output nodes
against the current eval point registry:

```rust
impl MaterialGraphCompiler {
    /// Validate that all output node types correspond to currently registered
    /// eval points. Warns about orphan outputs (point no longer exists).
    fn validate_targets(
        &self,
        outputs: &[OutputNode],
        registry: &EvalPointRegistry,
    ) -> Vec<CompilationDiagnostic> {
        let mut diags = Vec::new();

        for output in outputs {
            let eval_name = output_node_to_eval_point(output.node_type());
            if registry.get(eval_name).is_none() {
                diags.push(CompilationDiagnostic::warning(format!(
                    "Output node '{}' feeds eval point '{}', but that eval point \
                     is not registered in the current render graph. This output \
                     will be ignored.",
                    output.name(),
                    eval_name,
                )));
            }
        }

        diags
    }
}
```

---

## 3. Preset System

### 3.1 Purpose

Presets provide one-click configuration of common material types. A preset
defines:

1. **Target eval points** — which checkboxes in the checklist are checked.
2. **Default shader template** — starting WGSL code for each eval function.
3. **Parameter defaults** — initial values for material parameters (sliders,
   colors, textures).
4. **Preview scene** — which test mesh and lighting environment to use.

### 3.2 Preset Definitions

```rust
/// A preset for quick material setup.
struct MaterialPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,              // icon name for the dropdown
    pub targets: &'static [&'static str],
    pub template: &'static str,          // default WGSL template
    pub default_params: Vec<(&'static str, ParamValue)>,
    pub preview_scene: PreviewSceneKind,
    pub warnings: &'static [PresetWarning],
}

/// Types of preview scenes.
enum PreviewSceneKind {
    /// A sphere in a neutral lighting environment.
    Sphere,
    /// A glass/crystal model on a reflective pedestal.
    Glass,
    /// A water plane with environment reflection.
    WaterPlane,
    /// A flat panel with the material applied.
    Panel,
    /// The user's current editor scene selection.
    CurrentSelection,
}

/// A warning to display when a preset creates a suboptimal configuration.
enum PresetWarning {
    /// E.g., Opaque + transparent = pointless
    ConfigurationWarning(&'static str),
}
```

The complete preset table:

| Preset | Targets | Description | Template | Preview |
|---|---|---|---|---|
| **Opaque** | `["gbuffer"]` | Standard PBR opaque material. Default for most solids. | `opaque_default.wgsl` | Sphere (neutral lighting) |
| **Transparent** | `["transparent"]` | Semi-transparent with alpha blending. No gbuffer writes. | `transparent_default.wgsl` | Panel (shows through) |
| **Glass** | `["gbuffer", "transparent"]` | Writes gbuffer normals/roughness for SSR + transparent color. | `glass_default.wgsl` | Glass (reflective pedestal) |
| **Water** | `["transparent"]` | Transparent-only with animated normals. Optionally SSR/SSS. | `water_default.wgsl` | WaterPlane |
| **SSR Custom** | `["ssr"]` | Custom screen-space reflections only. No rasterization. | `ssr_custom_default.wgsl` | Sphere (SSR debug view) |
| **Full Custom** | *(user selects)* | No presets. User manually checks eval points. | *(none — empty)* | CurrentSelection |

### 3.3 Preset Application Flow

When the user selects a preset, the editor:

```
User clicks "Glass" preset
  │
  ├── 1. Uncheck all eval points
  ├── 2. Check "gbuffer", "transparent"
  ├── 3. Replace WGSL source with glass_default template
  ├── 4. Reset parameters to glass defaults
  ├── 5. Switch preview scene to Glass model
  ├── 6. Trigger immediate recompile
  └── 7. Show preset-specific hints (e.g., "Consider adding SSR for extra realism")
```

```rust
impl MaterialEditor {
    pub fn apply_preset(&mut self, preset: &MaterialPreset) {
        // Set eval point targets
        self.eval_point_selection.clear();
        for target in preset.targets {
            self.eval_point_selection.insert(target.to_string());
        }

        // Replace WGSL source with preset template
        self.wgsl_editor.set_source(preset.template.to_string());

        // Reset parameter values
        self.parameters.clear();
        for (name, value) in &preset.default_params {
            self.parameters.insert(name.to_string(), value.clone());
        }

        // Switch preview scene
        self.preview_scene = preset.preview_scene;
        if let Some(handler) = &self.preview_handler {
            handler.set_scene(preset.preview_scene);
        }

        // Recompile
        self.compile_material();

        // Show hints
        self.show_preset_hints(preset);
    }
}
```

### 3.4 Preset Customization

After applying a preset, the user is free to:

- **Add or remove eval points** — the checklist remains editable. Switching
  from `Glass` to `Opaque + SSR` is as simple as unchecking "transparent"
  and checking "ssr". The WGSL editor keeps any relevant functions and
  grays out (or removes) functions for unchecked points.
- **Edit the WGSL** — the template is a starting point. The user can modify
  any function body, add new functions, or delete functions for unchecked
  eval points (the compiler will emit a warning but not an error).
- **Adjust parameters** — default slider ranges, colors, and texture slots
  are populated from the preset but fully editable.

### 3.5 Preset Change Detection

If the user manually modifies the eval point selection such that it matches
a preset, the editor hints "This matches the [Preset Name] preset. Apply
preset template?":

```rust
fn detect_preset_match(&self) -> Option<&MaterialPreset> {
    let selected: HashSet<&str> = self.eval_point_selection.iter().map(|s| s.as_str()).collect();
    for preset in ALL_PRESETS {
        let preset_set: HashSet<&str> = preset.targets.iter().copied().collect();
        if selected == preset_set {
            return Some(preset);
        }
    }
    None
}
```

### 3.6 Extensible Presets (Third-Party)

Third-party plugins can register custom presets with the editor via a
`MaterialPresetProvider` trait:

```rust
/// Registered by editor plugins to add custom presets.
trait MaterialPresetProvider {
    fn presets(&self) -> Vec<MaterialPreset>;
}

// Example: a water system plugin adds an "Ocean" preset:
impl MaterialPresetProvider for OceanPlugin {
    fn presets(&self) -> Vec<MaterialPreset> {
        vec![
            MaterialPreset {
                name: "Ocean (Deep)",
                targets: &["transparent", "ssr", "sss"],
                template: include_str!("ocean_deep_default.wgsl"),
                // ...
            },
            MaterialPreset {
                name: "Ocean (Shore)",
                targets: &["transparent", "ssr"],
                // ...
            },
        ]
    }
}
```

---

## 4. Performance Warnings

### 4.1 Warning Engine

The editor runs a static analysis pass on every material compilation. Warnings
are displayed in a dedicated **Performance Warnings** panel below the material
graph.

```rust
#[derive(Debug, Clone)]
struct PerformanceWarning {
    pub severity: WarningSeverity,
    pub title: String,
    pub message: String,
    pub help_link: Option<String>,  // URL to documentation
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WarningSeverity {
    Info,
    Warning,
    Error,
}
```

### 4.2 Warning Rules

#### Rule 1: Complexity Indicator (many eval points)

**Trigger**: Material targets 4+ eval points simultaneously.

**Rationale**: Each additional eval point means more shader variants, more
GPU work per pixel, and more complex WGSL to maintain.

```rust
fn warn_complexity(targets: &[String]) -> Option<PerformanceWarning> {
    let count = targets.len();
    if count >= 6 {
        Some(PerformanceWarning {
            severity: WarningSeverity::Warning,
            title: format!("High complexity: {} eval points", count),
            message: format!(
                "This material targets {} evaluation points. Each additional point \
                 adds compilation time, GPU dispatch overhead, and shader complexity. \
                 Consider whether all points are necessary.",
                count
            ),
            help_link: Some("/docs/radiant-v2/performance#complexity"),
        })
    } else if count >= 4 {
        Some(PerformanceWarning {
            severity: WarningSeverity::Info,
            title: format!("Moderate complexity: {} eval points", count),
            message: format!(
                "This material targets {} evaluation points. This is reasonable, \
                 but be aware of the dispatch and compilation cost.",
                count
            ),
            help_link: Some("/docs/radiant-v2/performance#complexity"),
        })
    } else {
        None
    }
}
```

The complexity is also displayed visually as a **gauge** in the material
editor toolbar:

```
┌──────────────────────────────────────────────────────────────┐
│  Complexity: ●●●○○○○○○○  (3 eval points)  [Performance... ▼] │
└──────────────────────────────────────────────────────────────┘
```

The gauge fills from green (1–2 points) through yellow (3–4) to red (5+).

#### Rule 2: Sparse Expensive Compute (poor GPU utilization)

**Trigger**: A compute eval point (SSR, SSS, or decal) is targeted, but the
material is expected to cover fewer than ~5% of screen pixels (estimated from
the preview or from the user's current scene selection).

**Rationale**: Compute passes dispatch full-screen workgroups. If only a tiny
fraction of pixels have the eval-point bit set, most workgroups early-return
after reading `template_id_tex`, wasting GPU dispatch and bandwidth. SSR
ray marching is particularly expensive to waste.

```rust
fn warn_sparse_compute(
    targets: &[String],
    estimated_coverage: f32,   // 0.0–1.0, estimated fraction of screen pixels
) -> Vec<PerformanceWarning> {
    let mut warnings = Vec::new();

    let compute_points = ["ssr", "sss", "decal", "post_process"];
    for target in targets {
        if compute_points.contains(&target.as_str()) && estimated_coverage < 0.05 {
            let point_display = match target.as_str() {
                "ssr" => "SSR (screen-space reflections)",
                "sss" => "SSS (subsurface scattering)",
                "decal" => "Decal",
                "post_process" => "Post-process",
                other => other,
            };

            warnings.push(PerformanceWarning {
                severity: WarningSeverity::Warning,
                title: format!("Sparse {} — poor GPU utilization", point_display),
                message: format!(
                    "The '{}' eval point dispatches a full-screen compute pass, but this \
                     material covers only ~{:.1}% of the screen. Most workgroups will \
                     early-return after reading template_id_tex. Consider using a \
                     fragment-only approach or merging this material with a more \
                     common eval point.",
                    target,
                    estimated_coverage * 100.0,
                ),
                help_link: Some("/docs/radiant-v2/performance#sparse-compute"),
            });
        }
    }

    warnings
}
```

**Coverage estimation heuristics:**

| Method | How it works | Accuracy |
|---|---|---|
| Preview viewport | Rasterize the current preview mesh, count pixels that pass the gbuffer discard check | High (requires actual rasterization) |
| Scene selection | If the user has a mesh selected in the editor, compute its bounding-box screen coverage from the active camera | Medium (screen size approximation) |
| Fallback | Default to 50% coverage (no warning) | Conservative (misses sparse cases) |

#### Rule 3: Transparent material NOT targeting "transparent"

**Trigger**: A material with transparent name/hints (or a preset like
Transparent/Water) does NOT have "transparent" in its target eval points.

**Rationale**: If the user expects see-through behavior but forgets to include
the transparent eval point, the material will render as opaque. This is almost
always a mistake.

```rust
fn warn_transparent_not_targeting_transparent(
    name: &str,
    targets: &[String],
    from_preset: Option<&str>,
) -> Option<PerformanceWarning> {
    let is_transparent_intent = name.to_lowercase().contains("transparent")
        || name.to_lowercase().contains("glass")
        || name.to_lowercase().contains("water")
        || name.to_lowercase().contains("see through")
        || from_preset == Some("Transparent")
        || from_preset == Some("Glass")
        || from_preset == Some("Water");

    if is_transparent_intent && !targets.iter().any(|t| t == "transparent") {
        return Some(PerformanceWarning {
            severity: WarningSeverity::Warning,
            title: "Transparent material not targeting 'transparent' eval point".into(),
            message:
                "This material has a transparent-like name or preset, but the 'transparent' \
                 eval point is not targeted. The material will render as opaque (no alpha \
                 blending). Did you forget to check 'transparent' in the eval point list?"
                    .into(),
            help_link: Some("/docs/radiant-v2/editor#transparent-warning"),
        });
    }

    None
}
```

#### Rule 4: Opaque material targeting "transparent"

**Trigger**: A material with "opaque" in its name (or from the Opaque preset)
targets the "transparent" eval point.

**Rationale**: If the user selects Opaque + transparent, the material will
be rendered in the transparent pass with alpha blending, but since it's
opaque, the alpha blend is pointless — it wastes GPU time in the transparent
pass without any visual benefit.

```rust
fn warn_opaque_targeting_transparent(
    name: &str,
    targets: &[String],
    from_preset: Option<&str>,
) -> Option<PerformanceWarning> {
    let is_opaque_intent = name.to_lowercase().contains("opaque")
        || from_preset == Some("Opaque");

    if is_opaque_intent && targets.iter().any(|t| t == "transparent") {
        return Some(PerformanceWarning {
            severity: WarningSeverity::Warning,
            title: "Opaque material targeting 'transparent' eval point".into(),
            message:
                "This material has an opaque preset/name but also targets the 'transparent' \
                 eval point. Opaque materials do not benefit from alpha blending. Consider \
                 removing 'transparent' from the target list unless the material actually \
                 needs transparency."
                    .into(),
            help_link: Some("/docs/radiant-v2/editor#opaque-transparent-warning"),
        });
    }

    None
}
```

### 4.3 Warning Display

Warnings appear in a collapsible **Performance Warnings** panel:

```
┌─ Performance Warnings (2) ────────────────────────────────────┐
│                                                                │
│  ⚠ Sparse SSR — poor GPU utilization                          │
│    The 'ssr' eval point dispatches a full-screen compute       │
│    pass, but this material covers only ~2.3% of the screen.   │
│    [Learn more...]                                             │
│                                                                │
│  ⚠ Transparent material not targeting 'transparent'           │
│    This material is named 'Glass_Pane' but the 'transparent'   │
│    eval point is not targeted. It will render as opaque.      │
│    [Learn more...]                                             │
│                                                                │
│  ⋮ Complexity: 3 eval points  (Info)                          │
└────────────────────────────────────────────────────────────────┘
```

Each warning has:
- **Severity icon**: `🔴` (Error), `🟡` (Warning), `ℹ️` (Info)
- **Title** in bold
- **Message** in normal weight
- **[Learn more...]** link that opens the relevant docs page

### 4.4 Suppression

Users can right-click a warning and select **Suppress this warning** or
**Suppress all [rule name]** to silence it for the current project.
Suppressed warnings are stored in the `.pulsar-material` file.

---

## 5. Material Preview

### 5.1 Preview Viewport Architecture

The material preview is a **dedicated mini render graph** that mirrors the
main graph's eval point structure. It is not the main editor viewport — it
is a separate panel specifically for material authoring.

```
┌─ Material Preview ──────────────────────────────────────────┐
│  ┌───────────────────────────────────────────────────────┐  │
│  │                                                       │  │
│  │   [ Preview mesh rendered with the current material ]  │  │
│  │                                                       │  │
│  │   ☐ Show wireframe     ☐ Show SSR only               │  │
│  │   ☐ Show gbuffer       ☐ Show transparent only       │  │
│  └───────────────────────────────────────────────────────┘  │
│  FPS: 60  |  Draws: 1  |  Resolution: 512×512              │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 Mini Render Graph

The preview viewport has its own simplified render graph that includes only
the eval points the current material targets, plus the bare minimum plumbing:

```
Preview Render Graph (for a Glass material targeting gbuffer + transparent):

   [Clear]
      │
      ▼
   [Depth Prepass]  ─── (simple, always runs)
      │
      ▼
   [GBuffer Pass]   ─── eval_point_mask check with MASK_GBUFFER
      │
      ├──→ [Transparent Pass]  ─── eval_point_mask check with MASK_TRANSPARENT
      │
      ▼
   [Composite]      ─── blend gbuffer lighting + transparent overlay
      │
      ▼
   [Present to preview panel]
```

For a material targeting only SSR (compute), the graph is even lighter:

```
   [Clear]
      │
      ▼
   [SsrPass]   ─── compute dispatch with Template ID texture
      │
      ▼
   [Present to preview panel]
```

### 5.3 Preview Scene Database

The editor ships with curated preview scenes:

| Scene | Description | Best For |
|---|---|---|
| `sphere` | Metallic sphere on a gray ground plane, IBL environment | Opaque, SSR |
| `sphere_glass` | Glass sphere with visible refraction background | Glass, Transparent |
| `water_plane` | Animated water plane with skybox reflection | Water |
| `panel` | Flat panel mounted on a wall | Transparent, Decal |
| `complex` | Detailed mesh (character head, vehicle) | Full material evaluation |

Each scene is a small `.pulsar-scene` file that references a mesh, an
environment map (IBL), and default lighting. Scenes are stored in
`editor/assets/preview_scenes/`.

### 5.4 Eval Point Mask and Preview Modes

The preview panel includes a toolbar of toggle buttons that let the user
**filter what the preview shows**. Each toggle corresponds to an eval point:

```
[GBuffer] [Transparent] [SSR] [SSS] [Composite]
```

- When **GBuffer** is active, the preview shows the gbuffer output (surface
  albedo, normals, ORM visualization).
- When **Transparent** is active, the preview shows the transparent output
  (alpha-blended color).
- When **SSR** is active, the preview shows the SSR output (reflection-only).
- When **Composite** is active, the preview shows the final composed result
  (all eval points combined).

Internally, these buttons modify which render targets the mini graph presents:

```rust
impl PreviewViewport {
    pub fn set_visible_eval_points(&mut self, points: &[&str]) {
        self.visible_points.clear();
        for p in points {
            self.visible_points.insert(p.to_string());
        }

        // Reconfigure the render graph to skip irrelevant passes
        // and present only the requested targets.
        self.mini_graph.set_active_passes(|pass, eval_points| {
            // Only execute a pass if its eval point is visible OR if the
            // pass provides data needed by visible passes (depth, etc.)
            pass.is_required_by(&self.visible_points)
        });

        // Force recompile + re-render
        self.request_redraw();
    }

    /// Determine whether a pixel is shown based on the eval point mask.
    /// Used by the preview's fragment shader to color non-targeting pixels.
    fn preview_highlight_shader(&self) -> String {
        // Generate WGSL that highlights pixels NOT targeted by the current
        // material in a desaturated color, so the user sees which parts
        // of the mesh participate in each eval point.
        format!(
            r"
            fn preview_highlight(input_mask: u32, target_mask: u32) -> vec4f {{
                if (input_mask & target_mask) == 0u {{
                    return vec4f(0.3, 0.3, 0.3, 0.5); // gray for non-targeting pixels
                }}
                return vec4f(1.0); // normal color for targeting pixels
            }}
            "
        )
    }
}
```

### 5.5 Real-Time Compilation

As the user edits the material graph (adds nodes, changes connections, adjusts
parameters), the editor triggers an automatic recompile:

```rust
impl MaterialEditor {
    /// Debounce timer — 500ms after the last edit, recompile.
    fn on_graph_modified(&mut self) {
        self.recompile_timer.reset(500);  // ms
    }

    /// Called by the editor's main loop when the timer fires.
    fn on_recompile_timer(&mut self) {
        match self.compile_material() {
            Ok(result) => {
                self.last_compilation = Some(result);
                self.update_preview();
                self.warnings = self.analyze_performance(&result);
            }
            Err(errors) => {
                self.show_compile_errors(errors);
            }
        }
    }
}
```

**Compilation is lazy and incremental:**
- The graph topology change check is O(N) where N is the number of nodes.
- Only dirty output subgraphs are re-traced.
- The WGSL output is diffed against the previous compilation; if identical,
  no re-registration occurs (saves GPU pipeline compilation).

### 5.6 Preview Resolution

The default preview resolution is **512×512**. The user can scale it:

| Quality | Resolution | Use Case |
|---|---|---|
| Low | 256×256 | Fast iteration, low-end GPUs |
| Medium (default) | 512×512 | Good balance |
| High | 1024×1024 | Final material polish |
| Ultra | 2048×2048 | Screenshot-quality preview |

Resolution changes do NOT trigger a full graph rebuild — the mini graph's
render targets are re-created with the new dimensions.

### 5.7 Preview Camera Controls

The preview viewport has orbit camera controls (left-drag to orbit,
scroll to zoom, middle-drag to pan). Camera state is per-scene and persists
between edits.

When the user switches preview scenes, the camera resets to the scene's
default camera position (stored in the scene file).

---

## 6. Shader Template System

### 6.1 Template Per Eval Point

The editor ships with starter WGSL templates for every built-in eval point.
Each template is a complete WGSL file annotated with:

1. **Editable regions** — marked with `// -- EDITABLE: <name> --` comments.
   These are the sections the user modifies. Everything outside editable
   regions is read-only boilerplate (bindings, struct definitions).
2. **Documentation comments** — triple-slash (`///`) comments explaining the
   purpose of each eval function, its parameters, return value, and
   recommended usage.
3. **Default implementations** — each eval function has a working default
   that produces a reasonable visual result.

### 6.2 Template File Format

Templates are stored in `editor/templates/eval_<name>.wgsl`.

```wgsl
/// ====================================================================
/// Template: gbuffer
/// Eval point: "gbuffer" (index 0)
/// Entry: fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData
///
/// This function writes surface data to the G-buffer. The G-buffer stores:
///   - albedo (RGB) + metalness (A)    → @location(0)
///   - normal (view-space, packed)      → @location(1)
///   - roughness (R) + ambient_occlusion (G) → @location(2)
///   - emissive (RGB)                   → @location(3)
///
/// Available bindings (do not modify):
///   @group(0) @binding(0) var<uniform> camera: Camera;
///   @group(0) @binding(1) var<uniform> globals: Globals;
///   @group(0) @binding(2) var<storage> materials: array<GpuMaterial>;
///   @group(0) @binding(3) var material_textures: binding_array<texture_2d<f32>>;
///
/// The 'material_id' parameter indexes into the materials array and
/// the material_textures array.
///
/// 'world_pos' is the fragment's world-space position (vec3f).
/// 'normal' is the interpolated vertex normal (world-space, normalized).
/// 'uv' is the primary UV coordinate (vec2f, 0..1).
///
/// Returns SurfaceData — see the struct definition below.
/// ====================================================================

// -- EDITABLE: SurfaceData struct (customize fields if needed) --
struct SurfaceData {
    albedo: vec4f,
    normal: vec3f,
    roughness: f32,
    metallic: f32,
    specular_f0: vec3f,
    emissive: vec3f,
    subsurface_color: vec3f,
    flags: u32,
}

// -- EDITABLE: eval_gbuffer implementation --
fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData {
    // TODO: Replace with your material logic.
    //
    // Example — simple PBR from material parameters:
    let mat = materials[material_id];
    let tex = material_textures[material_id];
    let albedo_sample = textureSampleLevel(tex, mat_sampler, uv, 0.0);

    var s: SurfaceData;
    s.albedo = albedo_sample;
    s.normal = normal;
    s.roughness = mat.roughness;
    s.metallic = albedo_sample.a;  // metalness packed in albedo alpha
    s.specular_f0 = mix(vec3f(0.04), albedo_sample.rgb, albedo_sample.a);
    s.emissive = vec3f(0.0);
    s.subsurface_color = vec3f(0.0);
    s.flags = 0u;
    return s;
}
```

### 6.3 Template for Each Built-in Eval Point

| Eval Point | Template File | Key Docs in Template |
|---|---|---|
| `gbuffer` | `eval_gbuffer.wgsl` | Documents SurfaceData fields, texture sampling, material buffer indexing |
| `transparent` | `eval_transparent.wgsl` | Documents alpha blending, premultiplied alpha convention, fresnel helpers |
| `shadow` | `eval_shadow.wgsl` | Documents return value meaning (true=cast shadow), alpha test pattern |
| `depth_prepass` | `eval_depth.wgsl` | Documents modified depth return, parallax occlusion mapping example |
| `ssr` | `eval_ssr.wgsl` | Documents SsrParams fields, Hi-Z ray march integration, roughness/reflectivity |
| `sss` | `eval_sss.wgsl` | Documents SssParams fields, subsurface color meaning, blur radius |
| `decal` | `eval_decal.wgsl` | Documents DecalOutput fields, gbuffer fallback pattern |
| `post_process` | `eval_post.wgsl` | Documents PostProcessOutput, input color meaning, effect bitmask |
| `velocity` | `eval_velocity.wgsl` | Documents previous-frame world position, screen-space UV velocity |

### 6.4 Editable Region Parsing

The editor's WGSL source view is a **structured editor**:

```
┌─────────────────────────────────────────────────────────────────┐
│ /// Template: gbuffer                                           │
│ /// Entry: fn eval_gbuffer(...)                                  │
│ ... (read-only header)                                          │
│                                                                  │
│ ┌─── EDITABLE: eval_gbuffer implementation ───────────────────┐  │
│ │ fn eval_gbuffer(material_id: u32, ...) -> SurfaceData {     │  │
│ │     var s: SurfaceData;                                     │  │
│ │     s.albedo = vec4f(0.8, 0.2, 0.1, 1.0);                  │  │
│ │     s.roughness = 0.5;                                      │  │
│ │     return s;                                               │  │
│ │ }                                                           │  │
│ └──────────────────────────────────────────────────────────────┘  │
│                                                                  │
│ ... (read-only footer)                                           │
└─────────────────────────────────────────────────────────────────┘
```

- **Read-only regions** have a gray background and are not user-editable.
  They contain binding declarations, struct definitions (unless marked
  editable), vertex shaders, and built-in helper functions.
- **Editable regions** have a white (or theme-matching) background with
  a colored left border (accent color). Only these regions accept keyboard
  input.
- The editor parses `// -- EDITABLE: <name> --` and
  `// -- END EDITABLE: <name> --` markers to delimit regions. If a template
  has no markers, the entire file is editable.

```rust
struct EditableRegion {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

/// Parse a template string into read-only segments and editable regions.
fn parse_template(source: &str) -> (Vec<EditableRegion>, Vec<String>) {
    let mut editable = Vec::new();
    let mut read_only_parts = Vec::new();
    let mut in_editable = false;
    let mut current_region: Option<EditableRegion> = None;

    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("// -- EDITABLE:") {
            let name = trimmed
                .strip_prefix("// -- EDITABLE:")
                .unwrap()
                .trim()
                .trim_end_matches("--")
                .trim();
            in_editable = true;
            current_region = Some(EditableRegion {
                name: name.to_string(),
                start_line: i,
                end_line: i,
                content: String::new(),
            });
        } else if trimmed.starts_with("// -- END EDITABLE") {
            if let Some(mut region) = current_region.take() {
                region.end_line = i;
                editable.push(region);
            }
            in_editable = false;
        } else if in_editable {
            if let Some(ref mut region) = current_region {
                region.content.push_str(line);
                region.content.push('\n');
            }
        } else {
            read_only_parts.push(line.to_string());
        }
    }

    (editable, read_only_parts)
}
```

### 6.5 Template Versioning

Each template has a version string in its header metadata:

```wgsl
/// Template Version: 2.1.0
/// Compatible Engine: >= 0.8.0
```

When the engine is updated and eval point signatures change, the editor
checks template compatibility and offers to migrate:

```rust
impl TemplateManager {
    pub fn check_template_compatibility(&self, template: &str, engine_version: &Version) -> Result<(), TemplateMigration> {
        // Parse version from template header
        let template_version = self.parse_version(template)?;

        // Check against the current engine's required template version
        let required = self.required_template_version();
        if template_version < required {
            return Err(TemplateMigration {
                from: template_version,
                to: required,
                migration_script: Some(self.get_migration_script(template_version, required)),
            });
        }

        Ok(())
    }
}
```

---

## 7. Export / Compilation

### 7.1 Material Definition Format

A compiled material is saved as a `.pulsar-material` file. This is a JSON
document with embedded WGSL:

```json
{
    "format_version": "2.0.0",
    "material": {
        "name": "Wine Glass",
        "author": "artist@studio.com",
        "description": "Tinted glass with SSR reflections",

        "target_eval_points": ["gbuffer", "transparent", "ssr"],

        "wgsl_source": "fn eval_gbuffer(...) { ... }\nfn eval_transparent(...) { ... }\nfn eval_ssr(...) { ... }",

        "parameters": {
            "tint": {
                "type": "color_rgba",
                "default": [0.95, 0.96, 0.97, 0.6],
                "ui": {
                    "display_name": "Glass Tint",
                    "group": "Appearance"
                }
            },
            "ior": {
                "type": "float",
                "default": 1.52,
                "range": [1.0, 2.5],
                "ui": {
                    "display_name": "Index of Refraction",
                    "group": "Optics"
                }
            },
            "roughness": {
                "type": "float",
                "default": 0.02,
                "range": [0.0, 1.0],
                "ui": {
                    "display_name": "Roughness",
                    "group": "Appearance"
                }
            },
            "albedo_map": {
                "type": "texture_2d",
                "default": null,
                "ui": {
                    "display_name": "Albedo Map",
                    "group": "Maps"
                }
            }
        },

        "graph": {
            "nodes": [ ... ],
            "edges": [ ... ]
        },

        "preview": {
            "scene": "sphere_glass",
            "camera": { "theta": 0.8, "phi": 0.3, "distance": 3.0 }
        },

        "suppressed_warnings": [
            "sparse_compute_ssr"
        ]
    }
}
```

### 7.2 JSON Schema

The full JSON Schema for `.pulsar-material` files (see Appendix §10).

### 7.3 Export Flow

```
User clicks "Export" or "Save As..."
  │
  ├── 1. Compile material graph (if dirty)
  ├── 2. Collect material name, description, author
  ├── 3. Serialize parameter metadata + current values
  ├── 4. Serialize material graph (nodes + edges) for re-import
  ├── 5. Serialize suppressed warnings list
  ├── 6. Assemble JSON document
  ├── 7. Write to .pulsar-material file
  └── 8. Show file in OS file manager (optional)
```

```rust
impl MaterialExporter {
    pub fn export_to_file(
        &self,
        material: &Material,
        path: &Path,
    ) -> Result<(), ExportError> {
        let document = MaterialDocument {
            format_version: "2.0.0".to_string(),
            material: MaterialData {
                name: material.name.clone(),
                author: material.author.clone().unwrap_or_default(),
                description: material.description.clone().unwrap_or_default(),
                target_eval_points: material.targets.clone(),
                wgsl_source: material.wgsl_source.clone(),
                parameters: self.serialize_parameters(&material.parameters),
                graph: self.serialize_graph(&material.graph),
                preview: self.serialize_preview_state(&material.preview_state),
                suppressed_warnings: material.suppressed_warnings.clone(),
            },
        };

        let json = serde_json::to_string_pretty(&document)
            .map_err(|e| ExportError::Serialization(e.to_string()))?;

        std::fs::write(path, json)
            .map_err(|e| ExportError::Io(e.to_string()))?;

        Ok(())
    }
}
```

### 7.4 Import / Load Flow

```
Engine loads a .pulsar-material file at runtime (or editor re-imports it)
  │
  ├── 1. Parse JSON
  ├── 2. Validate format_version compatibility
  ├── 3. Validate target_eval_points against current registry
  ├── 4. Call renderer.register_material(name, wgsl_source, targets)
  │       └── Returns a MaterialClassId
  ├── 5. Store parameter metadata in a MaterialParameterRegistry
  ├── 6. (Editor only) Reconstruct the material graph from serialized nodes
  └── 7. (Editor only) Restore preview camera and suppressed warnings
```

```rust
impl MaterialImporter {
    pub fn load_from_file(
        &self,
        path: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedMaterial, ImportError> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| ImportError::Io(e.to_string()))?;

        let document: MaterialDocument = serde_json::from_str(&json)
            .map_err(|e| ImportError::Parse(e.to_string()))?;

        // Version check
        if !self.is_version_compatible(&document.format_version) {
            return Err(ImportError::UnsupportedVersion(document.format_version));
        }

        // Validate eval points
        let registry = renderer.eval_registry();
        for target in &document.material.target_eval_points {
            if registry.get(target).is_none() {
                return Err(ImportError::UnknownEvalPoint(target.clone()));
            }
        }

        // Register with engine
        let class_id = renderer.register_material(
            &document.material.name,
            &document.material.wgsl_source,
            &document.material.target_eval_points,
        );

        // Store parameters (for editor re-import)
        let params = self.deserialize_parameters(&document.material.parameters);

        Ok(LoadedMaterial {
            class_id,
            name: document.material.name,
            parameters: params,
            graph: document.material.graph.map(|g| self.deserialize_graph(g)),
            preview_state: document.material.preview.map(|p| self.deserialize_preview(p)),
            suppressed_warnings: document.material.suppressed_warnings,
        })
    }
}
```

### 7.5 Version Compatibility

| `format_version` | Engine Version | Notes |
|---|---|---|
| `2.0.0` | `>= 0.8.0` | Initial Radiant v2 format |
| `2.1.0` | `>= 0.9.0` | Added `suppressed_warnings` field, `parameter.ui.group` |
| `3.0.0` | `>= 1.0.0` | Future major revision |

The loader performs a semver-compatible check. A `2.1.0` file is loadable
by a `2.0.0` parser (forward-compatible with unknown fields ignored) but a
`3.0.0` file is rejected by a `2.x` parser.

### 7.6 Material Parameters

Parameters are the bridge between the material's WGSL and the editor's UI.
Each parameter declaration in the JSON file maps to a WGSL uniform:

```rust
/// A material parameter exposed to the editor UI and compiled into WGSL.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum MaterialParameter {
    Float {
        default: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<[f32; 2]>,
        ui: ParameterUi,
    },
    Float2 {
        default: [f32; 2],
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<[f32; 2]>,
        ui: ParameterUi,
    },
    Float3 {
        default: [f32; 3],
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<[f32; 2]>,
        ui: ParameterUi,
    },
    ColorRgb {
        default: [f32; 3],
        ui: ParameterUi,
    },
    ColorRgba {
        default: [f32; 4],
        ui: ParameterUi,
    },
    Int {
        default: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<[i32; 2]>,
        ui: ParameterUi,
    },
    Texture2D {
        default: Option<String>,  // asset path
        ui: ParameterUi,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ParameterUi {
    pub display_name: String,
    #[serde(default)]
    pub group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
```

The WGSL compiler generates the uniform block automatically:

```rust
fn generate_parameter_block(params: &[MaterialParameter]) -> String {
    let mut wgsl = String::from("struct MaterialParams {\n");
    for param in params {
        let wgsl_type = match param {
            MaterialParameter::Float { .. } => "f32",
            MaterialParameter::Float2 { .. } => "vec2f",
            MaterialParameter::Float3 { .. } => "vec3f",
            MaterialParameter::ColorRgb { .. } => "vec3f",
            MaterialParameter::ColorRgba { .. } => "vec4f",
            MaterialParameter::Int { .. } => "i32",
            MaterialParameter::Texture2D { .. } => continue, // handled separately
        };
        let name = to_snake_case(&param.ui().display_name);
        wgsl.push_str(&format!("    {}: {},\n", name, wgsl_type));
    }
    wgsl.push_str("}\n\n");
    wgsl.push_str("@group(2) @binding(0) var<uniform> params: MaterialParams;\n");
    wgsl
}
```

---

## 8. Hot-Reload Support

### 8.1 Design Goals

1. **No restart required** — artists iterate on materials without relaunching
   the editor or reloading the entire scene.
2. **Frame-coherent transition** — the material switches atomically between
   the old and new variants. No intermediate state.
3. **Lazy recompilation** — shader variants are not recompiled until they're
   needed (next frame that needs them).
4. **Instance update** — all mesh instances using the material automatically
   pick up the new eval point mask on the next scene rebuild.

### 8.2 Re-Registration with Same Class ID

When a material is recompiled in the editor, it must retain its
`MaterialClassId` so that all existing instances continue to reference it.

```rust
impl Renderer {
    /// Hot-reload an existing material.
    ///
    /// Replaces the material's WGSL and eval point targets in place,
    /// keeping the same MaterialClassId.
    ///
    /// # Parameters
    /// - `class_id`: The MaterialClassId returned by the original
    ///   `register_material()` call.
    /// - `new_wgsl`: The updated WGSL source.
    /// - `new_targets`: The updated list of target eval points.
    ///
    /// # Returns
    /// The same `MaterialClassId` (ownership is unchanged).
    ///
    /// # Panics
    /// - If `class_id` was not previously registered.
    /// - If any eval point in `new_targets` is not in the current registry.
    pub fn hot_reload_material(
        &mut self,
        class_id: MaterialClassId,
        new_wgsl: &str,
        new_targets: &[&str],
    ) -> MaterialClassId {
        let registry = self.eval_registry();

        // 1. Validate new targets
        for target in new_targets {
            assert!(
                registry.get(target).is_some(),
                "hot-reload: unknown eval point '{}'",
                target
            );
        }

        // 2. Compute the new dispatch mask
        let mut new_mask = 0u32;
        for target in new_targets {
            if let Some(idx) = registry.index(target) {
                new_mask |= 1 << idx;
            }
        }

        // 3. Update the material definition in place
        let material = self.materials.get_mut(class_id)
            .expect("hot-reload: unknown template ID");

        let old_mask = material.eval_point_mask;
        let old_wgsl = material.wgsl_source.clone();

        material.wgsl_source = new_wgsl.to_string();
        material.targets = new_targets.iter().map(|s| s.to_string()).collect();
        material.eval_point_mask = new_mask;

        // 4. Invalidate shader variants for changed eval points
        //    (they will be recompiled lazily on next use)
        let old_targets: HashSet<&str> = material.targets.iter().map(|s| s.as_str()).collect();
        let new_target_set: HashSet<&str> = new_targets.iter().copied().collect();

        let added = new_target_set.difference(&old_targets);
        let removed = old_targets.difference(&new_target_set);

        for point in added.chain(removed) {
            if let Some(idx) = registry.index(point) {
                // Mark all variants for this (class_id × eval_point) as dirty
                material.invalidate_variants_for_eval_point(idx);
            }
        }

        // 5. Notify the scene system that instance masks may need updating
        //    (only if the mask changed)
        if old_mask != new_mask {
            self.scene.notify_mask_changed(class_id, new_mask);
        }

        class_id
    }
}
```

### 8.3 Dispatch Mask Update

When the eval point mask changes, all **existing instances** in the scene that
reference this material need updated instance data. The scene rebuild system
handles this efficiently:

```rust
impl SceneManager {
    /// Called by the renderer when a material's dispatch mask changes.
    pub fn notify_mask_changed(&mut self, class_id: MaterialClassId, new_mask: u32) {
        // Only mark this class as dirty — we'll update the mask during
        // the next rebuild_instance_buffers() call.
        self.dirty_material_masks.insert(class_id);

        // Request a scene rebuild on the next frame
        self.request_rebuild();
    }

    /// During rebuild, update the mask on all instances of the dirty class.
    fn rebuild_instance_buffers(&mut self) {
        for instance in &mut self.instances {
            if self.dirty_material_masks.contains(&instance.material_id) {
                let new_mask = self.materials[instance.material_id].eval_point_mask;
                instance.eval_point_mask = new_mask;
            }
        }
        self.dirty_material_masks.clear();

        // Upload updated instance buffer to GPU
        self.upload_instance_buffer();
    }
}
```

**Key property:** The mask update is atomic from the GPU's perspective. The
instance buffer is uploaded as a whole — there is no window where some
instances have the new mask and others the old.

### 8.4 Lazy Shader Recompilation

After hot-reload, shader variants are **not** compiled immediately. Instead,
they are compiled lazily on the first frame that needs them:

```rust
impl Renderer {
    /// Get or create a variant for the given shader key.
    /// Compiles lazily if the variant was invalidated.
    fn get_or_create_variant(
        &mut self,
        key: ShaderVariantKey,
        eval_point: &EvalPoint,
    ) -> &wgpu::RenderPipeline {
        // Check if the variant is dirty (invalidated by hot-reload)
        if self.dirty_variants.contains(&key) {
            self.dirty_variants.remove(&key);
            self.variants.remove(&key);
        }

        if !self.variants.contains_key(&key) {
            let material = self.materials.get(key.template_id).unwrap();
            let composed = compose_for_eval_point(
                eval_point.base_shader,
                &material.wgsl_source,
                eval_point.entry_fn_name,
            );

            // Compile (takes 1–10 ms depending on shader complexity)
            let pipeline = self.compile_pipeline(&key, &composed, eval_point);
            self.variants.insert(key, pipeline);
        }

        &self.variants[&key]
    }
}
```

This means:
- If the material's WGSL changed but the eval point targets didn't, only
  variants for the changed functions are recompiled.
- If only one eval point was added (e.g., SSR), only the SSR variant is
  compiled. The gbuffer and transparent variants are reused from cache.
- Compilation is spread across frames, avoiding a single-frame hitch.

### 8.5 Frame Lifecycle During Hot-Reload

```
Frame N:  User edits material graph
          │
          └──→ hot_reload_material() called
               │
               ├── Update material definition (CPU, immediate)
               ├── Invalidate affected shader variants
               ├── Mark material class as dirty for scene rebuild
               │
               └── Return (editor continues rendering)

Frame N+1:  Scene rebuild runs
            │
            ├── Instance masks updated for dirty classes
            ├── Instance buffer uploaded to GPU
            │
            └── Frame rendered with new masks but old shaders
                (old variants still valid for unchanged eval points)

Frame N+2:  First draw call that needs a recompiled variant
            │
            ├── get_or_create_variant() compiles the new variant
            ├── Pipeline barrier inserted (wgpu handles internally)
            │
            └── Frame rendered with new shaders
```

**Worst-case latency:** 2–3 frames (at 60 FPS: 33–50 ms). In practice,
the human eye perceives this as instantaneous because the visual change
coincides with the frame the new variant is first used.

### 8.6 Hot-Reload from External File

The editor also supports watching `.pulsar-material` files on disk for
changes (useful when material files are edited outside the editor):

```rust
impl MaterialHotReloadWatcher {
    /// Watch a directory for .pulsar-material file changes.
    pub fn watch_directory(&self, path: &Path) {
        let mut watcher = notify::recommended_watcher(move |event| {
            if let notify::Event {
                kind: notify::EventKind::Modify(_),
                paths,
                ..
            } = event
            {
                for path in paths {
                    if path.extension().map_or(false, |e| e == "pulsar-material") {
                        self.on_file_changed(&path);
                    }
                }
            }
        }).unwrap();

        watcher.watch(path, notify::RecursiveMode::NonRecursive).unwrap();
    }

    fn on_file_changed(&self, path: &Path) {
        // Re-import and hot-reload
        match self.importer.load_from_file(path, &mut self.renderer) {
            Ok(loaded) => {
                // If this material is already registered, hot-reload it.
                // Otherwise, register it fresh.
                if let Some(existing) = self.find_by_name(&loaded.name) {
                    self.renderer.hot_reload_material(
                        existing.class_id,
                        &loaded.wgsl_source,
                        &loaded.targets,
                    );
                } else {
                    loaded.class_id = self.renderer.register_material(
                        &loaded.name,
                        &loaded.wgsl_source,
                        &loaded.targets,
                    );
                }

                self.editor.log(format!("Hot-reloaded material: {}", loaded.name));
            }
            Err(e) => {
                self.editor.log_error(format!("Failed to reload material: {}", e));
            }
        }
    }
}
```

### 8.7 Edge Cases

| Scenario | Behavior |
|---|---|
| New eval point added to targets | New variant compiled lazily. Mask updated on next rebuild. Old variants unchanged. |
| Eval point removed from targets | Corresponding variant is unused (but stays in cache). Mask updated. GPU skips this material for that pass. |
| WGSL syntax error | `hot_reload_material` is NOT called. Editor shows compile error. Material continues using last valid version. |
| Registry changed (graph rebuilt) | All materials invalidated and recompiled. Editor shows "migration required" dialog if eval points were renamed. |
| template ID conflicts | Hot-reload uses the existing ID. If a material was deleted and recreated, it gets a new ID (instances referencing the old ID will use a fallback). |

---

## 9. Appendix: UI Data Flow

### 9.1 Data Flow Diagram

```
┌──────────────┐    graph.lock()     ┌────────────────────┐
│ Render Graph  │ ──────────────────► │ EvalPointRegistry  │
│ (all passes)  │                     │ (frozen, on        │
│               │                     │  Renderer)         │
└──────────────┘                      └────────┬───────────┘
                                               │
                                    registry.points()
                                               │
                                               ▼
┌──────────────────┐               ┌────────────────────┐
│ Material Graph   │               │ Eval Point         │
│ Editor Panel     │◄──────────────│ Checklist UI       │
│                  │   selection   │ (multi-select)     │
│  output nodes ───┤   changes     └────────────────────┘
│  parameters   │  │
│  WGSL source  │  │
└───────┬──────────┘
        │
        │ compile()
        ▼
┌──────────────────┐
│ MaterialCompiler  │ ───► WGSL + targets + parameters
└───────┬──────────┘
        │
        ├──► hot_reload_material() (preview / live scene)
        │       │
        │       └──► Scene rebuild → instance mask update
        │
        └──► export_to_file() → .pulsar-material JSON
```

### 9.2 UI Components Summary

| Component | File | Purpose |
|---|---|---|
| `EvalPointChecklist` | `editor/ui/material_editor/eval_checklist.rs` | Multi-select eval point list with icons |
| `MaterialGraphWidget` | `editor/ui/material_editor/graph_widget.rs` | Node-based material graph editor |
| `WgslEditorWidget` | `editor/ui/material_editor/wgsl_editor.rs` | Structured WGSL source editor with editable regions |
| `PresetDropdown` | `editor/ui/material_editor/presets.rs` | Material type preset selector |
| `PerformanceWarningsPanel` | `editor/ui/material_editor/warnings.rs` | Collapsible performance warning display |
| `PreviewViewport` | `editor/ui/material_editor/preview.rs` | Dedicated render viewport for material preview |
| `ParameterPanel` | `editor/ui/material_editor/parameters.rs` | Sliders, color pickers, texture slots |

---

## 10. Appendix: .pulsar-material Schema

### 10.1 Full JSON Schema

```json
{
    "$schema": "http://json-schema.org/draft-07/schema#",
    "$id": "https://pulsar-native.dev/schemas/pulsar-material-v2.json",
    "title": "Pulsar Material v2",
    "description": "A compiled Helio Radiant v2 material definition",
    "type": "object",
    "required": ["format_version", "material"],
    "properties": {
        "format_version": {
            "description": "Semantic version of the .pulsar-material format",
            "type": "string",
            "pattern": "^\\d+\\.\\d+\\.\\d+$",
            "examples": ["2.0.0", "2.1.0"]
        },
        "material": {
            "type": "object",
            "required": ["name", "target_eval_points", "wgsl_source"],
            "properties": {
                "name": {
                    "description": "Human-readable material name",
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256
                },
                "author": {
                    "description": "Author identifier (username or email)",
                    "type": "string",
                    "default": ""
                },
                "description": {
                    "description": "Short description of the material",
                    "type": "string",
                    "default": "",
                    "maxLength": 1024
                },
                "target_eval_points": {
                    "description": "List of eval point names this material targets",
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 32,
                    "items": {
                        "type": "string",
                        "examples": ["gbuffer", "transparent", "ssr"]
                    },
                    "uniqueItems": true
                },
                "wgsl_source": {
                    "description": "Complete WGSL source containing eval functions for all target points",
                    "type": "string"
                },
                "parameters": {
                    "description": "Material parameters exposed to the editor UI",
                    "type": "object",
                    "patternProperties": {
                        "^[a-zA-Z_][a-zA-Z0-9_]*$": {
                            "$ref": "#/definitions/parameter"
                        }
                    },
                    "additionalProperties": false,
                    "default": {}
                },
                "graph": {
                    "description": "Serialized material graph (for re-import in the editor)",
                    "type": "object",
                    "properties": {
                        "nodes": {
                            "type": "array",
                            "items": { "$ref": "#/definitions/graph_node" }
                        },
                        "edges": {
                            "type": "array",
                            "items": { "$ref": "#/definitions/graph_edge" }
                        }
                    },
                    "required": ["nodes", "edges"]
                },
                "preview": {
                    "description": "Preview viewport state",
                    "type": "object",
                    "properties": {
                        "scene": {
                            "type": "string",
                            "enum": ["sphere", "sphere_glass", "water_plane", "panel", "complex"]
                        },
                        "camera": {
                            "type": "object",
                            "properties": {
                                "theta": { "type": "number" },
                                "phi": { "type": "number" },
                                "distance": { "type": "number" }
                            },
                            "required": ["theta", "phi", "distance"]
                        }
                    }
                },
                "suppressed_warnings": {
                    "description": "List of performance warning rule names the user suppressed",
                    "type": "array",
                    "items": {
                        "type": "string",
                        "examples": ["sparse_compute_ssr", "complexity_high"]
                    },
                    "default": []
                }
            }
        }
    },
    "definitions": {
        "parameter": {
            "oneOf": [
                {
                    "type": "object",
                    "required": ["type", "default", "ui"],
                    "properties": {
                        "type": { "enum": ["float"] },
                        "default": { "type": "number" },
                        "range": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "ui": { "$ref": "#/definitions/parameter_ui" }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "default", "ui"],
                    "properties": {
                        "type": { "enum": ["float2", "float3"] },
                        "default": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 3
                        },
                        "range": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "ui": { "$ref": "#/definitions/parameter_ui" }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "default", "ui"],
                    "properties": {
                        "type": { "enum": ["color_rgb", "color_rgba"] },
                        "default": {
                            "type": "array",
                            "items": { "type": "number", "minimum": 0, "maximum": 1 },
                            "minItems": 3,
                            "maxItems": 4
                        },
                        "ui": { "$ref": "#/definitions/parameter_ui" }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "default", "ui"],
                    "properties": {
                        "type": { "enum": ["int"] },
                        "default": { "type": "integer" },
                        "range": {
                            "type": "array",
                            "items": { "type": "integer" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "ui": { "$ref": "#/definitions/parameter_ui" }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "default", "ui"],
                    "properties": {
                        "type": { "enum": ["texture_2d"] },
                        "default": {
                            "type": ["string", "null"],
                            "description": "Asset path to the default texture"
                        },
                        "ui": { "$ref": "#/definitions/parameter_ui" }
                    }
                }
            ]
        },
        "parameter_ui": {
            "type": "object",
            "required": ["display_name"],
            "properties": {
                "display_name": { "type": "string" },
                "group": {
                    "type": "string",
                    "default": "General"
                },
                "description": {
                    "type": "string",
                    "default": ""
                }
            }
        },
        "graph_node": {
            "type": "object",
            "required": ["id", "node_type", "position"],
            "properties": {
                "id": { "type": "string" },
                "node_type": { "type": "string" },
                "position": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "number" },
                        "y": { "type": "number" }
                    },
                    "required": ["x", "y"]
                },
                "parameters": {
                    "type": "object",
                    "default": {}
                }
            }
        },
        "graph_edge": {
            "type": "object",
            "required": ["from_node", "from_socket", "to_node", "to_socket"],
            "properties": {
                "from_node": { "type": "string" },
                "from_socket": { "type": "string" },
                "to_node": { "type": "string" },
                "to_socket": { "type": "string" }
            }
        }
    }
}
```

### 10.2 File Extension

- Extension: `.pulsar-material`
- MIME type: `application/vnd.pulsar.material+json`
- Default location: `assets/materials/` within the project directory

### 10.3 Binary Size Budget

| Component | Typical Size | Notes |
|---|---|---|
| JSON header + metadata | ~500 bytes | Name, targets, version |
| WGSL source | ~2–20 KB | Depends on graph complexity |
| Serialized graph | ~1–10 KB | Node positions, connections |
| Parameter definitions | ~500–2000 bytes | Per-parameter metadata |
| **Total** | **~4–35 KB** | Plain JSON, easily compressible |

Materials are small enough to be loaded synchronously at scene load time.
For shipping builds, the WGSL is already compiled to SPIR-V/MSL by the engine's
shader cache — the `.pulsar-material` JSON only stores the source form for
editor re-import.

---


> **Part of the Helio Radiant v2 Specification Series**  
> Status: **Draft** · Last updated: 2026-07-29

---

## Table of Contents

1. [v1 → v2 API Mapping](#1-v1--v2-api-mapping)
2. [Deprecation Warnings](#2-deprecation-warnings)
3. [Automatic Migration (Shim Layer)](#3-automatic-migration-shim-layer)
4. [What Existing Scene Code Needs](#4-what-existing-scene-code-needs)
5. [What Existing Pass Code Needs](#5-what-existing-pass-code-needs)
6. [Removal Timeline](#6-removal-timeline)
7. [Migration Script](#7-migration-script)
8. [Breaking Changes List](#8-breaking-changes-list)
9. [Testing Strategy](#9-testing-strategy)

---

## 1. v1 → v2 API Mapping

Every public API in Helio Radiant v1 maps to a v2 equivalent as
described in the table below. Cells marked *Removed* have no direct
replacement — the functionality is subsumed by a different mechanism.

| v1 API | v2 API | Notes |
|---|---|---|
| `MaterialTemplateRegistry::register_str(name, wgsl)` | `Renderer::register_material(name, wgsl, &["gbuffer"])` | Auto-targets the `gbuffer` eval point; returns the same `MaterialClassId` |
| `MaterialTemplateRegistry::register_partial_str(name, wgsl)` | `Renderer::register_material(name, wgsl, &["gbuffer"])` | Same as `register_str` — partial templates are now unified |
| `MaterialTemplateRegistry::template_registry_mut()` | **Removed** | `Renderer` owns all material state; direct registry access is no longer exposed |
| `MaterialTemplateRegistry::material_class_ranges()` | **Removed** | Replaced by the `eval_point_mask` field on `GpuInstanceData` |
| `MaterialTemplateRegistry::get_mut()` | **Removed** | Materials are immutable after registration; mutations go through `Renderer` |
| `FLAG_TRANSPARENT_ONLY` | **Removed** | Use target eval points to control pass inclusion — no global flag |
| `TransparentPass` (hardcoded transparent-shader path) | `eval_point("transparent")` | Custom transparent templates now supported via eval points |
| `TransparentPass::new(device, config)` | `TransparentPass::new(device, config, &eval_points)` | Constructor now requires a slice of registered eval points |
| `GBufferPass::new(device, config)` | `GBufferPass::new(device, config, &eval_points)` | Constructor now requires a slice of registered eval points |
| `Renderer::create_material_bgl(...)` | Shared via `eval_point_bgl()` on `EvalPoint` | BGL creation extracted to a shared crate (`helio_radiant_material_bgl`) — no per-pass duplication |
| `MaterialClassId::from_raw(id)` | `from_raw(id)` still exists | Retained but deprecated at code level; prefer `Renderer::material_class_id(name)` |
| `VertexOutput` (no eval_point_mask) | `VertexOutput { ..., eval_point_mask: u32 }` | New field appended at the end; all shaders must be updated |
| `GpuInstanceData` (no eval_point_mask) | `GpuInstanceData { ..., eval_point_mask: u32 }` | New field replaces `material_class_ranges` lookups at runtime |
| `ShadowPass` (no eval point awareness) | `ShadowPass::register_eval_points(...)` | Pass now participates in eval-point dispatch; constructor unchanged |
| `CustomPass` (no eval point awareness) | `CustomPass` with `register_eval_points` default no-op | Backward compatible — existing third-party passes compile without changes |
| `RenderGraph::add_pass(Box<dyn Pass>)` | Unchanged | Pass trait unchanged; only new required method added |
| `Pass::name()` | Unchanged | — |
| `Pass::execute()` | Unchanged | Signature unchanged |
| `Pass::prepare()` | Unchanged | Signature unchanged |
| `Pass::register_eval_points()` | **New (required)** | Returns `Vec<EvalPointConfig>` — see §5 |

---

## 2. Deprecation Warnings

Each deprecated API emits a one-time warning at `warn!` level (Rust
`log` crate) on first invocation. The message includes the v2
replacement and a removal version target.

### 2.1 Warning table

| Deprecated API | Warning message | Log level | Removed in |
|---|---|---|---|
| `register_str` | `[deprecated] MaterialTemplateRegistry::register_str is deprecated. Use Renderer::register_material(name, wgsl, &["gbuffer"]) instead. This shim will be removed in v2.1.` | `warn!` | v2.1 |
| `register_partial_str` | `[deprecated] MaterialTemplateRegistry::register_partial_str is deprecated. Use Renderer::register_material(name, wgsl, &["gbuffer"]) instead. Partial templates are no longer a separate concept. This shim will be removed in v2.1.` | `warn!` | v2.1 |
| `template_registry_mut` | `[deprecated] MaterialTemplateRegistry::template_registry_mut is deprecated. All material registration goes through Renderer directly. This shim will be removed in v2.1.` | `warn!` | v2.1 |
| `material_class_ranges` | `[deprecated] MaterialTemplateRegistry::material_class_ranges is deprecated. Use GpuInstanceData::eval_point_mask instead. This shim will be removed in v2.1.` | `warn!` | v2.1 |
| `get_mut` | `[deprecated] MaterialTemplateRegistry::get_mut is deprecated. Materials are immutable after registration. Use Renderer::material_info(name) for read-only access. This shim will be removed in v2.1.` | `warn!` | v2.1 |
| `FLAG_TRANSPARENT_ONLY` | `[deprecated] FLAG_TRANSPARENT_ONLY is deprecated. Use eval point targets instead. This constant will be removed in v2.1.` | `warn!` (compile-time `#[deprecated]` attr also fires) | v2.1 |
| `MaterialClassId::from_raw` (when used outside of internal paths) | `[deprecated] MaterialClassId::from_raw is deprecated for external use. Use Renderer::material_class_id(name) instead.` | `warn!` | v2.1 |

### 2.2 Warning behaviour

- **One-shot**: each warning fires at most once per process lifetime
  (backed by `std::sync::OnceLock` or `AtomicBool`).
- **No spam**: if the caller invokes a deprecated function in a hot
  loop, only the first call produces a warning.
- **Backtrace hint**: the warning includes the callsite location via
  `std::panic::Location::caller()` when compiled in debug mode.
- **Silencing**: set `HELIO_RADIANT_SILENCE_DEPRECATED=1` at build
  time (via `build.rs` env-var) to suppress all v1→v2 deprecation
  warnings. This is intended for CI migration periods only.

### 2.3 Compile-time deprecation

Constants and re-exports carry `#[deprecated(since = "2.0.0", note = "...")]`
attributes so that `rustc` emits a warning at compile time. For
functions inside trait impls (where `#[deprecated]` cannot be applied),
the runtime warning is the only signal.

---

## 3. Automatic Migration (Shim Layer)

A backward-compatibility shim lives in
`crates/subsystems/helio_radiant/src/deprecated.rs`. It is gated by
the `deprecated-v1-api` feature flag, which is **enabled by default**
in v2.0 and **removed** (along with the file) in v2.1.

### 3.1 `register_str` → `register_material` shim

```rust
// crates/subsystems/helio_radiant/src/deprecated.rs (v2.0 only)

impl MaterialTemplateRegistry {
    pub fn register_str(&self, name: &str, wgsl: &str) -> MaterialClassId {
        warn_deprecated("register_str", "Renderer::register_material(name, wgsl, &[\"gbuffer\"])");
        let renderer = self.renderer_handle.upgrade().expect("Renderer dropped");
        renderer.register_material(name, wgsl, &["gbuffer"])
    }

    pub fn register_partial_str(&self, name: &str, wgsl: &str) -> MaterialClassId {
        warn_deprecated("register_partial_str", "Renderer::register_material(name, wgsl, &[\"gbuffer\"])");
        let renderer = self.renderer_handle.upgrade().expect("Renderer dropped");
        renderer.register_material(name, wgsl, &["gbuffer"])
    }
}
```

### 3.2 Class ID identity

The `MaterialClassId` returned by the shim is **the same** value that
`Renderer::register_material` returns — there is no wrapping or
indirection. Existing code that stores `MaterialClassId` values and
passes them to `GpuInstanceData` continues to work without changes.

### 3.3 Existing PBR materials

The built-in PBR material templates (`"pbr_opaque"`, `"pbr_alpha_cut"`,
`"pbr_transparent"`) are registered internally via
`register_material(name, wgsl, &["gbuffer"])` in v2 init, exactly as
they were in v1. No scene code changes are needed.

### 3.4 `template_registry_mut` shim

```rust
impl MaterialTemplateRegistry {
    pub fn template_registry_mut(&mut self) -> &mut Self {
        warn_deprecated("template_registry_mut", "Renderer::register_material(...) directly");
        self  // identity — no-op, kept only to compile existing callers
    }
}
```

The method still compiles but does nothing useful. Callers that
previously chained `.template_registry_mut().register_str(...)` now
silently invoke the deprecation-warning shim for both the mut accessor
and the register call.

### 3.5 Shim removal in v2.1

The entire `deprecated.rs` file and the `deprecated-v1-api` feature
gate are deleted in the v2.1 release. Any workspace member still
relying on these APIs will fail to compile with a clear error message
pointing to the migration script.

---

## 4. What Existing Scene Code Needs

### 4.1 Inserting a basic opaque material — **no change**

```rust
// v1 (unchanged in v2)
let mat_id = registry.register_str("my_pbr", MY_PBR_WGSL);
instance.material_class_id = mat_id;
```

The shim intercepts `register_str`, calls
`renderer.register_material("my_pbr", MY_PBR_WGSL, &["gbuffer"])`,
and returns the same `MaterialClassId`. The `gbuffer` eval point is
the default target, so the material renders in the GBuffer pass as
before.

### 4.2 Inserting a glass material — old way vs new way

```rust
// v1 (DEPRECATED in v2)
let glass_id = registry.register_str("glass", GLASS_WGSL);
registry.tag_transparent(glass_id);
instance.material_class_id = glass_id;
```

```rust
// v2 (preferred)
let glass_id = renderer.register_material("glass", GLASS_WGSL, &["gbuffer", "transparent"]);
instance.material_class_id = glass_id;
```

Key changes:
- `tag_transparent` is gone — the eval-point list `&["gbuffer", "transparent"]`
  tells the renderer that this material runs in both the gbuffer and
  transparent passes.
- The `eval_point_mask` on `GpuInstanceData` is automatically derived
  from registration; no manual tagging.

### 4.3 Inserting a transparent water material — old way vs new way

```rust
// v1 (DEPRECATED in v2)
let water_id = registry.register_str("water", WATER_WGSL);
registry.tag_transparent(water_id);
// Also had to ensure water WgSL included `#import <TransparentPass::vertex>` etc.
instance.material_class_id = water_id;
```

```rust
// v2 (preferred)
let water_id = renderer.register_material("water", WATER_WGSL, &["gbuffer", "transparent"]);
instance.material_class_id = water_id;
```

The v2 water material can now define its own vertex/fragment entry
points under the `transparent` eval point — the old hardcoded
`TransparentPass` shader chain is gone.

### 4.4 Custom SSR material — new only

```rust
// v1: INVALID — no support for custom eval points
//
// v2:
let ssr_id = renderer.register_material("custom_ssr", SSR_WGSL, &["gbuffer", "ssr"]);
instance.material_class_id = ssr_id;
```

v1 had no concept of user-defined eval points beyond gbuffer and
transparent. In v2, if a pass called `"ssr"` is registered (see §5),
any material can target it by including `"ssr"` in its eval-point list.

---

## 5. What Existing Pass Code Needs

### 5.1 GBufferPass

The GBufferPass is the most commonly extended pass. Migration requires:

#### 5.1.1 Add `register_eval_points` implementation

```rust
// v2 — GBufferPass
impl Pass for GBufferPass {
    fn register_eval_points(&self) -> Vec<EvalPointConfig> {
        vec![EvalPointConfig {
            name: "gbuffer".into(),
            bgl: self.eval_point_bgl(),          // from shared material_bgl crate
            vertex_shader: self.gbuffer_vs(),     // existing
            fragment_shader: self.gbuffer_fs(),   // existing (with discard, see below)
            depth_write: true,
            blend_mode: BlendMode::None,
        }]
    }
}
```

#### 5.1.2 Fragment shader discard pattern

The v2 GBuffer pass fragment shader must include a discard mechanism
for materials whose `eval_point_mask` does not include the gbuffer
bit. Add at the top of the fragment main:

```wgsl
// v2 GBuffer fragment shader — top of main()
let local_eval_mask = material.eval_point_mask;
if (local_eval_mask & EVAL_POINT_GBUFFER_BIT) == 0u {
    discard;
}
```

Where `EVAL_POINT_GBUFFER_BIT` is a shader constant (value `1u`)
injected via a shared `eval_point_bits.wgsl` header. This replaces
the old `material_class_ranges` conditional dispatch.

#### 5.1.3 Constructor

```rust
// v1
let gbuffer_pass = GBufferPass::new(&device, &config);

// v2
let eval_points = vec![/* from register_eval_points */];
let gbuffer_pass = GBufferPass::new(&device, &config, &eval_points);
```

### 5.2 TransparentPass

The TransparentPass undergoes the most significant change — a full
rewrite.

#### 5.2.1 Old behaviour (v1)

- Single hardcoded vertex/fragment shader pair.
- Used `FLAG_TRANSPARENT_ONLY` to identify instances.
- No support for custom transparent templates.
- Depth-write disabled globally.

#### 5.2.2 New behaviour (v2)

- Registers the `"transparent"` eval point.
- Iterates all eval-point-enabled materials whose mask includes the
  transparent bit.
- Supports **per-material** vertex/fragment shaders (custom templates).
- Backward compatibility shim: if no custom transparent templates are
  registered, the pass falls back to the old built-in transparent
  shader.

#### 5.2.3 Constructor

```rust
// v1
let transparent_pass = TransparentPass::new(&device, &config);

// v2
let transparent_pass = TransparentPass::new(&device, &config, &eval_points);
```

#### 5.2.4 register_eval_points

```rust
impl Pass for TransparentPass {
    fn register_eval_points(&self) -> Vec<EvalPointConfig> {
        vec![EvalPointConfig {
            name: "transparent".into(),
            bgl: self.eval_point_bgl(),
            vertex_shader: self.default_vs(),     // fallback; overridable per-material
            fragment_shader: self.default_fs(),   // fallback; overridable per-material
            depth_write: false,
            blend_mode: BlendMode::Alpha,
        }]
    }
}
```

### 5.3 ShadowPass

#### 5.3.1 Changes

The `ShadowPass` gains a `register_eval_points` method but it
**returns an empty vec** — shadows do not use eval points in v2 (they
continue to use their own material-class dispatch internally).

```rust
impl Pass for ShadowPass {
    fn register_eval_points(&self) -> Vec<EvalPointConfig> {
        vec![]  // Shadows are not eval-point-driven
    }
}
```

The constructor signature is unchanged. No shader changes needed.

### 5.4 Third-party passes

Third-party passes that implement the `Pass` trait **must** add the
`register_eval_points` method in v2, but the **default trait method**
returns an empty vec — existing code compiles without any changes:

```rust
// In helio_radiant crate — Pass trait definition (v2)
pub trait Pass: Send + Sync {
    fn name(&self) -> &'static str;
    fn execute(&mut self, ...);
    fn prepare(&mut self, ...);

    /// NEW in v2 — returns the eval points this pass exposes.
    /// Default: no eval points (pass does not participate in
    /// eval-point dispatch).
    fn register_eval_points(&self) -> Vec<EvalPointConfig> {
        vec![]
    }
}
```

If the third-party pass does need eval points, the author adds:

```rust
impl Pass for MyCustomPass {
    fn register_eval_points(&self) -> Vec<EvalPointConfig> {
        vec![EvalPointConfig {
            name: "my_custom".into(),
            bgl: /* shared or custom BGL */,
            vertex_shader: self.custom_vs(),
            fragment_shader: self.custom_fs(),
            depth_write: true,
            blend_mode: BlendMode::None,
        }]
    }

    // ... existing methods unchanged ...
}
```

---

## 6. Removal Timeline

### 6.1 v2.0 (current)

| What | Status |
|---|---|
| New API (`register_material`, eval points) | Fully available |
| Old API (`register_str`, etc.) | Present behind `deprecated-v1-api` feature (default on) |
| Deprecation warnings | Emitted at `warn!` level on first use |
| Compile-time `#[deprecated]` attributes | Present on all deprecated re-exports |
| Migration script | Available in `tools/migration/v1-to-v2/` |
| Documentation | Migration guide in `MIGRATING_v1_to_v2.md` |

### 6.2 v2.1 (target: 3 months after v2.0)

| What | Status |
|---|---|
| `deprecated-v1-api` feature | Removed |
| `crates/.../deprecated.rs` | Deleted |
| Old API symbols | Removed from crate public surface |
| Compile-time `#[deprecated]` attributes | Removed |
| Migration script | Still available; updated to handle edge cases |
| Documentation | Migration guide updated; deprecation warnings removed |
| CI gate | `deny(deprecated)` is enforced in workspace `Cargo.toml` |

### 6.3 Migration window

Users have **one minor release cycle** (v2.0 → v2.1) to migrate. The
following timeline is recommended:

| Phase | Timeline | Action |
|---|---|---|
| **Audit** | v2.0 day 1 | Run `cargo check 2>&1 | grep deprecated` or use the migration script |
| **Fix scene code** | v2.0 + 2 weeks | Replace `register_str` calls, remove `template_registry_mut` chains |
| **Fix pass code** | v2.0 + 4 weeks | Add `register_eval_points` implementations, update constructors |
| **Remove shim** | v2.0 + 8 weeks | Feature-flag gate shim off; test with `deprecated-v1-api` disabled |
| **Final cleanup** | v2.0 + 12 weeks | Delete shim; release v2.1 |

### 6.4 Compilation guarantee

All workspace members **must** compile without warnings under
`cargo check --workspace` by v2.1. The CI pipeline enforces
`#![deny(deprecated)]` in every crate with the tag:

```rust
#![deny(deprecated)]  // Added in v2.1
```

---

## 7. Migration Script

A migration script lives at `tools/migration/v1-to-v2/migrate.py`
(supported) and `tools/migration/v1-to-v2/migrate.rs` (community).
Both produce identical output.

### 7.1 Python `migrate.py`

```bash
python tools/migration/v1-to-v2/migrate.py \
    --workspace ./crates \
    --apply     # optional; without this, dry-run only
```

#### 7.1.1 What it scans

Uses `regex` + `tree-sitter` (Rust grammar) to find:

| Pattern | Detection |
|---|---|
| `.register_str(` | Regex: `\.register_str\(` |
| `.register_partial_str(` | Regex: `\.register_partial_str\(` |
| `.template_registry_mut(` | Regex: `\.template_registry_mut\(` |
| `.material_class_ranges(` | Regex: `\.material_class_ranges\(` |
| `.tag_transparent(` | Regex: `\.tag_transparent\(` |
| `FLAG_TRANSPARENT_ONLY` | Regex: `FLAG_TRANSPARENT_ONLY` |
| `MaterialClassId::from_raw(` | Regex: `MaterialClassId::from_raw\(` |

#### 7.1.2 Suggested replacements

For each match, the script suggests a replacement:

| Matched pattern | Suggested replacement | Confidence |
|---|---|---|
| `registry.register_str("name", wgsl)` | `renderer.register_material("name", wgsl, &["gbuffer"])` | **High** — always safe |
| `registry.register_partial_str("name", wgsl)` | `renderer.register_material("name", wgsl, &["gbuffer"])` | **High** — always safe |
| `registry.template_registry_mut()` | (Remove — identity no-op) | **High** |
| `registry.material_class_ranges()` | `instance.eval_point_mask` | **Medium** — needs manual var check |
| `registry.tag_transparent(id)` | Move `"transparent"` into eval-point list of the `register_material` call | **Medium** — needs code analysis to find associated `register_str` |
| `FLAG_TRANSPARENT_ONLY` | Remove; use eval point mask checks | **Low** — manual review required |

#### 7.1.3 `--apply` mode

When `--apply` is passed, the script:

1. Backs up each modified file to `{file}.v1.bak`.
2. Performs substitutions with **high-confidence** patterns only.
3. Leaves a `// TODO(v2-migration):` comment at each **medium/low**
   confidence match.
4. Prints a summary report.

#### 7.1.4 Report output

```
=== Migration Report ====================================
Total files scanned:     142
Total matches found:     34
  - High confidence:      28 (auto-fixed)
  - Medium confidence:     4 (TODOs inserted)
  - Low confidence:        2 (manual review required)

Files needing manual review:
  src/scene/glass.rs         (tag_transparent → eval point list)
  src/scene/water.rs         (tag_transparent → eval point list)
  src/passes/custom_ssr.rs   (FLAG_TRANSPARENT_ONLY used)

Auto-fixed files (28):
  src/scene/terrain.rs       register_str → register_material
  src/scene/skybox.rs        register_str → register_material
  ...

Backups saved as *.v1.bak in each directory.
Run `git diff` to review changes.
==========================================================
```

### 7.2 Rust binary `migrate`

A Rust equivalent is provided for environments without Python:

```bash
cargo run --manifest-path tools/migration/v1-to-v2/Cargo.toml -- --workspace ./crates
```

Behaviour is identical to the Python version.

---

## 8. Breaking Changes List

Every change below is **breaking** — code that compiled under v1
**will not compile** under v2 without modification. Each entry
includes the error symptom and the fix.

### 8.1 `GpuInstanceData` layout change

| | |
|---|---|
| **What changed** | Added `eval_point_mask: u32` field |
| **v1 struct** | `GpuInstanceData { model, normal, albedo, roughness, metallic, material_class_id }` |
| **v2 struct** | `GpuInstanceData { model, normal, albedo, roughness, metallic, material_class_id, eval_point_mask }` |
| **Error symptom** | `error[E0063]: missing field 'eval_point_mask' in initializer of 'GpuInstanceData'` |
| **Fix** | Add `eval_point_mask: renderer.eval_point_mask_for(material_class_id)` when constructing |
| **Shader impact** | All shaders reading `GpuInstanceData` must include the new field; WGSL struct must match |

### 8.2 `material_class_ranges` removal

| | |
|---|---|
| **What changed** | `MaterialTemplateRegistry::material_class_ranges()` removed |
| **Error symptom** | `error[E0599]: no method named 'material_class_ranges' found` |
| **Fix** | Use `eval_point_mask` on `GpuInstanceData` combined with `EvalPoint::bit()` comparisons |
| **Shader impact** | None — this was a Rust-only API |

### 8.3 `template_registry_mut` removal

| | |
|---|---|
| **What changed** | `MaterialTemplateRegistry::template_registry_mut()` removed |
| **Error symptom** | `error[E0599]: no method named 'template_registry_mut' found` |
| **Fix** | Remove call; chain to `Renderer::register_material` directly |

### 8.4 `register_str` / `register_partial_str` removal

| | |
|---|---|
| **What changed** | Both methods removed from `MaterialTemplateRegistry` |
| **Error symptom** | `error[E0599]: no method named 'register_str' found` |
| **Fix** | Replace with `renderer.register_material(name, wgsl, &["gbuffer"])` |

### 8.5 `FLAG_TRANSPARENT_ONLY` removal

| | |
|---|---|
| **What changed** | Constant removed |
| **Error symptom** | `error[E0425]: cannot find value 'FLAG_TRANSPARENT_ONLY'` |
| **Fix** | Remove reference; use `eval_point_mask & TRANSPARENT_BIT != 0` at the point of use |

### 8.6 `TransparentPass::new` signature change

| | |
|---|---|
| **What changed** | Added `eval_points: &[EvalPointConfig]` parameter |
| **Error symptom** | `error[E0061]: this function takes 3 arguments but 2 were supplied` |
| **Fix** | Add eval points argument from `register_eval_points()` |

### 8.7 `GBufferPass::new` signature change

| | |
|---|---|
| **What changed** | Added `eval_points: &[EvalPointConfig]` parameter |
| **Error symptom** | `error[E0061]: this function takes 3 arguments but 2 were supplied` |
| **Fix** | Add eval points argument from `register_eval_points()` |

### 8.8 `Pass` trait — new required method `register_eval_points`

| | |
|---|---|
| **What changed** | New method on `Pass` trait with default no-op impl |
| **Error symptom** | None — existing code compiles due to default impl |
| **Hidden risk** | If the pass was expected to expose eval points (e.g., a custom render pass), the default no-op silently skips it. The fix is to override. |

### 8.9 `VertexOutput` layout change

| | |
|---|---|
| **What changed** | Added `@location(5) eval_point_mask: u32` to vertex output struct |
| **Error symptom** | WGSL: `error: 'VertexOutput' does not have member 'eval_point_mask'` in fragment shader |
| **Fix** | Update all WGSL vertex shaders to write `eval_point_mask`, update all WGSL fragment shaders to read it |
| **Shader impact** | Every `.wgsl` file that defines `VertexOutput` must add the field |

### 8.10 GBuffer pass fragment shader — discard pattern

| | |
|---|---|
| **What changed** | Fragment shader must discard for non-gbuffer materials |
| **Error symptom** | Visual: materials that target multiple eval points render incorrectly in gbuffer (duplicate contribution) |
| **Fix** | Add the discard guard `if (eval_point_mask & GBUFFER_BIT) == 0 { discard; }` at the top of the fragment shader |

### 8.11 `Renderer::eval_point_mask_for` new method

| | |
|---|---|
| **What changed** | Added `pub fn eval_point_mask_for(&self, class_id: MaterialClassId) -> u32` |
| **Error symptom** | None — this is additive |
| **Action** | Use this to populate `GpuInstanceData::eval_point_mask` when constructing instances; if not set correctly, materials may render in wrong passes |

### 8.12 `EvalPointConfig` struct new

| | |
|---|---|
| **What changed** | Introduced `EvalPointConfig { name, bgl, vertex_shader, fragment_shader, depth_write, blend_mode }` |
| **Error symptom** | `error[E0432]: unresolved import 'EvalPointConfig'` |
| **Fix** | Import from `helio_radiant::eval_point::EvalPointConfig` |

---

## 9. Testing Strategy

### 9.1 Reference renders

For every built-in scene in the test suite:

| Test | v1 reference | v2 output | Acceptable diff |
|---|---|---|---|
| `basic_opaque` | `ref/basic_opaque_v1.png` | `ref/basic_opaque_v2.png` | ≤ 0.5% pixel diff (FP precision) |
| `alpha_cut` | `ref/alpha_cut_v1.png` | `ref/alpha_cut_v2.png` | ≤ 0.5% pixel diff |
| `glass` | `ref/glass_v1.png` | `ref/glass_v2.png` | ≤ 1.0% pixel diff (alpha blend) |
| `water` | `ref/water_v1.png` | `ref/water_v2.png` | ≤ 1.0% pixel diff |
| `shadow_map` | `ref/shadow_map_v1.png` | `ref/shadow_map_v2.png` | ≤ 0.5% pixel diff |
| `full_scene` | `ref/full_scene_v1.png` | `ref/full_scene_v2.png` | ≤ 1.0% pixel diff |

**Method**: Use `tests/render_compare.rs` which renders each scene
with both v1 and v2 code paths (v1 path exists only when
`deprecated-v1-api` feature is on) and compares via
`image::PixelComparison::eq` with a per-channel threshold of `2`
(out of 255).

### 9.2 Performance benchmarks

| Benchmark | v1 baseline (ms/frame) | v2 target (ms/frame) | Regression limit |
|---|---|---|---|
| `bench_opaque_1000` | `1.20` | `1.20` | +5% |
| `bench_opaque_10000` | `8.50` | `8.50` | +5% |
| `bench_transparent_500` | `0.90` | `0.90` | +5% |
| `bench_mixed_5000` | `6.20` | `6.30` | +10% (new eval-point dispatch) |

**Method**: `cargo bench --bench render_bench` records wall-clock time
per frame averaged over 1000 frames. Regression gates are defined in
`ci/benchmark-tolerances.toml`.

### 9.3 Material preview tests

For every built-in eval point, verify that a test material targeting
only that eval point produces correct output:

| Eval point | Test scene | Checks |
|---|---|---|
| `gbuffer` | `test_material_gbuffer` | Albedo, normal, roughness, metallic render correctly |
| `transparent` | `test_material_transparent` | Alpha blending, order-independent transparency |
| `ssr` | `test_material_ssr` | Reflection texture is written and sampled |
| `post_process` | `test_material_post` | Full-screen triangle output |

**Method**: `tests/material_preview.rs` — instantiates a material with
a single eval point, renders one frame, and compares against a
hand-verified reference.

### 9.4 Migration script tests

| Test | Input | Expected output |
|---|---|---|
| `migrate_basic_opaque` | `registry.register_str("a", wgsl)` | `renderer.register_material("a", wgsl, &["gbuffer"])` |
| `migrate_transparent` | `registry.register_str("a", wgsl)` + `registry.tag_transparent(id)` | `renderer.register_material("a", wgsl, &["gbuffer", "transparent"])` |
| `migrate_mut_chain` | `registry.template_registry_mut().register_str(...)` | `renderer.register_material(...)` |
| `migrate_partial` | `registry.register_partial_str("a", wgsl)` | `renderer.register_material("a", wgsl, &["gbuffer"])` |

**Method**: `tests/migration_script.rs` — runs the migration script
(python or rust) on a set of known fixture files and asserts the
output matches expected results.

### 9.5 Negative tests

Ensure that v1-only code fails to compile when `deprecated-v1-api` is
disabled:

```rust
// tests/compile_fail/register_str_removed.rs
// ```compile_fail
// use helio_radiant::prelude::*;
// fn main() {
//     let registry = get_registry();
//     registry.register_str("x", "/* wgsl */");  //~ ERROR: no method named `register_str`
// }
// ```
```

These tests are gated behind `ignore = false` only in CI when testing
the v2.1 profile.

### 9.6 CI pipeline

```yaml
# .github/workflows/ci.yml (v2.0+)
migration-tests:
  steps:
    - run: cargo check --workspace --features deprecated-v1-api
    - run: python tools/migration/v1-to-v2/migrate.py --workspace crates --check
    - run: cargo test --test render_compare
    - run: cargo bench --bench render_bench

v2-1-compliance:
  steps:
    - run: cargo check --workspace  # deprecated-v1-api is off
    - run: cargo check --workspace 2>&1 | grep -q "deprecated" && exit 1 || true
    - run: cargo test --test render_compare
```

---

> **End of section: Migration & Backward Compatibility**  
> Next section: *TBD*

---



