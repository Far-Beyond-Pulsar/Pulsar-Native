//! Spectral sampling and per-wavelength RGB reconstruction weights.

use crate::glass::D_LINE_NM;

/// Lower/upper bound of the sampled visible band.
pub const BAND_NM: (f64, f64) = (400.0, 700.0);

/// Smooth positive RGB channel sensitivities (Gaussians; peak, sigma in nm).
const CHANNELS: [(f64, f64); 3] = [(600.0, 45.0), (545.0, 40.0), (455.0, 28.0)];

/// Relative sensitivity of the R, G, B channels at `wavelength_nm`.
pub fn rgb_response(wavelength_nm: f64) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (o, (mu, sigma)) in out.iter_mut().zip(CHANNELS) {
        let z = (wavelength_nm - mu) / sigma;
        *o = (-0.5 * z * z).exp();
    }
    out
}

/// A set of sample wavelengths and, per wavelength, how much of it feeds each
/// output channel.
///
/// A scene channel `c` is imaged once per wavelength `k` (each with its own
/// ray map) and recombined as `out_c = sum_k weights[k][c] * image_k_c`. The
/// weights are normalized so `sum_k weights[k][c] == 1` for every channel: a
/// lens with no chromatic aberration (all `image_k` identical) reproduces the
/// scene exactly, so white stays white.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectralSampling {
    pub wavelengths_nm: Vec<f64>,
    pub weights: Vec<[f32; 3]>,
}

impl SpectralSampling {
    /// `n` bin-centred samples uniformly spanning 400-700 nm (`n = 3` gives
    /// 450, 550, 650 nm).
    pub fn uniform(n: usize) -> Self {
        let n = n.max(1);
        let w = (0..n)
            .map(|k| BAND_NM.0 + (k as f64 + 0.5) * (BAND_NM.1 - BAND_NM.0) / n as f64)
            .collect();
        Self::with_wavelengths(w)
    }

    /// Weights for arbitrary sample wavelengths.
    pub fn with_wavelengths(wavelengths_nm: Vec<f64>) -> Self {
        let n = wavelengths_nm.len().max(1);
        let resp: Vec<[f64; 3]> = wavelengths_nm.iter().map(|w| rgb_response(*w)).collect();
        let mut sum = [0.0; 3];
        for r in &resp {
            for c in 0..3 {
                sum[c] += r[c];
            }
        }
        let weights = resp
            .iter()
            .map(|r| {
                let mut o = [0.0f32; 3];
                for c in 0..3 {
                    // A channel with no spectral support falls back to a box.
                    o[c] = if sum[c] > 1e-12 {
                        (r[c] / sum[c]) as f32
                    } else {
                        1.0 / n as f32
                    };
                }
                o
            })
            .collect();
        SpectralSampling {
            wavelengths_nm,
            weights,
        }
    }

    pub fn len(&self) -> usize {
        self.wavelengths_nm.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wavelengths_nm.is_empty()
    }

    /// Per-channel sum of the weights (1.0 up to f32 rounding).
    pub fn channel_sums(&self) -> [f64; 3] {
        let mut s = [0.0; 3];
        for w in &self.weights {
            for c in 0..3 {
                s[c] += w[c] as f64;
            }
        }
        s
    }

    /// Recombine one RGB sample per wavelength into the output colour.
    pub fn reconstruct(&self, samples: &[[f32; 3]]) -> [f32; 3] {
        let mut o = [0.0; 3];
        for (w, s) in self.weights.iter().zip(samples) {
            for c in 0..3 {
                o[c] += w[c] * s[c];
            }
        }
        o
    }

    /// Index of the sample wavelength closest to the d line.
    pub fn reference_index(&self) -> usize {
        self.wavelengths_nm
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - D_LINE_NM).abs().total_cmp(&(b.1 - D_LINE_NM).abs()))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

impl Default for SpectralSampling {
    fn default() -> Self {
        Self::uniform(7)
    }
}
