//! The one way GPU-driven passes issue their indirect draws (Helio#306).
//!
//! wgpu validates `multi_draw_indexed_indirect` one draw slot at a time on
//! the CPU inside `CommandEncoder::finish` whenever
//! `InstanceFlags::VALIDATION_INDIRECT_CALL` is set (part of wgpu's default
//! flags, release builds included), so a pass's encode cost grew with the
//! number of draw groups even though it recorded a single call.
//! `multi_draw_indexed_indirect_count` has no per-draw CPU work, so its cost
//! is flat. Every GPU-driven draw goes through [`multi_draw_indexed_indirect`]
//! so the fast path is the default wherever the device supports it.

/// Where a draw's count lives on the GPU: a `u32` at `offset` in `buffer`.
#[derive(Clone, Copy)]
pub struct GpuDrawCount<'a> {
    pub buffer: &'a wgpu::Buffer,
    pub offset: u64,
}

/// Size of one `DrawIndexedIndirectArgs` entry (5 × `u32`).
pub const DRAW_INDEXED_INDIRECT_STRIDE: u64 = 20;

/// Draw `count` indexed-indirect entries of `indirect`, starting at entry
/// `first`.
///
/// With `gpu_count` (only offered when the device has
/// `MULTI_DRAW_INDIRECT_COUNT`) this is one `multi_draw_indexed_indirect_count`
/// with `count` as its maximum; without it, one `multi_draw_indexed_indirect`;
/// on wasm32, which has no multi-draw, one `draw_indexed_indirect` per entry.
pub fn multi_draw_indexed_indirect(
    pass: &mut helio_core::RenderCmds<'_>,
    indirect: &wgpu::Buffer,
    first: u32,
    count: u32,
    gpu_count: Option<GpuDrawCount<'_>>,
) {
    if count == 0 {
        return;
    }
    let offset = first as u64 * DRAW_INDEXED_INDIRECT_STRIDE;
    #[cfg(not(target_arch = "wasm32"))]
    match gpu_count {
        Some(gpu_count) => pass.multi_draw_indexed_indirect_count(
            indirect,
            offset,
            gpu_count.buffer,
            gpu_count.offset,
            count,
        ),
        None => pass.multi_draw_indexed_indirect(indirect, offset, count),
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = gpu_count;
        for i in 0..count as u64 {
            pass.draw_indexed_indirect(indirect, offset + i * DRAW_INDEXED_INDIRECT_STRIDE);
        }
    }
}
