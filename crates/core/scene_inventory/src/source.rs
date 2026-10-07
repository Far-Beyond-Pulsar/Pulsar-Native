//! Source-tree facts: lifecycle call sites, declared world components, GPU
//! buffer producers/consumers and render pass crates.
//!
//! Production code only. Directories named `tests`, `examples`, `benches`,
//! `fixtures` or `target`, files named `tests.rs`/`*_tests.rs`, comment lines
//! and `#[cfg(test)] mod …` blocks are skipped.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// A lifecycle API whose call sites the ledger must account for.
pub struct SitePattern {
    pub id: &'static str,
    /// Literals one of which every match contains; lines without any are
    /// skipped before the regex runs.
    pub needles: &'static [&'static str],
    pub regex: &'static str,
    pub meaning: &'static str,
}

/// Every call site of these APIs is a row in the ledger's `[[site]]` table.
pub const SITE_PATTERNS: &[SitePattern] = &[
    SitePattern {
        id: "destructive-drain",
        needles: &["take_component_change_events"],
        regex: r"\btake_component_change_events\s*\(",
        meaning: "Drains SceneDB's single shared change-event queue",
    },
    SitePattern {
        id: "panel-drain",
        needles: &["take_world_component_events"],
        regex: r"\btake_world_component_events\s*\(",
        meaning: "Properties-panel wrapper over the shared drain",
    },
    SitePattern {
        id: "script-drain",
        needles: &["take_change_events_for"],
        regex: r"\btake_change_events_for\s*\(",
        meaning: "Script object model: drains everything, then filters",
    },
    SitePattern {
        id: "subscribe",
        needles: &["subscribe"],
        regex: r"\.subscribe_id\s*\(|\.subscribe::<|\bsubscribe_component\s*\(",
        meaning: "Arms an entity/component subscription feeding the shared queue",
    },
    SitePattern {
        id: "change-cursor",
        needles: &["open_change_cursor", "read_changes"],
        regex: r"\bopen_change_cursor\b|\bread_changes\s*\(",
        meaning: "Independent per-component journal cursor",
    },
    SitePattern {
        id: "gpu-refresh",
        needles: &["refresh_"],
        regex: r"\brefresh_world_component_gpu_mirror\w*\s*\(|\.refresh_gpu_mirror\)\s*\(",
        meaning: "Caller-required GPU companion refresh",
    },
    SitePattern {
        id: "render-mark",
        needles: &["mark_render_components_changed"],
        regex: r"\bmark_render_components_changed\s*\(",
        meaning: "Synthetic change notification so the renderer re-projects",
    },
    SitePattern {
        id: "render-arm",
        needles: &["arm_render_row_subscriptions"],
        regex: r"\barm_render_row_subscriptions\w*\s*\(",
        meaning: "Arms renderer subscriptions so later edits are projected",
    },
    SitePattern {
        id: "pending-world-writes",
        needles: &["PendingWorldWrites"],
        regex: r"\bPendingWorldWrites\b",
        meaning: "Deferred world-write queue with no production drain",
    },
    SitePattern {
        id: "cpu-projection",
        needles: &["sync_static_mesh_rows", "sync_editor_light_rows", "project_movability", "sync_editor_postprocess"],
        regex: r"\b(sync_static_mesh_rows|sync_editor_light_rows|project_movability|sync_editor_postprocess)\s*\(",
        meaning: "CPU projection of authored components into pass-owned rows",
    },
    SitePattern {
        id: "mirror-replay",
        needles: &["ensure_gpu_mirror"],
        regex: r"\bensure_gpu_mirror\s*\(",
        meaning: "Attaches the shared GPU mirror (SceneDB replays existing rows on attach)",
    },
    SitePattern {
        id: "json-hydrate",
        needles: &["hydrate_"],
        regex: r"\bhydrate_world_component\w*\s*\(|\bhydrate_canonical_component\s*\(",
        meaning: "Builds a live component by deserializing JSON",
    },
    SitePattern {
        id: "render-props-sync",
        needles: &["sync_registered_component_props_to_scene_db"],
        regex: r"\bsync_registered_component_props_to_scene_db\s*\(",
        meaning: "Re-derives JSON RenderProps from attached components",
    },
    SitePattern {
        id: "behavior-dispatch",
        needles: &["apply_runtime_behavior_for_class", "dispatch_world_component"],
        regex: r"\bapply_runtime_behavior_for_class\s*\(|\bdispatch_world_component\w*\s*\(",
        meaning: "Generic ComponentRuntimeBehavior dispatch",
    },
    SitePattern {
        id: "force-resync",
        needles: &["force_full_resync"],
        regex: r"\b(queue_)?force_full_resync\s*\(",
        meaning: "Repair path: re-project the whole scene",
    },
    SitePattern {
        id: "add-component",
        needles: &["add_component", "AddComponent"],
        regex: r"\bcomponents::add_component(_instance)?\s*\(|\bSceneCommand::AddComponent\b",
        meaning: "Editor component-add producer",
    },
    SitePattern {
        id: "property-edit",
        needles: &["update_", "SetComponentData"],
        regex: r"\bupdate_live_component_property\s*\(|\bupdate_component_property\s*\(|\bupdate_component\s*\(|\bSceneCommand::SetComponentData\b",
        meaning: "Editor component-edit producer",
    },
];

