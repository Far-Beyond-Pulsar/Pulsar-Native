//! How much of each texture's mip chain the GPU holds, decided by a VRAM
//! budget.
//!
//! Every cooked texture carries its whole mip chain. When the textures the
//! scenes show would take more video memory than the project allows, the top
//! levels (the largest, and about three quarters of a texture's bytes) are left
//! off at upload: all textures drop the same number of levels, the *detail
//! bias*. A bias of 1 holds each texture at half its width and height for a
//! quarter of the bytes; the sampler reads the levels that are there, so the
//! picture softens instead of memory running out.
//!
//! The bias follows the total *native* size of the resident textures (what
//! they would take in full) against `streaming.texture_vram_budget_mb`. A
//! texture is uploaded at the bias current when it is bound, and uploaded
//! again at the new bias the next time it is bound after the bias changed, so
//! the detail comes back by itself when scenes close and the total shrinks.

use std::sync::atomic::{AtomicU32, Ordering};

/// The most levels dropped, which divides every texture's bytes by about 4096.
pub const MAX_BIAS: u32 = 6;

/// The budget when the project sets none: 3 GB of cooked textures.
pub const DEFAULT_BUDGET_MB: i64 = 3072;

static BIAS: AtomicU32 = AtomicU32::new(0);

/// The number of top mip levels left off new uploads right now.
pub fn detail_bias() -> u32 {
    BIAS.load(Ordering::Relaxed)
}

pub fn set_detail_bias(bias: u32) {
    BIAS.store(bias.min(MAX_BIAS), Ordering::Relaxed);
}

/// The smallest number of levels to drop so `native_bytes` fits `budget_bytes`.
/// A budget of 0 means no limit.
pub fn bias_for_budget(native_bytes: u64, budget_bytes: u64) -> u32 {
    if budget_bytes == 0 {
        return 0;
    }
    let mut bias = 0;
    while bias < MAX_BIAS && (native_bytes >> (2 * bias)) > budget_bytes {
        bias += 1;
    }
    bias
}

/// Project setting `streaming.texture_vram_budget_mb` in bytes; 0 is no limit.
pub fn vram_budget_bytes() -> u64 {
    let configured = engine_state::settings::global_config()
        .get(engine_state::settings::NS_PROJECT, "streaming", "texture_vram_budget_mb")
        .ok();
    let megabytes = match configured {
        Some(engine_state::settings::ConfigValue::Int(value)) => value,
        _ => DEFAULT_BUDGET_MB,
    };
    megabytes.max(0) as u64 * 1024 * 1024
}

/// Set the detail bias for `native_bytes` of resident textures.
pub(crate) fn rebalance(native_bytes: u64) -> u32 {
    let bias = bias_for_budget(native_bytes, vram_budget_bytes());
    set_detail_bias(bias);
    bias
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn nothing_is_dropped_while_the_textures_fit() {
        assert_eq!(bias_for_budget(100 * MB, 100 * MB), 0);
        assert_eq!(bias_for_budget(0, 100 * MB), 0);
    }

    #[test]
    fn each_dropped_level_divides_the_bytes_by_four() {
        assert_eq!(bias_for_budget(101 * MB, 100 * MB), 1);
        assert_eq!(bias_for_budget(400 * MB, 100 * MB), 1);
        assert_eq!(bias_for_budget(401 * MB, 100 * MB), 2);
        assert_eq!(bias_for_budget(1600 * MB, 100 * MB), 2);
        assert_eq!(bias_for_budget(1601 * MB, 100 * MB), 3);
    }

    #[test]
    fn a_zero_budget_is_unlimited_and_the_bias_is_capped() {
        assert_eq!(bias_for_budget(u64::MAX, 0), 0);
        assert_eq!(bias_for_budget(u64::MAX, 1), MAX_BIAS);
    }

    #[test]
    fn the_bias_never_exceeds_the_cap_when_set() {
        set_detail_bias(99);
        assert_eq!(detail_bias(), MAX_BIAS);
        set_detail_bias(0);
        assert_eq!(detail_bias(), 0);
    }
}
