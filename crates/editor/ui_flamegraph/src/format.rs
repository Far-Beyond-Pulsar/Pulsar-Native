//! Number formatting shared by the panels.

/// A duration with a unit that keeps three or four significant digits.
pub fn format_duration(ns: u64) -> String {
    if ns < 1_000 {
        format!("{} ns", ns)
    } else if ns < 1_000_000 {
        format!("{:.2} µs", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.2} s", ns as f64 / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn picks_a_readable_unit() {
        assert_eq!(format_duration(950), "950 ns");
        assert_eq!(format_duration(12_500), "12.50 µs");
        assert_eq!(format_duration(3_250_000), "3.25 ms");
        assert_eq!(format_duration(2_000_000_000), "2.00 s");
    }
}