const SCAN_ROOTS: &[&str] = &[
    "crates/core",
    "crates/editor",
    "crates/subsystems",
    "crates/renderer/helio/crates",
    "plugins/vendor",
];

const SKIPPED_DIRS: &[&str] = &[
    "target",
    "tests",
    "examples",
    "benches",
    "fixtures",
    "node_modules",
    ".git",
    // This crate: its pattern table would match itself.
    "scene_inventory",
];

/// One production source file, already stripped of test modules and comments.
pub struct SourceFile {
    /// Repository-relative, `/`-separated.
    pub path: String,
    /// `(1-based line number, line)` for every non-comment line.
    pub lines: Vec<(usize, String)>,
    /// Cargo package that owns the file.
    pub package: String,
}

fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name == "tests.rs" || name.ends_with("_tests.rs") || name == "build.rs"
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name) {
                walk(&path, out);
            }
        } else if name.ends_with(".rs") && !is_test_file(&path) {
            out.push(path);
        }
    }
}

fn package_name(dir: &Path, cache: &mut HashMap<PathBuf, Option<String>>) -> Option<String> {
    if let Some(cached) = cache.get(dir) {
        return cached.clone();
    }
    let manifest = dir.join("Cargo.toml");
    let found = fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| {
            table
                .get("package")?
                .get("name")?
                .as_str()
                .map(str::to_string)
        });
    let result = match found {
        Some(name) => Some(name),
        None => dir.parent().and_then(|parent| package_name(parent, cache)),
    };
    cache.insert(dir.to_path_buf(), result.clone());
    result
}

/// Production lines of `text`: comment lines and `#[cfg(test)] mod … { … }`
/// blocks dropped. Braces are counted naively, which is enough for test
/// modules.
fn production_lines(text: &str) -> Vec<(usize, String)> {
    let all: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < all.len() {
        let trimmed = all[i].trim_start();
        if trimmed.starts_with("#[cfg(test)]") {
            let module = (i + 1..all.len()).find(|&j| !all[j].trim().is_empty());
            if let Some(start) = module.filter(|&j| {
                let l = all[j].trim_start();
                l.starts_with("mod ") || l.starts_with("pub mod ") || l.starts_with("pub(crate) mod ")
            }) {
                let mut depth = 0i32;
                let mut end = start;
                for (j, line) in all.iter().enumerate().skip(start) {
                    depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
                    end = j;
                    // `mod tests;` has no body; a block ends when braces balance.
                    if depth <= 0 && (line.contains('}') || line.trim_end().ends_with(';')) {
                        break;
                    }
                }
                i = end + 1;
                continue;
            }
        }
        if !trimmed.starts_with("//") {
            out.push((i + 1, all[i].to_string()));
        }
        i += 1;
    }
    out
}

