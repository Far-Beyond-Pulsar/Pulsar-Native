//! The eight-target pipeline contract.
//!
//! This is the plan's corrected §12 perf lever, and the part of it that is a *safety*
//! property rather than an optimisation. Grass writes five of the eight G-buffer
//! targets. The pipeline still declares all eight, because a pipeline's fragment targets
//! must match the render pass's colour attachments in count and format
//! (`RenderPassContext::check_compatible` compares them element-wise), so a five-target
//! pipeline would need its own render pass and forfeit the subpass fusion this pass is
//! positioned for.
//!
//! The three it does not write therefore carry `ColorWrites::empty()`. Without the mask
//! a declared target with no shader output has an **undefined** value, and the deferred
//! pass reads `sss` and `extra` for every pixel — so the failure is not "grass is
//! slightly wrong", it is "everything grass touches has garbage subsurface and
//! anisotropy parameters".

use helio_pass_foliage_gbuffer::*;

#[test]
fn there_are_exactly_eight_targets_in_gbuffer_order() {
    // Order and format copied from `GBufferPass::declare_resources` /
    // `get_or_create_pipeline`. The chain will only fuse if the attachment lists match
    // exactly, so a divergence here does not error — it silently costs a store and
    // reload of the whole G-buffer.
    assert_eq!(GBUFFER_TARGET_FORMATS.len(), 8);
    assert_eq!(
        GBUFFER_TARGET_FORMATS,
        [
            wgpu::TextureFormat::Rgba8Unorm,  // 0 albedo
            wgpu::TextureFormat::Rgba16Float, // 1 normal
            wgpu::TextureFormat::Rgba8Unorm,  // 2 orm
            wgpu::TextureFormat::Rgba16Float, // 3 emissive
            wgpu::TextureFormat::Rg16Float,   // 4 lightmap_uv
            wgpu::TextureFormat::Rgba16Float, // 5 sss
            wgpu::TextureFormat::Rgba16Float, // 6 extra
            wgpu::TextureFormat::Rgba16Float, // 7 velocity and depth residual
        ]
    );

    let targets = color_target_states();
    assert_eq!(targets.len(), 8);
    for (index, target) in targets.iter().enumerate() {
        let target = target.as_ref().expect("every target slot must be occupied");
        assert_eq!(
            target.format, GBUFFER_TARGET_FORMATS[index],
            "target {index} format diverged from the G-buffer's"
        );
    }
}

#[test]
fn the_unwritten_targets_have_an_empty_write_mask() {
    // MANDATORY, not cosmetic. See the module docs.
    assert_eq!(UNWRITTEN_TARGET_INDICES, [5, 6]);
    let targets = color_target_states();
    for &index in &UNWRITTEN_TARGET_INDICES {
        let mask = targets[index].as_ref().unwrap().write_mask;
        assert_eq!(
            mask,
            wgpu::ColorWrites::empty(),
            "target {index} (sss / extra) must be write-masked off — grass \
             emits no @location for it, and an unwritten fragment output is undefined"
        );
    }
}

#[test]
fn the_written_targets_write_every_channel() {
    // Lightmap UV (4) included: grass writes its "no lightmap" value.
    let targets = color_target_states();
    for index in [0usize, 1, 2, 3, 4, 7] {
        assert_eq!(
            targets[index].as_ref().unwrap().write_mask,
            wgpu::ColorWrites::ALL,
            "target {index} must write all channels"
        );
    }
}

#[test]
fn no_target_blends() {
    // The LOD cross-fade is a stochastic alpha *test* resolved by TAA, not a blend.
    // Blending grass into the G-buffer would write a half-weight normal and a
    // half-weight roughness, which is not a surface any deferred lighting model can
    // interpret — the pixel would be lit as a material that does not exist.
    for target in color_target_states().iter().flatten() {
        assert!(target.blend.is_none());
    }
}

#[test]
fn the_written_and_unwritten_sets_partition_the_eight_targets() {
    // A target that is in neither set is a target nobody thought about.
    let written = [0usize, 1, 2, 3, 4, 7];
    let mut seen = [false; 8];
    for index in written.iter().chain(UNWRITTEN_TARGET_INDICES.iter()) {
        assert!(!seen[*index], "target {index} is in both sets");
        seen[*index] = true;
    }
    assert!(seen.iter().all(|&s| s), "some target is in neither set");
}

#[test]
fn the_attachment_set_costs_fifty_two_bytes_per_sample() {
    // Motion plus reconstruction residual increases the set to 52 bytes,
    // above WebGPU's guaranteed 32-byte floor. Keep foliage compatible with
    // the same eight attachments rather than adding another target.
    let bytes: u32 = GBUFFER_TARGET_FORMATS
        .iter()
        .map(|format| match format {
            wgpu::TextureFormat::Rgba8Unorm => 4,
            wgpu::TextureFormat::Rg16Float => 4,
            wgpu::TextureFormat::Rgba16Float => 8,
            other => panic!("unbudgeted G-buffer format {other:?}"),
        })
        .sum();
    assert_eq!(bytes, 52);
}
