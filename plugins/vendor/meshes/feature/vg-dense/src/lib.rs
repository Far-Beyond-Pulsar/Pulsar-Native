//! Virtual geometry Tier 3: ultra-dense overlap (plan
//! `.agents/plans/virtual-geometry.md` section 5.4).
//!
//! Dense 16x16 tiles (tiles where Tier 1/2 overdraw is pathological, flagged by
//! T7's `tile_classify`) are re-resolved here, replacing whatever Tier 1/2
//! wrote for those pixels. Two interchangeable backends produce the same
//! output, a per-pixel `(depth bits, vis id)` key in a storage buffer
//! ([`DenseBuffers::dense_key`]), so T9's `vis_resolve` is backend-agnostic:
//!
//! * [`DenseNonRtPass`] (3-NonRT, runs everywhere, no int64): exact tiled
//!   depth peel over the real cluster triangles.
//! * [`DenseRtPass`] (3-RT, gated): primary rays through a TLAS of instance
//!   proxies. Approximate by construction (see "Proxy-LOD risk").
//!
//! [`Tier3Selector`] / [`rt_available`] implement the `VgMode` policy.
//!
//! # 3-NonRT algorithm (shaders: `tiles`, `args`, `bin`, `peel`)
//!
//! 1. `compact`: dense-tile mask -> compact tile list (+ tile -> slot map).
//! 2. `make_args`: indirect dispatch sizes.
//! 3. `bin`: one thread per visible cluster adds the cluster to the list of
//!    every dense tile its conservative screen bound overlaps
//!    (cap `cluster_cap`, overflow counted).
//! 4. `peel`: one workgroup per dense tile. It streams the tile's clusters
//!    through shared memory (vertex transform, triangle setup) to build a
//!    `2^bucket_bits`-bucket histogram of nearest-vertex reversed-Z depth
//!    (12 bits by default), then peels front-to-back: each layer takes the
//!    nearest unpeeled buckets worth `slab_tris` triangles and tests only
//!    those. A pixel is final once its depth beats everything the remaining
//!    buckets can hold; the tile stops when all pixels are final. Layers
//!    beyond K (`nominal_layers`, default 4) are extra iterations; at
//!    `max_layers` the tile clamps and bumps the `clamped_*` counters.
//!
//! Shared memory is 13.3 KiB at the defaults and 5.7 KiB in the
//! low-shared-memory config ([`DenseConfig::low_shared_memory`], 8x8 tiles,
//! K = 2, per plan section 6).
//!
//! Deviations from the plan text, all deliberate:
//!
//! * Triangles are not stored in shared memory; only the histogram, one
//!   cluster's vertices and a 64-triangle setup batch are. Layers therefore
//!   re-stream the tile's clusters (re-transforming vertices). Layer count,
//!   not shared-memory capacity, is what bounds work, and shared memory is
//!   bounded regardless of triangle count.
//! * The "bucket sort" is a histogram partition into depth slabs, not a full
//!   sort of triangles; ordering inside a slab is irrelevant because every
//!   pixel takes the max over the slab.
//! * Clusters are binned to tiles at cluster (sphere) granularity in global
//!   memory, not triangle granularity; triangle binning happens in shared
//!   memory per tile.
//! * No `T7` shared-memory tile-bin code was reused (it did not exist when
//!   this was written); the vertex-stage/setup code here is therefore a
//!   second copy of what `vg_tile_raster.wgsl` does and may be unified later.
//!
//! # Inputs (reconciled in T10a)
//!
//! `shaders/inputs.wgsl` lists them: the dense tile mask (one `u32` per tile),
//! the cull's `state` buffer (its per-phase, per-bin `{first, count}` ranges
//! say which entries of the unified visible list are live; BOTH phases and ALL
//! six bins are binned into dense tiles, because a dense tile replaces what
//! Tier 1/2 drew there), the cull's `visible_clusters` (`{instance,
//! cluster_slot}`), the `helio-vg-stream` page pool and `vg_asset_quant`
//! decoded by the shared `helio_vg_format::DECODE_WGSL`, `vg_instances`, and the
//! 252-byte `static_objects` rows. The `bin` kernel is dispatched indirectly as
//! rows of 32768 workgroups of 64 threads (`args[3..6]`).
//!
//! # Contract for T9 / T10
//!
//! * Output: `DenseBuffers::dense_key[y * width + x] = (depth f32 bits, id)`,
//!   `id = vis_pack(visible_cluster, tri)`, depth bits 0 = empty. Valid only
//!   for pixels of dense tiles (every pixel of every processed dense tile is
//!   written, including empty ones). Reversed-Z: larger depth is nearer, so
//!   merging with other tiers is `max` on the `(depth, id)` pair. T9 must
//!   *replace* (not merge) Tier 1/2 results inside dense tiles.
//! * 3-RT additionally fills `DenseRtPass::dense_bary` (hardware barycentrics
//!   as `bary_pack(u, v)`); NonRT leaves barycentrics to `vis_resolve`.
//! * 3-RT vis ids use cluster indices `>= DENSE_RT_VISIBLE_BASE (1 << 24)`,
//!   resolved through `DenseRtPass::rt_visible()` instead of T5's visible
//!   list. T5 must therefore keep its own indices below `1 << 24`.
//! * Counters: [`DenseStats`] (`copy_stats`/`read_stats`), surfaced in the
//!   debug UI; `overflowed()` means dropped or clamped work.
//! * Frame order: Tier 1/2 raster -> `tile_classify` -> Tier 3 -> `vis_resolve`.
//!
//! # Proxy-LOD risk (3-RT) -- read before trusting RT output
//!
//! Hardware ray tracing intersects a fixed BLAS; it cannot evaluate the
//! per-pixel DAG cut the raster tiers do. Each instance is traced through a
//! single *proxy LOD*: a uniform cut of its cluster DAG, expanded to plain
//! triangles. The LOD is chosen per instance so its projected error at the
//! instance's *nearest* distance is at most tau ([`select_proxy_lod`]). Hence:
//!
//! * Large instances spanning a wide distance range are traced at the LOD for
//!   their nearest point, i.e. finer than necessary elsewhere (cost, not error).
//! * If the finest streamed proxy still exceeds tau at that distance, the
//!   result silently has more error than raster would; it is flagged in
//!   [`RtFrameInfo::over_tau_instances`] but not corrected.
//! * Proxy triangles are real cluster triangles, so silhouettes can differ
//!   from raster output by up to tau between LOD cuts, and cracks between
//!   adjacent clusters of different DAG depth cannot occur only if the proxy
//!   cut is a consistent DAG cut (builder responsibility).
//! * Costs: one BLAS per (asset, proxy LOD) in use plus a TLAS rebuild when
//!   instances or selections change; memory is ~tri_count of the chosen proxy
//!   LODs per distinct asset.
//! * wgpu ray queries are an experimental feature and the repo strips them on
//!   adapters with a shader-compiler crash (`usable_adapter_features`); on
//!   those, [`rt_available`] is false and only 3-NonRT runs.
//! * Primary rays start at `ray_t[0]` (set to the near distance) and use
//!   opaque, force-opaque geometry: no alpha-tested proxies (alpha-masked
//!   clusters are only resolved by the depth peel).
//!
//! # 3-RT in the render graph (as wired)
//!
//! [`VgDensePass`] drives both backends. The CPU side is [`RtTier`]:
//!
//! * Proxies: `helio_vg_stream::proxy` builds the proxy ladder of an asset
//!   (consistent DAG cuts at geometric error levels, welded `Float32x3` +
//!   `u32`) on background threads under a byte budget with LRU eviction; at
//!   most one finished proxy is uploaded per frame.
//! * Instances: the `collect` kernel ([`RtCollect`]) marks, GPU-side and
//!   deduplicated, the instances overlapping dense tiles and appends their
//!   transform/asset to a bounded buffer that is read back asynchronously
//!   (1-3 frames late, never waited on). CPU cost is O(dense instances).
//! * Pages: the pool slot of a proxy cluster is only known at frame time, so
//!   proxies are page-addressed; the tier asks the store for the pages of the
//!   selected levels and leaves an instance out (frame falls back to NonRT)
//!   while one is not resident.
//! * TLAS: rebuilt only when the instance set, a selected level, a BLAS or a
//!   transform changes (a full rebuild each time: no refit); the tables
//!   (instance rows, RT visible rows) only when the set, a level or the pool
//!   residency changed. Counted in `VgTier3Stats`.
//! * Policy: see [`graph`]. `ForceNonRtFallback` never creates the tier.
//!
//! Known limits: 1-3 frame candidate latency (a newly dense instance can be a
//! hole for those frames while RT is active; the selector then falls back to
//! NonRT until the TLAS covers it); uniform proxy error (not per pixel); no
//! TLAS refit, no per-instance material/alpha; one proxy upload per frame;
//! proxy CPU copies of the prim_map stay resident; resolve merges the RT keys
//! with Tier 1/2 by depth rather than replacing them.

