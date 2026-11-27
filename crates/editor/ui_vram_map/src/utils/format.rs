//! Byte counts and fractions as the map shows them.

pub fn bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub fn percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_and_fractions_read_as_people_write_them() {
        assert_eq!(super::bytes(512), "512 B");
        assert_eq!(super::bytes(3 * 1024 * 1024 / 2), "1.5 MiB");
        assert_eq!(super::bytes(2 * 1024 * 1024 * 1024), "2.00 GiB");
        assert_eq!(super::percent(0.256), "26%");
    }
}
