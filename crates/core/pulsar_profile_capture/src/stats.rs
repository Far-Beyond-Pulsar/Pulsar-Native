//! Small statistics over `f64` samples.

/// Mean, percentiles and extremes of a set of samples, in the samples' unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    /// Summarise `samples` (any order). Empty input gives all zeros.
    pub fn of(samples: &[f64]) -> Self {
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return Self::default();
        }
        sorted.sort_by(|a, b| a.total_cmp(b));
        let sum: f64 = sorted.iter().sum();
        Self {
            count: sorted.len(),
            mean: sum / sorted.len() as f64,
            min: sorted[0],
            p50: percentile_sorted(&sorted, 50.0),
            p95: percentile_sorted(&sorted, 95.0),
            p99: percentile_sorted(&sorted, 99.0),
            max: sorted[sorted.len() - 1],
        }
    }

    /// As JSON with every figure rounded to three decimals.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "count": self.count,
            "mean": round3(self.mean),
            "min": round3(self.min),
            "p50": round3(self.p50),
            "p95": round3(self.p95),
            "p99": round3(self.p99),
            "max": round3(self.max),
        })
    }
}

/// Linear-interpolated percentile of an ascending slice (`p` in 0..=100).
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        1 => sorted[0],
        n => {
            let rank = (p / 100.0).clamp(0.0, 1.0) * (n - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
        }
    }
}

/// Round to three decimals (a microsecond when the unit is milliseconds).
pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Nanoseconds as milliseconds.
pub fn ms(ns: u64) -> f64 {
    ns as f64 / 1.0e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_interpolate() {
        let s = Summary::of(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(s.count, 5);
        assert_eq!(s.p50, 3.0);
        assert_eq!(s.max, 5.0);
        assert!((s.p95 - 4.8).abs() < 1e-9);
        assert_eq!(Summary::of(&[]).count, 0);
    }
}
