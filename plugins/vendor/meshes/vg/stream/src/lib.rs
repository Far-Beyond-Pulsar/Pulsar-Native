//! Virtual-geometry page streaming (plan: `.agents/plans/virtual-geometry.md`
//! section 2.2/2.3).
//!
//! * [`VgAssetStore`] registers assets by content id from a [`VgeoSource`],
//!   packs their always-resident hierarchy / group refs / headers into shared
//!   GPU arrays with stable `asset_slot`s, and streams their 128 KiB pages
//!   into one lazily grown page pool.
//! * A GPU residency table maps global page id -> pool slot (or
//!   [`PAGE_NOT_RESIDENT`]). Cull appends deduplicated page requests and
//!   "used" marks via `shaders/vg_stream.wgsl`; a non-blocking readback feeds
//!   the [`PageScheduler`] (priority, per-frame budget, LRU eviction that
//!   never evicts root pages).
//! * CPU page bytes are held in SceneDB `Residency<PageBytes>` so they are
//!   budgeted and visible in the Memory Explorer; the GPU pool is named
//!   `vg-stream page pool` in the GPU allocator report.
//!
//! Frame protocol:
//! ```text
//! store.update();                                  // before encoding passes
//! ... cull/raster passes bind store buffers and call vg_request_page ...
//! store.record_feedback_readback(&mut encoder);    // after the cull dispatches
//! queue.submit(..); store.after_submit();
//! ```

pub mod core;
pub mod feedback;
pub mod proxy;
pub mod ranges;
pub mod runtime;
pub mod scheduler;
pub mod source;
pub mod store;

/// The virtual-geometry feature's settings (a renderer config carries them in
/// its feature settings): which runtime the graph builds VG passes for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VgSettings {
    /// `VgRuntime::handle_id` of the runtime, or `None` for classic
    /// rendering only.
    pub runtime: Option<u64>,
}

impl VgSettings {
    pub fn for_runtime(runtime: &runtime::VgRuntime) -> Self {
        Self {
            runtime: Some(runtime.handle_id()),
        }
    }
}

pub use crate::core::{PageBytes, StoreCore, VgStreamConfig, VgStreamError, PAGE_NOT_RESIDENT};
pub use feedback::{Feedback, FeedbackLayout};
pub use proxy::{
    build_proxy, error_ladder, proxy_key, ProxyBuild, ProxyCache, ProxyCacheStats, ProxyLodCpu,
    ProxyOptions, ProxyState,
};
pub use runtime::{content_words, VgAssetInfo, VgRuntime, VgRuntimeSettings, VgTier3Stats};
pub use scheduler::{Fetch, PageScheduler, SchedulerConfig, SchedulerStats};
pub use source::{testing, VgeoBytes, VgeoSource};
pub use store::{
    FeedbackReadbackPlan, VgAssetHandle, VgAssetStore, VgBindKind, VgBuffer, VgPoolStats,
    VgStreamBuffers,
};

/// Residency lookup and page data helpers. Prepend
/// `helio_vg_format::WGSL` and [`bindings_wgsl`] first.
pub const WGSL: &str = include_str!("../shaders/vg_stream.wgsl");

/// Request/touch helpers; compute stages only (need the read-write feedback binding).
pub const WGSL_FEEDBACK: &str = include_str!("../shaders/vg_stream_feedback.wgsl");

/// Storage bindings the helpers expect, at `@group(group)` starting at
/// `first_binding`. `include_feedback = false` omits the read-write feedback
/// buffer (for vertex/fragment stages; matches [`VgBindKind::ReadOnly`]).
/// Requires `helio_vg_format::WGSL` before it (struct types).
pub fn bindings_wgsl(group: u32, first_binding: u32, include_feedback: bool) -> String {
    let b = |i: u32| format!("@group({group}) @binding({})", first_binding + i);
    let mut s = String::new();
    s += &format!("{} var<storage, read> vg_page_pool: array<u32>;\n", b(0));
    s += &format!("{} var<storage, read> vg_residency: array<u32>;\n", b(1));
    s += &format!(
        "{} var<storage, read> vg_nodes: array<VgHierarchyNode>;\n",
        b(2)
    );
    s += &format!(
        "{} var<storage, read> vg_group_refs: array<VgGroupRef>;\n",
        b(3)
    );
    s += &format!(
        "{} var<storage, read> vg_asset_headers: array<VgAssetHeader>;\n",
        b(4)
    );
    s += &format!(
        "{} var<storage, read> vg_asset_bases: array<vec4<u32>>;\n",
        b(5)
    );
    s += &format!(
        "{} var<storage, read> vg_asset_quant: array<vec4<f32>>;\n",
        b(6)
    );
    if include_feedback {
        s += &format!(
            "{} var<storage, read_write> vg_feedback: array<atomic<u32>>;\n",
            b(7)
        );
    }
    s
}

/// Full shader prelude: format declarations + bindings + helpers (compute).
pub fn prelude_wgsl(group: u32, first_binding: u32) -> String {
    format!(
        "{}
{}
{}
{}",
        helio_vg_format::WGSL,
        bindings_wgsl(group, first_binding, true),
        WGSL,
        WGSL_FEEDBACK
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(src: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
        module
    }

    #[test]
    fn wgsl_prelude_validates_with_helpers_used() {
        let src = format!(
            "{}\n@compute @workgroup_size(1) fn main() {{
                let s = vg_use_or_request_page(3u);
                if (s != VG_PAGE_NOT_RESIDENT) {{
                    let c = vg_load_cluster(s, 0u);
                    vg_touch_page(u32(c.self_error));
                }}
                let r = vg_asset_headers[0].root_node + vg_node_base(0u) + vg_group_ref_base(0u)
                    + vg_page_base(0u) + select(0u, 1u, vg_asset_ready(0u))
                    + vg_global_page(0u, vg_group_refs[0]) + vg_nodes[0].children.x;
                vg_request_page(r);
            }}",
            prelude_wgsl(0, 0)
        );
        validate(&src);
    }

    #[test]
    fn read_only_bindings_validate_in_vertex_stage() {
        let src = format!(
            "{}\n{}\n{}\n@vertex fn vs() -> @builtin(position) vec4<f32> {{
                let s = vg_page_slot(0u);
                return vec4<f32>(f32(vg_page_u32(s, 0u)), 0.0, 0.0, 1.0);
            }}",
            helio_vg_format::WGSL,
            bindings_wgsl(1, 4, false),
            WGSL
        );
        validate(&src);
    }
}