/// Every production `.rs` file under the scan roots, in path order.
pub fn production_files(root: &Path) -> &'static [SourceFile] {
    static FILES: OnceLock<Vec<SourceFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        let mut paths = Vec::new();
        for scan_root in SCAN_ROOTS {
            walk(&root.join(scan_root), &mut paths);
        }
        let mut packages = HashMap::new();
        paths
            .into_iter()
            .filter_map(|path| {
                let text = fs::read_to_string(&path).ok()?;
                let rel = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
                let package = path
                    .parent()
                    .and_then(|dir| package_name(dir, &mut packages))
                    .unwrap_or_else(|| "<no package>".to_string());
                Some(SourceFile {
                    path: rel,
                    lines: production_lines(&text),
                    package,
                })
            })
            .collect()
    })
}

/// `(pattern id, file) -> number of matching call sites`. Function
/// definitions (`fn name(`) are not call sites and are not counted.
pub fn sites(root: &Path) -> BTreeMap<(String, String), usize> {
    let compiled: Vec<(&SitePattern, Regex)> = SITE_PATTERNS
        .iter()
        .map(|p| (p, Regex::new(p.regex).expect("site pattern compiles")))
        .collect();
    let mut out = BTreeMap::new();
    for file in production_files(root) {
        for (_, line) in &file.lines {
            for (pattern, regex) in &compiled {
                if !pattern.needles.iter().any(|needle| line.contains(needle)) {
                    continue;
                }
                let hits = regex
                    .find_iter(line)
                    .filter(|m| !line[..m.start()].trim_end().ends_with("fn"))
                    .count();
                if hits > 0 {
                    *out.entry((pattern.id.to_string(), file.path.clone())).or_insert(0) += hits;
                }
            }
        }
    }
    out
}

/// Classes declared with `#[register_world_component]` in production source:
/// `class name -> file`.
pub fn declared_world_components(root: &Path) -> BTreeMap<String, String> {
    let target = Regex::new(r"impl\s+ComponentRuntimeBehavior\s+for\s+(\w+)").unwrap();
    let mut out = BTreeMap::new();
    for file in production_files(root) {
        let mut pending = false;
        for (_, line) in &file.lines {
            if !pending && !line.contains("#[register_world_component") {
                continue;
            }
            if line.trim_start().starts_with("#[register_world_component") {
                pending = true;
            } else if pending {
                if let Some(caps) = target.captures(line) {
                    out.insert(caps[1].to_string(), file.path.clone());
                    pending = false;
                }
            }
        }
    }
    out
}

/// Source-level GPU buffer graph.
#[derive(Debug, Default)]
pub struct BufferGraph {
    /// buffer key -> type names whose `#[gpu(buffer = "…")]` writes it.
    pub producers: BTreeMap<String, BTreeSet<String>>,
    /// buffer key -> packages that look it up with `BufferKey::of("…")`.
    pub consumers: BTreeMap<String, BTreeSet<String>>,
    /// type name -> buffer keys it writes.
    pub buffers_of_type: BTreeMap<String, BTreeSet<String>>,
}

pub fn buffer_graph(root: &Path) -> BufferGraph {
    let buffer = Regex::new(r#"#\[gpu\([^\]]*\bbuffer\s*=\s*"([^"]+)""#).unwrap();
    let structure = Regex::new(r"\bstruct\s+(\w+)").unwrap();
    let lookup = Regex::new(r#"BufferKey::of\(\s*"([^"]+)"\s*\)"#).unwrap();
    let mut graph = BufferGraph::default();
    for file in production_files(root) {
        let mut current_struct: Option<String> = None;
        for (index, (_, line)) in file.lines.iter().enumerate() {
            if line.contains("struct ") {
                if let Some(caps) = structure.captures(line) {
                    current_struct = Some(caps[1].to_string());
                }
            }
            if !line.contains("#[gpu(") && !line.contains("BufferKey::of") {
                continue;
            }
            if let Some(caps) = buffer.captures(line) {
                // A struct-level attribute precedes its `struct`; a field-level
                // one sits inside the struct it belongs to.
                let next_item = file.lines[index + 1..]
                    .iter()
                    .map(|(_, l)| l.trim_start())
                    .find(|l| !l.is_empty() && !l.starts_with("#["));
                let owner = next_item
                    .and_then(|l| structure.captures(l))
                    .map(|c| c[1].to_string())
                    .or_else(|| current_struct.clone())
                    .unwrap_or_else(|| "<unknown>".to_string());
                let key = caps[1].to_string();
                graph.producers.entry(key.clone()).or_default().insert(owner.clone());
                graph.buffers_of_type.entry(owner).or_default().insert(key);
            }
            for caps in lookup.captures_iter(line) {
                graph
                    .consumers
                    .entry(caps[1].to_string())
                    .or_default()
                    .insert(file.package.clone());
            }
        }
    }
    graph
}

/// The editor binary.
pub const EDITOR_PACKAGE: &str = "pulsar_engine";

/// Packages the editor binary links (its normal-dependency closure), from
/// `cargo metadata`. Helio also builds standalone apps (web, Android,
/// examples) that register their own buffers; those do not count.
pub fn editor_packages(root: &Path) -> BTreeSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(root)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata JSON");
    let names: HashMap<&str, &str> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?)))
        .collect();
    let nodes: HashMap<&str, Vec<&str>> = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let deps = node["deps"]
                .as_array()?
                .iter()
                .filter(|dep| {
                    dep["dep_kinds"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|k| k["kind"].is_null()))
                })
                .filter_map(|dep| dep["pkg"].as_str())
                .collect();
            Some((node["id"].as_str()?, deps))
        })
        .collect();
    let start = names
        .iter()
        .find(|(_, name)| **name == EDITOR_PACKAGE)
        .map(|(id, _)| *id)
        .expect("editor package in metadata");
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if let Some(name) = names.get(id) {
            if seen.insert(name.to_string()) {
                stack.extend(nodes.get(id).into_iter().flatten().copied());
            }
        }
    }
    seen
}

