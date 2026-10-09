//! Phase 0 closure ledger: every exposed class, GPU schema, buffer key, pass
//! crate and lifecycle call site has a row, and every row's facts match the
//! linked binary and the source tree.
//!
//! A failure prints the rows to add or correct, with `TODO` placeholders for
//! the human-owned columns.

use scene_inventory::{ledger, linked, repo_root, source};

fn report(section: &str, problems: Vec<String>) {
    assert!(
        problems.is_empty(),
        "{section}: {} problem(s) in {}\n\n{}",
        problems.len(),
        scene_inventory::ledger_path().display(),
        problems.join("\n")
    );
}

fn load() -> ledger::Ledger {
    ledger::load(&scene_inventory::ledger_path()).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn every_linked_class_has_a_row() {
    report("classes", ledger::check_classes(&load(), &linked::collect()));
}

#[test]
fn unfinished_rows_match_unfinished_declarations() {
    let linked = linked::collect();
    assert!(!linked.unfinished.is_empty(), "no unfinished declarations are linked");
    report("unfinished classes", ledger::check_unfinished(&load(), &linked));
}

#[test]
fn every_declared_world_component_is_linked() {
    let declared = source::declared_world_components(&repo_root());
    assert!(!declared.is_empty(), "the source scan found no world components");
    report(
        "declared world components",
        ledger::check_declared_world_components(&declared, &linked::collect()),
    );
}

#[test]
fn every_linked_gpu_schema_has_a_row() {
    let root = repo_root();
    let graph = source::buffer_graph(&root);
    let registered = source::registered_gpu_schemas(&root, &source::editor_packages(&root));
    report(
        "schemas",
        ledger::check_schemas(&load(), &linked::collect(), &graph, &registered),
    );
}

#[test]
fn every_gpu_buffer_has_a_row() {
    report("buffers", ledger::check_buffers(&load(), &source::buffer_graph(&repo_root())));
}

#[test]
fn every_pass_crate_has_a_row() {
    let root = repo_root();
    let passes = source::pass_crates(&root);
    assert!(!passes.is_empty(), "no pass crates under the Helio submodule");
    report("passes", ledger::check_passes(&load(), &passes, &source::buffer_graph(&root)));
}

#[test]
fn every_lifecycle_call_site_has_a_row() {
    report("sites", ledger::check_sites(&load(), &source::sites(&repo_root())));
}

#[test]
fn every_row_has_a_disposition() {
    report("dispositions", ledger::check_dispositions(&load()));
}
