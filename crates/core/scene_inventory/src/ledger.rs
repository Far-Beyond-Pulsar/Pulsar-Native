//! The closure ledger and its comparison against linked and source facts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::linked::Linked;
use crate::source::{BufferGraph, PassCrate, SITE_PATTERNS};

/// Allowed `status` values. Anything stronger than `at-risk` needs a named
/// test or reproduction in `test`.
pub const STATUSES: &[&str] = &[
    // Reproduced failure (test or runtime evidence named in `test`).
    "broken",
    // Source-level defect against the corrective plan; not reproduced.
    "at-risk",
    // No defect identified, no evidence either way.
    "unverified",
    // Behavior verified by the named test.
    "verified",
    // Outside the corrective plan; `disposition` says why.
    "out-of-scope",
];

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub meta: Meta,
    #[serde(default, rename = "class")]
    pub classes: Vec<ClassRow>,
    #[serde(default, rename = "schema")]
    pub schemas: Vec<SchemaRow>,
    #[serde(default, rename = "buffer")]
    pub buffers: Vec<BufferRow>,
    #[serde(default, rename = "pass")]
    pub passes: Vec<PassRow>,
    #[serde(default, rename = "site")]
    pub sites: Vec<SiteRow>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub pulsar_native: String,
    pub helio: String,
    pub scenedb: String,
    pub reflection: String,
}

/// Human-owned columns shared by every row kind. Row structs flatten this,
/// which serde cannot combine with `deny_unknown_fields`.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Disposition {
    /// Module or crate that owns the change.
    pub owner: String,
    /// What happens to this row in the corrective plan.
    pub disposition: String,
    /// Plan phase and contract section the row must satisfy.
    pub target_contract: String,
    /// Test or reproduction that proves the row, or `none: <what is missing>`.
    pub test: String,
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Deserialize)]
pub struct ClassRow {
    pub name: String,
    pub category: String,
    pub world_registered: bool,
    pub own_gpu_columns: bool,
    pub runtime_behavior: bool,
    #[serde(flatten)]
    pub d: Disposition,
}

#[derive(Debug, Deserialize)]
pub struct SchemaRow {
    #[serde(rename = "type")]
    pub ty: String,
    pub path: String,
    pub buffers: Vec<String>,
    pub clears_on_remove: bool,
    pub releases_var_len: bool,
    /// Production code registers its GPU columns; otherwise SceneDB drops
    /// every write to them.
    pub registered: bool,
    /// Code that inserts this type's rows into `World`.
    pub writer: String,
    #[serde(flatten)]
    pub d: Disposition,
}

#[derive(Debug, Deserialize)]
pub struct BufferRow {
    pub key: String,
    pub producers: Vec<String>,
    pub consumers: Vec<String>,
    #[serde(flatten)]
    pub d: Disposition,
}

#[derive(Debug, Deserialize)]
pub struct PassRow {
    #[serde(rename = "crate")]
    pub package: String,
    pub in_default_graph: bool,
    pub reads: Vec<String>,
    #[serde(flatten)]
    pub d: Disposition,
}

#[derive(Debug, Deserialize)]
pub struct SiteRow {
    pub pattern: String,
    pub file: String,
    pub count: usize,
    #[serde(flatten)]
    pub d: Disposition,
}

pub fn load(path: &Path) -> Result<Ledger, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

const TODO: &str = r#"owner = "TODO"
disposition = "TODO"
target_contract = "TODO"
test = "TODO"
status = "TODO""#;

fn list(items: &BTreeSet<String>) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

fn sorted(items: &[String]) -> BTreeSet<String> {
    items.iter().cloned().collect()
}

/// Rows whose human columns are missing or whose status is not allowed.
pub fn check_dispositions(ledger: &Ledger) -> Vec<String> {
    let mut problems = Vec::new();
    let mut check = |row: String, d: &Disposition| {
        for (field, value) in [
            ("owner", &d.owner),
            ("disposition", &d.disposition),
            ("target_contract", &d.target_contract),
            ("test", &d.test),
        ] {
            if value.trim().is_empty() || value.contains("TODO") {
                problems.push(format!("{row}: `{field}` is not filled in"));
            }
        }
        if !STATUSES.contains(&d.status.as_str()) {
            problems.push(format!(
                "{row}: status {:?} is not one of {STATUSES:?}",
                d.status
            ));
        }
        if matches!(d.status.as_str(), "broken" | "verified") && d.test.starts_with("none") {
            problems.push(format!("{row}: status `{}` needs a named test", d.status));
        }
    };
    for r in &ledger.classes {
        check(format!("class {}", r.name), &r.d);
    }
    for r in &ledger.schemas {
        check(format!("schema {}", r.ty), &r.d);
    }
    for r in &ledger.buffers {
        check(format!("buffer {}", r.key), &r.d);
    }
    for r in &ledger.passes {
        check(format!("pass {}", r.package), &r.d);
    }
    for r in &ledger.sites {
        check(format!("site {} in {}", r.pattern, r.file), &r.d);
    }
    problems
}

