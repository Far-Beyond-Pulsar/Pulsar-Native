//! A Markdown report of everything the Memory Explorer shows, written to disk.
//!
//! One click captures the heap's large allocations (with full stacks), the GPU
//! allocator's view by name and block, the process's address space, and the
//! residency and derived-data caches, so a run can be attached to a bug,
//! compared with another, or quoted. Plain Markdown on purpose: it diffs, and
//! reads in any editor.

use crate::utils::gpu_alloc_report::GpuReport;
use crate::utils::large_allocs::LargeSnapshot;
use crate::utils::process_memory::{describe_protection, ProcessMemory};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct ReportInput<'a> {
    pub heap: &'a LargeSnapshot,
    pub gpu: &'a GpuReport,
    pub process: &'a ProcessMemory,
    pub cache: pulsar_scenedb::ResidencyStats,
    pub derived: pulsar_scenedb::DerivedStats,
    pub tracking_was_on: bool,
    /// Free text from the person exporting (what they were doing).
    pub label: &'a str,
    /// Seconds since the Unix epoch, for the heading.
    pub unix_time: u64,
}

pub fn fmt_bytes(bytes: u64) -> String {
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

/// Escape a cell so a `|` or newline in a symbol cannot break a table.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

pub fn render(input: &ReportInput) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Memory report");
    let _ = writeln!(out);
    let _ = writeln!(out, "- captured: unix time {}", input.unix_time);
    if !input.label.is_empty() {
        let _ = writeln!(out, "- note: {}", input.label);
    }
    let _ = writeln!(
        out,
        "- heap tracking: {}",
        if input.tracking_was_on {
            "on (allocations before it was enabled are not listed)"
        } else {
            "off (the Heap section is whatever was recorded earlier)"
        }
    );
    let _ = writeln!(out);

    // ── Process ─────────────────────────────────────────────────────────────
    let process = input.process;
    let _ = writeln!(out, "## Process address space");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | |\n|---|---|");
    let _ = writeln!(out, "| committed | {} |", fmt_bytes(process.committed));
    let _ = writeln!(out, "| private | {} |", fmt_bytes(process.private));
    let _ = writeln!(
        out,
        "| write-combined (GPU-visible) | {} |",
        fmt_bytes(process.write_combined)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| type · protection | regions | size |\n|---|---:|---:|");
    for total in process.totals.iter().take(16) {
        let _ = writeln!(
            out,
            "| {} · {} | {} | {} |",
            total.kind.label(),
            cell(&total.protection),
            total.regions,
            fmt_bytes(total.bytes)
        );
    }
    if !process.largest.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Largest regions:");
        let _ = writeln!(out);
        let _ = writeln!(out, "| base | kind | protection | size |\n|---|---|---|---:|");
        for region in process.largest.iter().take(20) {
            let _ = writeln!(
                out,
                "| `{:#x}` | {} | {} | {} |",
                region.base,
                region.kind.label(),
                cell(&describe_protection(region.protect)),
                fmt_bytes(region.bytes)
            );
        }
    }
    let _ = writeln!(out);

    // ── GPU ─────────────────────────────────────────────────────────────────
    let _ = writeln!(out, "## GPU allocations (wgpu allocator report)");
    let _ = writeln!(out);
    if input.gpu.devices.is_empty() {
        let _ = writeln!(out, "_No engine device reported._");
    }
    for device in &input.gpu.devices {
        let _ = writeln!(
            out,
            "### Device #{} — allocated {}, reserved {}, {} blocks",
            device.index,
            fmt_bytes(device.allocated),
            fmt_bytes(device.reserved),
            device.blocks.len()
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "| allocation name | count | largest | total |\n|---|---:|---:|---:|");
        for group in device.groups.iter().take(30) {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                cell(&group.name),
                group.count,
                fmt_bytes(group.largest),
                fmt_bytes(group.bytes)
            );
        }
        let _ = writeln!(out);
    }

    // ── Heap ────────────────────────────────────────────────────────────────
    let heap = input.heap;
    let _ = writeln!(out, "## Heap: live allocations of {} or more", fmt_bytes(heap.threshold as u64));
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} blocks live, {} total (peak {}); {} allocated, {} freed since tracking began.",
        heap.live_count,
        fmt_bytes(heap.live_bytes),
        fmt_bytes(heap.peak_bytes),
        heap.total_allocs,
        heap.total_frees
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| attributed frame | count | largest | live |\n|---|---:|---:|---:|");
    for group in heap.groups.iter().take(30) {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            cell(&group.label),
            group.count,
            fmt_bytes(group.largest),
            fmt_bytes(group.live_bytes)
        );
    }
    for group in heap.groups.iter().take(5) {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "<details><summary>{} — {} × up to {}</summary>\n",
            cell(&group.label),
            group.count,
            fmt_bytes(group.largest)
        );
        let _ = writeln!(out, "```");
        for (index, frame) in group.stack.iter().enumerate() {
            let _ = writeln!(out, "{index:>2}  {frame}");
        }
        let _ = writeln!(out, "```\n\n</details>");
    }
    let _ = writeln!(out);

    // ── Residency cache ─────────────────────────────────────────────────────
    let cache = &input.cache;
    let _ = writeln!(out, "## Rebuildable-data cache (SceneDB residency)");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | |\n|---|---|");
    let _ = writeln!(out, "| budget | {} |", fmt_bytes(cache.budget as u64));
    let _ = writeln!(
        out,
        "| resident | {} in {} values |",
        fmt_bytes(cache.resident_bytes as u64),
        cache.resident_values
    );
    let _ = writeln!(out, "| evictions | {} |", cache.evictions);
    let _ = writeln!(out, "| rebuilds | {} |", cache.rebuilds);
    let _ = writeln!(out, "| prefetched | {} |", cache.prefetches);
    let _ = writeln!(out, "| joined an in-flight rebuild | {} |", cache.joined);
    let _ = writeln!(out);

    // ── Derived-data cache ──────────────────────────────────────────────────
    let derived = &input.derived;
    let _ = writeln!(out, "## Derived-data cache (SceneDB, on disk)");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | |\n|---|---|");
    let _ = writeln!(out, "| hits (read from a file) | {} |", derived.hits);
    let _ = writeln!(out, "| misses (cooked) | {} |", derived.misses);
    let _ = writeln!(out, "| read | {} |", fmt_bytes(derived.bytes_read));
    let _ = writeln!(out, "| written | {} |", fmt_bytes(derived.bytes_written));
    let _ = writeln!(out, "| file errors | {} |", derived.errors);
    out
}