pub mod collect;
pub mod common;
pub mod config;
pub mod graph;
pub mod mask_union;
pub mod mode;
pub mod nonrt;
pub mod peel_ref;
pub mod rt;
pub mod rt_tier;
pub mod stats_readback;

pub use collect::{Candidate, CollectResult, RtCollect, CAND_CAPACITY};
pub use common::{read_buffer, DenseBuffers, DenseFrame, DenseParamsGpu, DenseStats};
pub use config::DenseConfig;
pub use graph::{VgDenseFrameData, VgDensePass, KEY_DENSE};
pub use mode::{rt_available, AutoPolicy, Tier3Backend, Tier3Selector};
pub use nonrt::{DenseError, DenseNonRtInputs, DenseNonRtPass};
pub use rt::{
    pixels_per_unit, pixels_per_unit_from_proj, proxy_mesh_id, select_proxy_lod, DenseRtPass,
    DenseVisibleGpu, ProxyAsset, ProxyLod, ProxySelection, RtError, RtFrameInfo, RtInstanceGpu,
    RtInstanceRequest, DENSE_RT_VISIBLE_BASE,
};
pub use rt_tier::{RtEnv, RtPrep, RtTier, RtTierConfig};

/// The visible-cluster record (`helio_vg_format::GpuVisibleCluster`).
pub type DenseVisible = DenseVisibleGpu;

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(name: &str, src: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            // Capabilities::all() includes RAY_QUERY, needed for `rt`.
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        module
    }

    #[test]
    fn all_shaders_validate_default_and_low_memory() {
        for cfg in [DenseConfig::default(), DenseConfig::low_shared_memory()] {
            cfg.validate().unwrap();
            for (name, src) in config::validation_sources(&cfg) {
                validate(name, &src);
            }
        }
    }

    /// The ray-query shader really exercises the ray-query capability, and
    /// does not validate without it.
    #[test]
    fn rt_shader_requires_ray_query_capability() {
        let src = config::rt_source(&DenseConfig::default());
        assert!(src.starts_with("enable wgpu_ray_query;"));
        let module = naga::front::wgsl::parse_str(&src).expect("parse");
        let without = naga::valid::Capabilities::all() & !naga::valid::Capabilities::RAY_QUERY;
        assert!(
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), without)
                .validate(&module)
                .is_err(),
            "rt.wgsl must depend on the RAY_QUERY capability"
        );
        validate("rt", &src);
    }

    #[test]
    fn rust_params_match_wgsl_params() {
        let src = config::tiles_source(&DenseConfig::default());
        let module = validate("tiles", &src);
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        let (h, _) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("DenseParams"))
            .unwrap();
        assert_eq!(
            layouter[h].size as usize,
            std::mem::size_of::<DenseParamsGpu>()
        );
        let (h, _) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("VgVisibleCluster"))
            .unwrap();
        assert_eq!(
            layouter[h].size as usize,
            std::mem::size_of::<DenseVisibleGpu>()
        );
    }

    #[test]
    fn shared_memory_is_within_budget() {
        let d = DenseConfig::default();
        assert!(
            (8 * 1024..=16 * 1024).contains(&d.shared_memory_bytes()),
            "{}",
            d.shared_memory_bytes()
        );
        let l = DenseConfig::low_shared_memory();
        assert!(l.shared_memory_bytes() < 8 * 1024);
        assert_eq!(DenseConfig::for_workgroup_storage(16384), d);
        assert_eq!(DenseConfig::for_workgroup_storage(8192), l);
    }

    /// The declared workgroup storage in the shader matches the Rust estimate
    /// (within the scalar slack), so the budget test is not vacuous.
    #[test]
    fn declared_workgroup_storage_matches_estimate() {
        for cfg in [DenseConfig::default(), DenseConfig::low_shared_memory()] {
            let module = validate("peel", &config::peel_source(&cfg));
            let mut layouter = naga::proc::Layouter::default();
            layouter.update(module.to_ctx()).unwrap();
            let bytes: u32 = module
                .global_variables
                .iter()
                .filter(|(_, g)| g.space == naga::AddressSpace::WorkGroup)
                .map(|(_, g)| layouter[g.ty].size)
                .sum();
            assert!(
                bytes <= cfg.shared_memory_bytes(),
                "declared {bytes} > estimate {}",
                cfg.shared_memory_bytes()
            );
            assert!(
                bytes + 64 >= cfg.shared_memory_bytes(),
                "estimate {} far above declared {bytes}",
                cfg.shared_memory_bytes()
            );
        }
    }

    #[test]
    fn config_validation_rejects_bad_values() {
        let mut c = DenseConfig::default();
        c.tile = 12;
        assert!(c.validate().is_err());
        let mut c = DenseConfig::default();
        c.cluster_cap = 1024;
        assert!(c.validate().is_err());
        let mut c = DenseConfig::default();
        c.tri_batch = 300;
        assert!(c.validate().is_err());
    }
}
