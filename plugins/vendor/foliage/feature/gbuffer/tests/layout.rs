//! Layout and interface-contract asserts.
//!
//! Everything here is a number that appears in two places — a Rust struct and a WGSL
//! struct, or this pass and `FoliagePlacePass` — with nothing at runtime to notice when
//! the two drift. A wrong uniform size does not error; the shader reads every field
//! after the mistake from the wrong offset and the grass comes out with garbage
//! dimensions, which is a long way from "someone added a `u32`".

use helio_pass_foliage_gbuffer::*;

// ── Uniform layouts ───────────────────────────────────────────────────────────

#[test]
fn foliage_globals_is_forty_eight_bytes() {
    // Mirrored by `struct FoliageGlobals` in foliage_gbuffer.wgsl. 48 is also a multiple
    // of 16, which a uniform-address-space struct has to be.
    assert_eq!(std::mem::size_of::<FoliageGlobals>(), 48);
    assert_eq!(std::mem::size_of::<FoliageGlobals>() % 16, 0);
}

#[test]
fn foliage_globals_field_offsets_match_the_wgsl_mirror() {
    assert_eq!(std::mem::offset_of!(FoliageGlobals, screen_size), 0);
    assert_eq!(std::mem::offset_of!(FoliageGlobals, frame), 8);
    assert_eq!(std::mem::offset_of!(FoliageGlobals, flags), 12);
    // The vec4s must land on 16-byte boundaries or WGSL inserts padding the Rust
    // side does not have.
    assert_eq!(std::mem::offset_of!(FoliageGlobals, clock), 16);
    assert_eq!(std::mem::offset_of!(FoliageGlobals, lod_quality_scale), 32);
    assert_eq!(std::mem::offset_of!(FoliageGlobals, lod_fade_band), 36);
    assert_eq!(
        std::mem::offset_of!(FoliageGlobals, interaction_strength),
        40
    );
}

#[test]
fn lod_uniform_is_thirty_two_bytes_and_fits_its_stride() {
    assert_eq!(std::mem::size_of::<FoliageLodUniform>(), 32);
    // The dynamic offset must be a multiple of `min_uniform_buffer_offset_alignment`,
    // which wgpu caps at 256. Using the cap is valid on every adapter; using the struct
    // size would not be.
    assert_eq!(LOD_UNIFORM_STRIDE, 256);
    assert!(std::mem::size_of::<FoliageLodUniform>() as u32 <= LOD_UNIFORM_STRIDE);
    assert_eq!(LOD_UNIFORM_STRIDE % 256, 0);
}

#[test]
fn draw_indirect_stride_matches_draw_indirect_args() {
    // `DrawIndirectArgs` is four u32: vertex_count, instance_count, first_vertex,
    // first_instance. The four LOD commands therefore sit at 0/16/32/48.
    assert_eq!(DRAW_INDIRECT_STRIDE, 16);
    let offsets: Vec<u64> = (0..4)
        .map(|lod| lod as u64 * DRAW_INDIRECT_STRIDE)
        .collect();
    assert_eq!(offsets, vec![0, 16, 32, 48]);
}

// ── "Vertex layout": there isn't one ──────────────────────────────────────────

#[test]
fn the_ladder_has_no_vertex_or_index_buffer_and_the_counts_are_pinned() {
    // Smooth foliage is four non-indexed instanced strip draws. These vertex counts are
    // what `FoliagePlacePass` writes into the indirect buffer (blocky foliage is drawn
    // as boxes, a fifth draw): L0 and L1 a blade strip with a
    // collapsed tip, L2 a tuft's blades as cards (four vertices and two joins each), L3 a
    // fan of three blades (three vertices and two joins each).
    assert_eq!(LOD_VERTEX_COUNTS, [11, 7, 88, 13]);
    assert_eq!(LOD_SEGMENTS, [5, 3, 1, 1]);
    for lod in 0..2 {
        assert_eq!(LOD_VERTEX_COUNTS[lod], 2 * LOD_SEGMENTS[lod] + 1);
    }
    assert_eq!(
        LOD_VERTEX_COUNTS[2],
        6 * helio_pass_foliage_place::FOLIAGE_MAX_TUFT_BLADES - 2
    );
    assert_eq!(LOD_VERTEX_COUNTS[3], 3 * 5 - 2);
}

#[test]
fn lod_uniforms_carry_the_ladder_verbatim() {
    let stride = 4096;
    let uniforms = FoliageGBufferPass::lod_uniforms(stride, 16);
    for (lod, uniform) in uniforms.iter().enumerate() {
        assert_eq!(uniform.lod, lod as u32);
        assert_eq!(uniform.segments, LOD_SEGMENTS[lod]);
        assert_eq!(uniform.vertex_count, LOD_VERTEX_COUNTS[lod]);
        assert_eq!(uniform.region_base, lod as u32 * stride);
        // Reserved words must be zero so a future build can tell "unset" from
        // "predates the field".
        assert_eq!(uniform._pad, 0);
    }
}

// ── `visible_blades` entry encoding ───────────────────────────────────────────

#[test]
fn visible_blade_halves_do_not_bleed_into_each_other() {
    // This is the failure that draws grass from a neighbouring tile at a
    // plausible-looking position — no crash, no error, just a field that is subtly wrong
    // and does not move when you edit the tile it appears to belong to.
    assert_eq!(pack_visible_blade(0xffff, 0x0000), 0xffff_0000);
    assert_eq!(pack_visible_blade(0x0000, 0xffff), 0x0000_ffff);
    assert_eq!(VISIBLE_TILE_SHIFT, 16);
    assert_eq!(VISIBLE_LOCAL_MASK, 0xffff);

    for tile_slot in [0u32, 1, 4095, 4096, 65535] {
        for local in [0u32, 1, 2559, 65535] {
            assert_eq!(
                unpack_visible_blade(pack_visible_blade(tile_slot, local)),
                (tile_slot, local)
            );
        }
    }
}

#[test]
fn visible_blade_halves_cover_every_band() {
    // 16 bits each has to be enough for the bands' slots and their slabs, or the encoding
    // silently aliases.
    let ceiling = VISIBLE_LOCAL_MASK;
    for quality in [
        helio_pass_foliage_place::FoliageQuality::Medium,
        helio_pass_foliage_place::FoliageQuality::Ultra,
    ] {
        let bands = helio_pass_foliage_place::plan_bands(&helio_pass_foliage_place::FoliageRange {
            range: quality.range(),
            tufts_per_m2: 12.0,
            ..Default::default()
        });
        assert!(bands.slots() <= ceiling);
        for band in bands.bands() {
            assert!(
                band.slab <= ceiling + 1,
                "{} records a slab exceeds the 16-bit local index",
                band.slab
            );
        }
    }
}

#[test]
fn region_offsets_are_contiguous_in_lod_order() {
    for stride in [1u32, 64, 4096, 262_144] {
        let offsets: Vec<u32> = (0..4)
            .map(|lod| visible_region_offset(lod, stride))
            .collect();
        assert_eq!(offsets, vec![0, stride, 2 * stride, 3 * stride]);
        // Strictly increasing, so no two LODs can read each other's region.
        assert!(offsets.windows(2).all(|w| w[1] > w[0]));
    }
}
