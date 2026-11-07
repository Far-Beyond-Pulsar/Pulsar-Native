//! Diagnostic: how many root (pinned) pages and total pages each cached `.vgeo`
//! has. Run with
//! `VG_CACHE_DIR=<dir> cargo test -p helio-vg-stream --test cache_roots -- --ignored --nocapture`.
//! Reads only the 72-byte header of every file.

use helio_vg_format::{VgeoHeader, PAGE_BYTES};
use std::{fs::File, io::Read, path::PathBuf};

#[test]
#[ignore = "diagnostic: set VG_CACHE_DIR"]
fn cached_assets_root_and_total_pages() {
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(PathBuf::from) else {
        eprintln!("set VG_CACHE_DIR");
        return;
    };
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("vgeo") {
            continue;
        }
        let mut header = [0u8; std::mem::size_of::<VgeoHeader>()];
        if File::open(&path)
            .and_then(|mut f| f.read_exact(&mut header))
            .is_err()
        {
            continue;
        }
        let h: VgeoHeader = *bytemuck::from_bytes(&header);
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        rows.push((
            h.asset.root_page_count,
            h.asset.page_count,
            h.asset.triangle_count,
            h.version,
            size,
            path,
        ));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let (mut roots, mut pages, mut bytes) = (0u64, 0u64, 0u64);
    println!(
        "{:>8} {:>8} {:>7} {:>11} {:>4} {:>8}  file",
        "roots", "pages", "root%", "tris", "ver", "MiB"
    );
    for (r, p, t, v, size, path) in &rows {
        roots += u64::from(*r);
        pages += u64::from(*p);
        bytes += size;
        println!(
            "{:>8} {:>8} {:>6.0}% {:>11} {:>4} {:>8.1}  {}",
            r,
            p,
            100.0 * f64::from(*r) / f64::from((*p).max(1)),
            t,
            v,
            *size as f64 / 1048576.0,
            path.file_name().unwrap().to_string_lossy()
        );
    }
    println!(
        "TOTAL roots {roots} pages ({:.0} MiB pinned of a 512 MiB pool), pages {pages} ({:.0} MiB), cache {:.0} MiB",
        roots as f64 * f64::from(PAGE_BYTES) / 1048576.0,
        pages as f64 * f64::from(PAGE_BYTES) / 1048576.0,
        bytes as f64 / 1048576.0
    );
}

/// The file-backed reader must return exactly what the in-memory one does.
/// Compares the five smallest cached files (all sections and every page).
#[test]
#[ignore = "diagnostic: set VG_CACHE_DIR"]
fn file_backed_reader_matches_memory_reader() {
    use helio_vg_stream::{VgeoBytes, VgeoSource};
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(PathBuf::from) else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("vgeo"))
        .map(|e| (e.metadata().unwrap().len(), e.path()))
        .collect();
    files.sort();
    for (_, path) in files.into_iter().take(5) {
        let mem = VgeoBytes::new(std::fs::read(&path).unwrap()).unwrap();
        let disk = VgeoBytes::open(&path).unwrap();
        assert_eq!(mem.header(), disk.header());
        assert_eq!(mem.hierarchy().unwrap(), disk.hierarchy().unwrap());
        assert_eq!(mem.group_refs().unwrap(), disk.group_refs().unwrap());
        assert_eq!(mem.page_deps().unwrap(), disk.page_deps().unwrap());
        assert_eq!(mem.quant().step, disk.quant().step);
        for p in 0..mem.header().asset.page_count {
            assert_eq!(
                mem.read_page(p).unwrap(),
                disk.read_page(p).unwrap(),
                "page {p} of {}",
                path.display()
            );
        }
        println!("ok {}", path.file_name().unwrap().to_string_lossy());
    }
}

/// Which cached assets have all their root clusters in one hierarchy leaf (the
/// precondition of the cull's per-instance root early-out), and how many roots.
#[test]
#[ignore = "diagnostic: set VG_CACHE_DIR"]
fn cached_assets_single_root_leaf() {
    use helio_vg_stream::{VgeoBytes, VgeoSource};
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(PathBuf::from) else {
        return;
    };
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        if e.path().extension().and_then(|x| x.to_str()) != Some("vgeo")
            || e.metadata().unwrap().len() > 2_000_000_000
        {
            continue;
        }
        let src = VgeoBytes::open(&e.path()).unwrap();
        let (nodes, refs) = (src.hierarchy().unwrap(), src.group_refs().unwrap());
        let root = src.header().asset.root_node;
        let leaf = helio_vg_format::root_leaves(&nodes, root);
        let count = leaf.as_ref().map(|l| {
            l.iter()
                .map(|&x| refs[x as usize].cluster_count())
                .collect::<Vec<_>>()
        });
        println!(
            "{} tris {:>10} nodes {:>6} single_root_leaf {:?} roots {:?}",
            e.file_name().to_string_lossy(),
            src.header().asset.triangle_count,
            nodes.len(),
            leaf,
            count
        );
    }
}