/// Class rows against the linked reflection and World registries.
pub fn check_classes(ledger: &Ledger, linked: &Linked) -> Vec<String> {
    let mut problems = Vec::new();
    let rows: BTreeMap<&str, &ClassRow> = ledger.classes.iter().map(|r| (r.name.as_str(), r)).collect();
    for class in &linked.classes {
        let category = class.category.clone().unwrap_or_default();
        match rows.get(class.name.as_str()) {
            None => problems.push(format!(
                "linked class `{}` has no ledger row:\n[[class]]\nname = {:?}\ncategory = {:?}\nworld_registered = {}\nown_gpu_columns = {}\nruntime_behavior = {}\n{TODO}\n",
                class.name, class.name, category, class.world_registered, class.own_gpu_columns, class.runtime_behavior
            )),
            Some(row) => {
                let facts = [
                    ("category", row.category != category, format!("{category:?}")),
                    ("world_registered", row.world_registered != class.world_registered, class.world_registered.to_string()),
                    ("own_gpu_columns", row.own_gpu_columns != class.own_gpu_columns, class.own_gpu_columns.to_string()),
                    ("runtime_behavior", row.runtime_behavior != class.runtime_behavior, class.runtime_behavior.to_string()),
                ];
                for (field, differs, actual) in facts {
                    if differs {
                        problems.push(format!("class `{}`: `{field}` is {actual} in the linked binary", class.name));
                    }
                }
            }
        }
    }
    for row in &ledger.classes {
        if !linked.classes.iter().any(|c| c.name == row.name) {
            problems.push(format!("class row `{}` is not a linked reflection class", row.name));
        }
    }
    for name in &linked.world_only {
        problems.push(format!("World registration `{name}` has no reflection class"));
    }
    problems
}

/// Every `#[register_world_component]` in source must be linked; otherwise a
/// class exists in code but is invisible to this binary's registry.
pub fn check_declared_world_components(declared: &BTreeMap<String, String>, linked: &Linked) -> Vec<String> {
    declared
        .iter()
        .filter(|(name, _)| !linked.classes.iter().any(|c| &c.name == *name && c.world_registered))
        .map(|(name, file)| format!("`{name}` is declared in {file} but not registered in the linked binary"))
        .collect()
}

/// Schema rows against the linked GPU-mirror registrations and source buffers.
pub fn check_schemas(
    ledger: &Ledger,
    linked: &Linked,
    graph: &BufferGraph,
    registered: &BTreeSet<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let rows: BTreeMap<&str, &SchemaRow> = ledger.schemas.iter().map(|r| (r.path.as_str(), r)).collect();
    for schema in &linked.schemas {
        let buffers = graph.buffers_of_type.get(&schema.short).cloned().unwrap_or_default();
        let is_registered = crate::source::is_registered(registered, &schema.path);
        match rows.get(schema.path.as_str()) {
            None => problems.push(format!(
                "linked GPU schema `{}` has no ledger row:\n[[schema]]\ntype = {:?}\npath = {:?}\nbuffers = {}\nclears_on_remove = {}\nreleases_var_len = {}\nregistered = {}\nwriter = \"TODO\"\n{TODO}\n",
                schema.path, schema.short, schema.path, list(&buffers), schema.clears_on_remove, schema.releases_var_len, is_registered
            )),
            Some(row) => {
                if row.ty != schema.short {
                    problems.push(format!("schema `{}`: `type` should be {:?}", schema.path, schema.short));
                }
                if sorted(&row.buffers) != buffers {
                    problems.push(format!("schema `{}`: `buffers` is {} in source", schema.path, list(&buffers)));
                }
                if row.clears_on_remove != schema.clears_on_remove || row.releases_var_len != schema.releases_var_len {
                    problems.push(format!(
                        "schema `{}`: clears_on_remove = {}, releases_var_len = {} in the linked binary",
                        schema.path, schema.clears_on_remove, schema.releases_var_len
                    ));
                }
                if row.registered != is_registered {
                    problems.push(format!(
                        "schema `{}`: `registered` is {is_registered} (production `register_gpu_columns*` calls)",
                        schema.path
                    ));
                }
                if row.writer.trim().is_empty() || row.writer.contains("TODO") {
                    problems.push(format!("schema `{}`: `writer` is not filled in", schema.path));
                }
            }
        }
    }
    for row in &ledger.schemas {
        if !linked.schemas.iter().any(|s| s.path == row.path) {
            problems.push(format!("schema row `{}` is not a linked GPU schema", row.path));
        }
    }
    problems
}