/// Types whose GPU columns the editor registers with a `SceneGpuStore`
/// (`T::register_gpu_columns*(…)` in a package of `linked`). Writes to an
/// unregistered column are dropped by SceneDB, so a schema missing here
/// never reaches the GPU. Each entry is the path as written at the call
/// site, e.g. `helio_pass_gbuffer::StaticObjectComponent`.
pub fn registered_gpu_schemas(root: &Path, linked: &BTreeSet<String>) -> BTreeSet<String> {
    let call = Regex::new(r"([A-Za-z_][\w:]*)::register_gpu_columns\w*\s*\(").unwrap();
    let mut out = BTreeSet::new();
    for file in production_files(root).iter().filter(|f| linked.contains(&f.package)) {
        for (_, line) in file.lines.iter().filter(|(_, l)| l.contains("register_gpu_columns")) {
            for caps in call.captures_iter(line) {
                out.insert(caps[1].to_string());
            }
        }
    }
    out
}

/// Whether `schema_path` (a full Rust type path) is one of `registered`'s
/// call-site paths: same last segment and, when the call site names a crate,
/// the same crate.
pub fn is_registered(registered: &BTreeSet<String>, schema_path: &str) -> bool {
    let short = crate::linked::short_type_name(schema_path);
    registered.iter().any(|call| {
        let segments: Vec<&str> = call.split("::").collect();
        let crate_hint = segments
            .first()
            .filter(|first| segments.len() > 1 && !matches!(**first, "crate" | "self" | "super"));
        segments.last() == Some(&short.as_str())
            && crate_hint.is_none_or(|krate| schema_path.starts_with(&format!("{krate}::")))
    })
}

/// A Helio render pass crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassCrate {
    pub package: String,
    /// Repository-relative directory.
    pub dir: String,
    /// `helio-default-graphs` depends on it.
    pub in_default_graph: bool,
}

pub fn pass_crates(root: &Path) -> Vec<PassCrate> {
    let helio = root.join("crates/renderer/helio/crates");
    let default_graph: BTreeSet<String> =
        fs::read_to_string(helio.join("helio-default-graphs/Cargo.toml"))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|t| t.get("dependencies").and_then(|d| d.as_table()).cloned())
            .map(|deps| deps.keys().cloned().collect())
            .unwrap_or_default();
    let mut cache = HashMap::new();
    let mut out = Vec::new();
    for group in ["passes/2d", "passes/3d"] {
        let Ok(entries) = fs::read_dir(helio.join(group)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let Some(package) = package_name(&dir, &mut cache) else {
                continue;
            };
            out.push(PassCrate {
                in_default_graph: default_graph.contains(&package),
                dir: dir.strip_prefix(root).unwrap_or(&dir).to_string_lossy().replace('\\', "/"),
                package,
            });
        }
    }
    out
}
