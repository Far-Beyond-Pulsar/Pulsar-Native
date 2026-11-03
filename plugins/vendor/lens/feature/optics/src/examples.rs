//! Shipped example prescriptions (embedded copies of `data/*.json`).

use crate::prescription::Prescription;

pub const COOKE_TRIPLET_JSON: &str = include_str!("../data/cooke_triplet_50mm_f5.json");
pub const DOUBLE_GAUSS_JSON: &str = include_str!("../data/double_gauss_50mm_f2.json");
pub const THIN_LENS_JSON: &str = include_str!("../data/thin_lens_50mm_f2.json");

/// Cooke triplet, 50 mm f/5.
pub fn cooke_triplet() -> Prescription {
    Prescription::from_json(COOKE_TRIPLET_JSON).expect("embedded Cooke triplet is valid")
}

/// Double-Gauss, 50 mm f/2.
pub fn double_gauss() -> Prescription {
    Prescription::from_json(DOUBLE_GAUSS_JSON).expect("embedded double Gauss is valid")
}

/// Idealized thin lens, 50 mm f/2.
pub fn thin_lens() -> Prescription {
    Prescription::from_json(THIN_LENS_JSON).expect("embedded thin lens is valid")
}

/// All shipped examples.
pub fn all() -> Vec<Prescription> {
    vec![thin_lens(), cooke_triplet(), double_gauss()]
}