/// Buffer rows against the source buffer graph.
pub fn check_buffers(ledger: &Ledger, graph: &BufferGraph) -> Vec<String> {
    let mut problems = Vec::new();
    let keys: BTreeSet<&String> = graph.producers.keys().chain(graph.consumers.keys()).collect();
    let rows: BTreeMap<&str, &BufferRow> = ledger.buffers.iter().map(|r| (r.key.as_str(), r)).collect();
    for key in keys {
        let producers = graph.producers.get(key).cloned().unwrap_or_default();
        let consumers = graph.consumers.get(key).cloned().unwrap_or_default();
        match rows.get(key.as_str()) {
            None => problems.push(format!(
                "buffer `{key}` has no ledger row:\n[[buffer]]\nkey = {key:?}\nproducers = {}\nconsumers = {}\n{TODO}\n",
                list(&producers),
                list(&consumers)
            )),
            Some(row) => {
                if sorted(&row.producers) != producers {
                    problems.push(format!("buffer `{key}`: `producers` is {} in source", list(&producers)));
                }
                if sorted(&row.consumers) != consumers {
                    problems.push(format!("buffer `{key}`: `consumers` is {} in source", list(&consumers)));
                }
            }
        }
    }
    for row in &ledger.buffers {
        if !graph.producers.contains_key(&row.key) && !graph.consumers.contains_key(&row.key) {
            problems.push(format!("buffer row `{}` is neither produced nor consumed in source", row.key));
        }
    }
    problems
}

/// Pass rows against the pass crates on disk.
pub fn check_passes(ledger: &Ledger, passes: &[PassCrate], graph: &BufferGraph) -> Vec<String> {
    let mut problems = Vec::new();
    let rows: BTreeMap<&str, &PassRow> = ledger.passes.iter().map(|r| (r.package.as_str(), r)).collect();
    for pass in passes {
        let reads: BTreeSet<String> = graph
            .consumers
            .iter()
            .filter(|(_, packages)| packages.contains(&pass.package))
            .map(|(key, _)| key.clone())
            .collect();
        match rows.get(pass.package.as_str()) {
            None => problems.push(format!(
                "pass crate `{}` ({}) has no ledger row:\n[[pass]]\ncrate = {:?}\nin_default_graph = {}\nreads = {}\n{TODO}\n",
                pass.package, pass.dir, pass.package, pass.in_default_graph, list(&reads)
            )),
            Some(row) => {
                if row.in_default_graph != pass.in_default_graph {
                    problems.push(format!("pass `{}`: `in_default_graph` is {}", pass.package, pass.in_default_graph));
                }
                if sorted(&row.reads) != reads {
                    problems.push(format!("pass `{}`: `reads` is {} in source", pass.package, list(&reads)));
                }
            }
        }
    }
    for row in &ledger.passes {
        if !passes.iter().any(|p| p.package == row.package) {
            problems.push(format!("pass row `{}` is not a pass crate", row.package));
        }
    }
    problems
}

/// Site rows against the lifecycle call sites in source.
pub fn check_sites(ledger: &Ledger, sites: &BTreeMap<(String, String), usize>) -> Vec<String> {
    let mut problems = Vec::new();
    for row in &ledger.sites {
        if !SITE_PATTERNS.iter().any(|p| p.id == row.pattern) {
            problems.push(format!("site row uses unknown pattern `{}`", row.pattern));
        }
    }
    let rows: BTreeMap<(String, String), &SiteRow> = ledger
        .sites
        .iter()
        .map(|r| ((r.pattern.clone(), r.file.clone()), r))
        .collect();
    for ((pattern, file), count) in sites {
        match rows.get(&(pattern.clone(), file.clone())) {
            None => problems.push(format!(
                "{count} `{pattern}` site(s) in {file} have no ledger row:\n[[site]]\npattern = {pattern:?}\nfile = {file:?}\ncount = {count}\n{TODO}\n"
            )),
            Some(row) if row.count != *count => problems.push(format!(
                "`{pattern}` in {file}: {count} site(s) in source, ledger says {}",
                row.count
            )),
            Some(_) => {}
        }
    }
    for row in &ledger.sites {
        if !sites.contains_key(&(row.pattern.clone(), row.file.clone())) {
            problems.push(format!(
                "site row `{}` in {} no longer matches any source line (remove the row or record its replacement)",
                row.pattern, row.file
            ));
        }
    }
    problems
}