/// Where reports go: `<project>/.pulsar/profiling/memory`.
pub fn report_dir(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join("profiling").join("memory")
}

/// Write `markdown` as a new report under `project_root`. Returns the path.
pub fn write(project_root: &Path, unix_time: u64, markdown: &str) -> std::io::Result<PathBuf> {
    let dir = report_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("memory_report_{unix_time}.md"));
    std::fs::write(&path, markdown)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::gpu_alloc_report::{GpuDeviceReport, GpuNameGroup};
    use crate::utils::large_allocs::LargeGroup;

    const MB: u64 = 1024 * 1024;

    fn sample() -> String {
        let heap = LargeSnapshot {
            groups: vec![LargeGroup {
                key: 1,
                label: "helio_component::texture_assets::decode_rgba | odd".into(),
                stack: vec!["alloc::vec::Vec::resize".into(), "helio_component::decode_rgba".into()],
                count: 3,
                live_bytes: 48 * MB,
                largest: 16 * MB,
                oldest_secs: 5.0,
                threads: vec![1],
            }],
            live_count: 3,
            live_bytes: 48 * MB,
            peak_bytes: 64 * MB,
            total_allocs: 9,
            total_frees: 6,
            threshold: (16 * MB) as usize,
        };
        let gpu = GpuReport {
            devices: vec![GpuDeviceReport {
                index: 0,
                allocated: 100 * MB,
                reserved: 256 * MB,
                blocks: vec![],
                groups: vec![GpuNameGroup { name: "Scene Texture".into(), count: 6, bytes: 96 * MB, largest: 16 * MB }],
            }],
            unsupported: 0,
        };
        render(&ReportInput {
            heap: &heap,
            gpu: &gpu,
            process: &ProcessMemory::default(),
            cache: pulsar_scenedb::ResidencyStats {
                budget: (1024 * MB) as usize,
                resident_bytes: (256 * MB) as usize,
                resident_values: 16,
                evictions: 2,
                rebuilds: 18,
                prefetches: 5,
                joined: 1,
            },
            derived: pulsar_scenedb::DerivedStats {
                hits: 40,
                misses: 8,
                bytes_read: 200 * MB,
                bytes_written: 40 * MB,
                errors: 0,
            },
            tracking_was_on: true,
            label: "after loading bazzar",
            unix_time: 1_700_000_000,
        })
    }

    #[test]
    fn the_report_has_every_section_with_numbers() {
        let report = sample();
        for heading in [
            "# Memory report",
            "## Process address space",
            "## GPU allocations",
            "## Heap: live allocations of 16.0 MiB or more",
            "## Rebuildable-data cache",
            "## Derived-data cache",
        ] {
            assert!(report.contains(heading), "missing {heading}\n{report}");
        }
        assert!(report.contains("| Scene Texture | 6 | 16.0 MiB | 96.0 MiB |"));
        assert!(report.contains("| rebuilds | 18 |"));
        assert!(report.contains("| prefetched | 5 |"));
        assert!(report.contains("| hits (read from a file) | 40 |"));
        assert!(report.contains("| misses (cooked) | 8 |"));
        assert!(report.contains("note: after loading bazzar"));
    }

    #[test]
    fn a_pipe_in_a_symbol_cannot_break_a_table_row() {
        let report = sample();
        assert!(report.contains("decode_rgba \\| odd"));
    }

    #[test]
    fn reports_are_written_under_the_project_profiling_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), 42, "hello").unwrap();
        assert!(path.ends_with("memory_report_42.md"));
        assert!(path.starts_with(report_dir(dir.path())));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "hello");
    }
}
