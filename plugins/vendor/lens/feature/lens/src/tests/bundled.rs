//! The bundled prescriptions, masks and iris shapes a host gets from
//! [`LensPass::with_bundled_resources`].

use super::*;

#[test]
fn the_bundled_lenses_bake_in_layer_order() {
    let started = std::time::Instant::now();
    let bakes = bundled_bakes();
    println!(
        "bundled bake ({} lenses, {}): {:?}",
        bakes.len(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        started.elapsed()
    );
    assert_eq!(bakes.len(), BundledLens::ALL.len());
    for (layer, lens) in BundledLens::ALL.into_iter().enumerate() {
        assert_eq!(lens.layer() as usize, layer);
        assert_eq!(BundledLens::from_layer(layer as u32), Some(lens));
    }
    assert_eq!(BundledLens::from_layer(BundledLens::ALL.len() as u32), None);

    // Every layer is a 50 mm lens, and the CPU rays are the bank's.
    let rig = Rig::new();
    let refs: Vec<&helio_lens_optics::LensBake> = bakes.iter().collect();
    let bank = PrescriptionBank::new(&rig.device, &refs);
    assert_eq!(bank.len(), BundledLens::ALL.len());
    for layer in 0..bank.len() {
        assert!(
            (bank.efl_mm(layer) - 50.0).abs() < 1.0,
            "layer {layer}: {} mm",
            bank.efl_mm(layer)
        );
    }
    assert_eq!(bank.rays(), bundled_rays());
    assert!(bundled_bakes_ready().is_some());
    // The widest baked apertures are the lenses' own.
    assert!((bank.min_f_number(BundledLens::ThinLens50F2 as usize) - 2.0).abs() < 0.05);
    assert!((bank.min_f_number(BundledLens::CookeTriplet50F5 as usize) - 5.0).abs() < 0.1);
}

#[test]
fn the_bundled_iris_shapes_are_distinct_shapes_inside_the_pupil() {
    let layers = bundled_aperture_layers();
    assert_eq!(layers.len(), BundledAperture::ALL.len());
    let n = APERTURE_SHAPE_SIZE as usize;
    let disc = std::f32::consts::PI * (n as f32 / 2.0).powi(2);
    let mut areas = Vec::new();
    for (shape, layer) in BundledAperture::ALL.into_iter().zip(&layers) {
        assert_eq!(layer.len(), n * n);
        let area: f32 = layer.iter().map(|&c| f32::from(c) / 255.0).sum();
        assert!(
            area > 0.25 * disc && area < 0.95 * disc,
            "{shape:?} covers {} of the pupil",
            area / disc
        );
        // Nothing in the square's corners, outside the pupil circle.
        for corner in [0, n - 1, n * (n - 1), n * n - 1] {
            assert_eq!(layer[corner], 0, "{shape:?}");
        }
        areas.push(area);
    }
    assert!(
        areas.windows(2).all(|w| (w[0] - w[1]).abs() > 0.02 * disc),
        "the shapes differ: {areas:?}"
    );
}

#[test]
fn the_bundled_mask_layer_holds_water_drops() {
    let layers = bundled_mask_layers();
    assert_eq!(layers.len(), 1);
    let layer = &layers[WATER_DROP_MASK_LAYER as usize];
    assert_eq!(layer.len(), (MASK_SIZE * MASK_SIZE * 4) as usize);
    let covered =
        layer.chunks(4).filter(|t| t[3] > 127).count() as f32 / (MASK_SIZE * MASK_SIZE) as f32;
    assert!(covered > 0.02 && covered < 0.4, "drops cover {covered}");
    // The same bytes every time: a level looks the same on every run.
    assert_eq!(layers, bundled_mask_layers());
}
