//! The near band's L0/L1 switch.
//!
//! Inside the switch band a tuft is emitted at both levels and draws at exactly one, by
//! its hash: the share at L1 grows across the band, and no tuft is ever drawn twice or
//! not at all (a thinning or doubling ring), nor as two see-through halves. Past L1 the
//! bands fade whole tufts (`contract::bands`).

use helio_pass_foliage_gbuffer::lod_draws;

#[test]
fn a_tuft_draws_at_exactly_one_level_across_the_band() {
    let (boundary, band) = (6.0, 4.0);
    let mut d = 0.0;
    while d < 12.0 {
        for k in 0..64 {
            let unit = k as f32 / 64.0;
            let drawn = u32::from(lod_draws(0, d, boundary, band, unit))
                + u32::from(lod_draws(1, d, boundary, band, unit));
            assert_eq!(drawn, 1, "tuft {unit} drawn {drawn} times at {d} m");
        }
        d += 0.13;
    }
}

#[test]
fn the_share_at_l1_grows_across_the_band() {
    let share = |d: f32| {
        (0..1000)
            .filter(|&k| lod_draws(1, d, 6.0, 4.0, k as f32 / 1000.0))
            .count()
    };
    assert_eq!(share(1.0), 0);
    assert!(share(3.0) < share(4.0) && share(4.0) < share(5.0));
    assert_eq!(share(7.0), 1000);
    // Tufts past L1 are faded by their bands, not this.
    assert!(lod_draws(2, 3.0, 6.0, 4.0, 0.5));
}

#[test]
fn a_zero_width_band_degrades_to_a_hard_switch() {
    assert!(lod_draws(0, 5.9, 6.0, 0.0, 0.99));
    assert!(!lod_draws(0, 6.0, 6.0, 0.0, 0.0));
    assert!(lod_draws(1, 6.1, 6.0, 0.0, 0.0));
}
