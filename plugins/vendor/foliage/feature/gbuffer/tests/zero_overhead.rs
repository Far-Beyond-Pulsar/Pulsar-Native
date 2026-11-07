//! The plan's §10 zero-overhead guarantee, asserted without a GPU.
//!
//! Guarantee 2: "passes present but no foliage types registered ⇒ `prepare()`
//! early-returns on an unwritten `frame.foliage` slot, `execute()` records no commands,
//! `declare_resources` allocates nothing."
//!
//! This is exactly the kind of property that survives a refactor in spirit and dies in
//! practice: the cost of breaking it is a couple of constant-size buffer writes and four
//! empty draws, which nobody notices in a profile and nobody's eye catches in review.
//! `decide_frame` exists as a pure function so the guarantee has somewhere to be
//! asserted that does not need an adapter.

use helio_pass_foliage_gbuffer::{decide_frame, FoliageTables};

#[test]
fn an_unwritten_foliage_slot_costs_nothing_at_all() {
    let decision = decide_frame(None, None);
    assert!(!decision.enabled);
    assert!(!decision.upload_types, "no table upload");
    assert!(!decision.upload_wind, "not even the 48-byte wind uniform");
    assert_eq!(decision.draw_count, 0, "not even four empty draws");
}

#[test]
fn a_published_but_empty_type_table_also_costs_nothing() {
    // A blade's type id indexes the descriptor table directly, so an empty table means
    // no blade can resolve to anything. Recording draws against it would be four
    // guaranteed-empty draw calls plus a per-frame wind upload for no possible pixel.
    let decision = decide_frame(
        Some(FoliageTables {
            type_count: 0,
            generation: 7,
        }),
        None,
    );
    assert!(!decision.enabled);
    assert!(!decision.upload_types);
    assert!(!decision.upload_wind);
    assert_eq!(decision.draw_count, 0);
}

#[test]
fn a_stale_decision_does_not_leak_across_the_disable() {
    // The pass caches its decision between `prepare` and `execute`. Turning foliage off
    // must produce a decision that stops `execute` — not merely a decision that uploads
    // nothing, which would leave the previous frame's four draws recording against
    // whatever the indirect buffer still holds.
    let enabled = decide_frame(
        Some(FoliageTables {
            type_count: 3,
            generation: 1,
        }),
        None,
    );
    assert!(enabled.enabled);
    let disabled = decide_frame(None, Some(1));
    assert!(!disabled.enabled);
    assert_eq!(disabled.draw_count, 0);
}

#[test]
fn registered_types_always_produce_exactly_four_draws() {
    // Guarantee 3: an empty ring is four `draw_indirect` calls with
    // `instance_count == 0`, not zero draws. The pass cannot know the ring is empty —
    // that count lives on the GPU — and must not try to.
    let decision = decide_frame(
        Some(FoliageTables {
            type_count: 1,
            generation: 0,
        }),
        None,
    );
    assert!(decision.enabled);
    assert_eq!(decision.draw_count, 4);
}

#[test]
fn the_type_table_uploads_only_when_the_generation_moves() {
    // Wind deliberately does not advance the foliage type-table generation, so a
    // steady-state frame must re-upload nothing but the 48-byte wind uniform. If this
    // ever inverts, the residency cache's whole point — that steady-state foliage costs
    // nothing on the CPU — is lost to a 24 KiB per-frame copy.
    let tables = FoliageTables {
        type_count: 4,
        generation: 12,
    };

    let first = decide_frame(Some(tables), None);
    assert!(
        first.upload_types,
        "the first frame has nothing uploaded yet"
    );

    let steady = decide_frame(Some(tables), Some(12));
    assert!(
        !steady.upload_types,
        "an unchanged generation must not re-upload"
    );
    assert!(
        steady.upload_wind,
        "but wind moves every frame — both timestamps do"
    );

    let edited = decide_frame(
        Some(FoliageTables {
            generation: 13,
            ..tables
        }),
        Some(12),
    );
    assert!(edited.upload_types, "an authoring edit must re-upload");
}
