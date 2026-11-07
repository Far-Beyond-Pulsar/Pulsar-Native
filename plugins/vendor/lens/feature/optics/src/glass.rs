//! Refractive index models and a small catalog of common optical glasses.

use serde::{Deserialize, Serialize};

/// Fraunhofer d line (helium), the reference wavelength for `nd`.
pub const D_LINE_NM: f64 = 587.5618;
/// Fraunhofer F line (hydrogen).
pub const F_LINE_NM: f64 = 486.1327;
/// Fraunhofer C line (hydrogen).
pub const C_LINE_NM: f64 = 656.2725;

/// Medium that follows a surface.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Glass {
    /// Air, n = 1 at every wavelength.
    #[default]
    Air,
    /// Non-dispersive constant index.
    Index(f64),
    /// Two-term Cauchy model fitted to `nd` at the d line and Abbe number
    /// `vd = (nd - 1) / (nF - nC)`. Convenient when only catalog headline
    /// numbers are known.
    Abbe { nd: f64, vd: f64 },
    /// Sellmeier equation, wavelength in micrometres:
    /// `n^2 = 1 + sum B_i l^2 / (l^2 - C_i)`.
    Sellmeier { b: [f64; 3], c: [f64; 3] },
    /// Name looked up in [`catalog_glass`] (case-insensitive).
    Catalog(String),
}

impl Glass {
    /// Refractive index at `wavelength_nm`, or `None` for an unknown catalog
    /// name or an invalid model.
    pub fn index_at(&self, wavelength_nm: f64) -> Option<f64> {
        let um = wavelength_nm * 1e-3;
        match self {
            Glass::Air => Some(1.0),
            Glass::Index(n) => (*n >= 1.0).then_some(*n),
            Glass::Abbe { nd, vd } => {
                if *vd <= 0.0 || *nd < 1.0 {
                    return None;
                }
                let (ud, uf, uc) = (D_LINE_NM * 1e-3, F_LINE_NM * 1e-3, C_LINE_NM * 1e-3);
                let b = (nd - 1.0) / vd / (1.0 / (uf * uf) - 1.0 / (uc * uc));
                let a = nd - b / (ud * ud);
                Some(a + b / (um * um))
            }
            Glass::Sellmeier { b, c } => {
                let l2 = um * um;
                let mut n2 = 1.0;
                for i in 0..3 {
                    n2 += b[i] * l2 / (l2 - c[i]);
                }
                (n2 > 0.0).then(|| n2.sqrt())
            }
            Glass::Catalog(name) => catalog_glass(name)?.index_at(wavelength_nm),
        }
    }

    /// Index at the d line.
    pub fn nd(&self) -> Option<f64> {
        self.index_at(D_LINE_NM)
    }

    /// Abbe number `(nd - 1) / (nF - nC)`; infinite for non-dispersive media.
    pub fn abbe(&self) -> Option<f64> {
        let nd = self.nd()?;
        let dn = self.index_at(F_LINE_NM)? - self.index_at(C_LINE_NM)?;
        Some(if dn.abs() < 1e-15 {
            f64::INFINITY
        } else {
            (nd - 1.0) / dn
        })
    }
}

/// (name, B1..B3, C1..C3 in um^2) from Schott / refractiveindex.info datasheets.
const CATALOG: &[(&str, [f64; 3], [f64; 3])] = &[
    (
        "N-BK7",
        [1.03961212, 0.231792344, 1.01046945],
        [0.00600069867, 0.0200179144, 103.560653],
    ),
    (
        "N-SF6",
        [1.77931763, 0.338149866, 2.08734474],
        [0.0133714182, 0.0617533621, 174.01759],
    ),
    (
        "N-LAK9",
        [1.46231905, 0.344399589, 1.15508372],
        [0.00724270156, 0.0243353131, 85.4686868],
    ),
    (
        "F2",
        [1.34533359, 0.209073176, 0.937357162],
        [0.00997743871, 0.0470450767, 111.886764],
    ),
    (
        "N-SK16",
        [1.34317774, 0.241144399, 0.994317969],
        [0.00704687339, 0.0229005, 92.7508526],
    ),
    (
        // Malitson 1965.
        "FUSED-SILICA",
        [0.6961663, 0.4079426, 0.8974794],
        [0.00467914826, 0.0135120631, 97.9340025],
    ),
];

/// Look up a catalog glass by name (case-insensitive). `"AIR"` yields
/// [`Glass::Air`].
pub fn catalog_glass(name: &str) -> Option<Glass> {
    if name.eq_ignore_ascii_case("air") {
        return Some(Glass::Air);
    }
    CATALOG
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, b, c)| Glass::Sellmeier { b: *b, c: *c })
}

/// Names of all catalog glasses (excluding air).
pub fn catalog_names() -> impl Iterator<Item = &'static str> {
    CATALOG.iter().map(|(n, _, _)| *n)
}
