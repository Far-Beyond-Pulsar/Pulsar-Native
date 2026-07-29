# Transparent Material Pipeline

**Status**: Resolved by Radiant v2 design (see spec at `.agents/radiant-v2-spec.md`)

---

## Original Problem

The engine had no way to render transparent/alpha-blended materials (glass, water,
gemstones) with proper see-through transparency. The gbuffer pass handled all opaque
geometry, and the transparent pass existed but had a fixed shader that couldn't use
custom Radiant templates.

## Root Causes (v1 Architecture)

### 1. Per-pass template ownership
Each pass owned its own `RadiantTemplateRegistry`. The gbuffer pass had templates;
the transparent pass had a fixed shader. Cross-pass template reuse was impossible
without circular dependencies and binding-layout mismatches.

### 2. CPU-side range splitting
`material_class_ranges` was a `Vec<(class, graph_hash, start, count)>` built during
scene rebuild. Every pass iterated it — CPU cost scaled with (passes × material classes).

### 3. No material-to-pass routing
The only way to route a material to a specific pass was ad-hoc flags
(`FLAG_TRANSPARENT_ONLY`, `class >= 100` threshold) that were fragile, opaque, and
impossible to extend.

### 4. Transparent pass was a stub
`TransparentPass` had a hardcoded WGSL shader that ignored material properties.
No custom templates, no per-material alpha, no Fresnel, no SSR interaction.

### 5. No multi-pass materials
A material couldn't participate in multiple passes. Glass couldn't write normals
to the gbuffer (for SSR) AND render as a transparent overlay — it had to choose
one or pay double geometry cost.

## How Radiant v2 Eliminates All of These

### Fix 1: Templates own eval points, not passes

Templates (not passes) declare which evaluation points they target. A template
provides WGSL functions for each point. The engine compiles the variants lazily.

```rust
// Glass template targets BOTH gbuffer (for SSR normals/roughness)
// and transparent (for see-through alpha blending) — one template, two eval points
let glass = engine.register_template(Template {
    name: "glass",
    targets: &["gbuffer", "transparent"],
    eval_sources: Map {
        "gbuffer" => glass_gbuffer_wgsl,       // writes normals, roughness, F0
        "transparent" => glass_transparent_wgsl, // alpha-blended color
    },
});
```

No circular deps. No binding mismatches. No per-pass template registries.

### Fix 2: GPU-side dispatch, no CPU range splitting

`material_class_ranges` is eliminated. The scene rebuild sets `template_id` per
instance (4 bytes, replaces padding). The GPU checks the dispatch mask per-pixel:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
if (mask & (1u << THIS_BIT)) == 0u { discard; }
```

CPU cost: O(1) per pass (single multi_draw). No iteration, no sorting, no ranges.

### Fix 3: Material-to-pass routing via eval point bitmask

Every template declares its target eval points at registration time. The dispatch
mask is a single u32 bitfield — 32 possible eval points, each material can target
any subset. No flags, no thresholds, no fragile heuristics.

### Fix 4: Transparent pass gets full template support

`TransparentPass` declares the `"transparent"` evaluation point with its own base
shader, blend mode (SrcAlpha / OneMinusSrcAlpha), and read-only depth. Any template
that targets `"transparent"` gets its WGSL composed into the transparent pass.
Custom glass, water, and gem templates work identically to gbuffer templates.

### Fix 5: Multi-pass materials are natural

A template can target any number of eval points. Glass targets `["gbuffer",
"transparent", "ssr"]`. Water targets `["transparent"]` only (skips gbuffer
entirely — zero cost). The engine compiles one shader variant per (eval point ×
template_id) pair, and the per-pixel dispatch check ensures each pass only
evaluates pixels whose template targets it.

## The Template Abstraction (Key Innovation)

The system doesn't operate on materials at the pipeline level — it operates on
**templates**. The engine ships ~30 built-in templates (PBR, clear coat, glass,
water, skin, etc.). Users create materials by instantiating a template with
parameters. Since thousands of materials can share the same template, the dispatch
cost is bounded by the number of templates (~30), not the number of materials
(potentially unlimited).

## Per-Pixel Check Cost

The concern that checking `template_dispatches[template_id].eval_point_mask &
BIT` per pixel per pass is expensive turns out to be unfounded:

- **Fragment passes**: Early-Z culling kills non-targeting pixels before the
  fragment shader runs. For the common case (opaque pixel in transparent pass),
  the check costs exactly 0 cycles.
- **Compute passes**: The check costs ~40-80 cycles per pixel vs. ~500-2000+
  cycles for the full evaluation. It saves 10-50× more than it costs.

See `.agents/radiant-v2-spec.md` (Second-Pass Revision section) for full analysis.

## Specification

The full Radiant v2 specification is at `.agents/radiant-v2-spec.md` (11,138 lines,
475 KB). It covers:

- Eval Point Registry — how passes declare injection points
- Material Registration & Shader Composition — template registration, multi-eval WGSL
- Instance Data & GPU Dispatch — template_id in instance data, GPU-side dispatch
- Fragment Pass Integration — gbuffer, transparent, shadow, depth, velocity passes
- Compute Pass Integration — SSR, SSS, decal, post-process compute patterns
- Editor Integration — material graph → eval function compilation, presets, hot-reload
- Migration & Backward Compatibility — v1 → v2 API mapping, deprecation timeline
