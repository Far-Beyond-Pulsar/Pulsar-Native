//! Crash-report capture helpers.

use super::*;

pub(super) fn save_crash_report(project_root: &PathBuf, stderr: &str) -> Option<PathBuf> {
    let crash_dir = project_root.join(".pulsar").join("crash-reports");
    if let Err(e) = std::fs::create_dir_all(&crash_dir) {
        tracing::warn!("[BUILD+RUN] could not create crash-reports dir: {e}");
        return None;
    }

    // Timestamp in a filename-safe format: YYYY-MM-DD_HH-MM-SS
    let ts = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Manual formatting to avoid pulling in chrono.
        let secs = now % 60;
        let mins = (now / 60) % 60;
        let hours = (now / 3600) % 24;
        let days = now / 86400;
        // Days since Unix epoch → approximate calendar date (good enough for filenames).
        let (y, m, d) = days_to_ymd(days);
        format!("{y:04}-{m:02}-{d:02}_{hours:02}-{mins:02}-{secs:02}")
    };

    let path = crash_dir.join(format!("crash_{ts}.log"));
    match std::fs::write(&path, stderr) {
        Ok(()) => {
            tracing::info!("[BUILD+RUN] crash report written to {}", path.display());
            Some(path)
        }
        Err(e) => {
            tracing::warn!("[BUILD+RUN] could not write crash report: {e}");
            None
        }
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
pub(super) fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian proleptic calendar approximation, sufficient for filenames.
    let mut year = 1970u64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for dim in &days_in_month {
        if days < *dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    (year, month, days + 1)
}