//! GPU engine utilization polling via Windows PDH performance counters.
//!
//! Reads `\GPU Engine(*)\Utilization Percentage` and groups results by engine
//! type (3D, Copy, VideoDecode, VideoEncode, Compute, etc.) — the same data
//! source that Windows Task Manager uses.

use std::collections::HashMap;

/// Per-engine GPU utilization snapshot, keyed by engine type name.
pub type EngineMap = HashMap<String, f64>;

/// Returns the latest engine utilization percentages, or an empty map if
/// PDH is unavailable (non-Windows, or counters not present).
pub fn collect() -> EngineMap {
    #[cfg(target_os = "windows")]
    return windows_impl::collect();
    #[cfg(not(target_os = "windows"))]
    return HashMap::new();
}

/// Known GPU engine type display names (for consistent ordering in UI).
pub const KNOWN_ENGINES: &[&str] = &[
    "3D", "Copy", "VideoDecode", "VideoEncode", "VideoProcessing", "Compute_0", "Compute_1", "Overlay",
];

// ─── Windows implementation ───────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::EngineMap;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhOpenQueryW,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
        PDH_HCOUNTER, PDH_HQUERY,
    };

    struct PdhState {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
    }

    // Safety: PDH handles are accessed only from the single background metrics thread.
    unsafe impl Send for PdhState {}

    static PDH: Mutex<Option<PdhState>> = Mutex::new(None);

    fn ensure_init() -> bool {
        let mut guard = PDH.lock().unwrap();
        if guard.is_some() {
            return true;
        }

        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return false;
            }

            let path = windows::core::w!(r"\GPU Engine(*)\Utilization Percentage");
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return false;
            }

            // First collect — rate counters need 2 samples to produce values
            PdhCollectQueryData(query);

            *guard = Some(PdhState { query, counter });
        }
        true
    }

    pub fn collect() -> EngineMap {
        if !ensure_init() {
            return HashMap::new();
        }

        let guard = PDH.lock().unwrap();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return HashMap::new(),
        };

        unsafe {
            // Second sample
            PdhCollectQueryData(state.query);

            let mut buf_size: u32 = 0;
            let mut item_count: u32 = 0;

            // First call: get required buffer size
            PdhGetFormattedCounterArrayW(
                state.counter,
                PDH_FMT_DOUBLE,
                &mut buf_size,
                &mut item_count,
                None,
            );

            if buf_size == 0 || item_count == 0 {
                return HashMap::new();
            }

            let mut buf = vec![0u8; buf_size as usize];
            let status = PdhGetFormattedCounterArrayW(
                state.counter,
                PDH_FMT_DOUBLE,
                &mut buf_size,
                &mut item_count,
                Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
            );

            if status != 0 {
                return HashMap::new();
            }

            let items = buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W;
            let mut acc: HashMap<String, (f64, u32)> = HashMap::new();

            for i in 0..item_count as usize {
                let item = &*items.add(i);
                if let Ok(name) = item.szName.to_string() {
                    if let Some(eng) = extract_engine_type(&name) {
                        let val = item.FmtValue.Anonymous.doubleValue;
                        if val.is_finite() {
                            let e = acc.entry(eng).or_insert((0.0, 0));
                            e.0 += val;
                            e.1 += 1;
                        }
                    }
                }
            }

            acc.into_iter()
                .map(|(k, (sum, _n))| (k, sum.min(100.0)))
                .collect()
        }
    }

    fn extract_engine_type(instance: &str) -> Option<String> {
        instance.rfind("engtype_").map(|pos| instance[pos + 8..].to_string())
    }
}
