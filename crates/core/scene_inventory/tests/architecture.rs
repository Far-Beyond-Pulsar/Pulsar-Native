//! Phase 6 architecture checks: removed mechanisms stay removed, JSON record
//! decodes stay at the boundary, and renderers neither scan nor subscribe to
//! the CPU scene. See `scene_inventory::architecture`.

use scene_inventory::{architecture, repo_root, source};

fn report(section: &str, problems: Vec<String>) {
    assert!(
        problems.is_empty(),
        "{section}: {} problem(s)\n\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn retired_mechanisms_have_no_call_sites() {
    report(
        "retired mechanisms",
        architecture::check_retired(&source::sites(&repo_root())),
    );
}

#[test]
fn json_records_decode_only_at_the_boundary() {
    report(
        "record boundary",
        architecture::check_record_boundary(&source::sites(&repo_root())),
    );
}

#[test]
fn renderers_do_not_subscribe_to_objects() {
    report(
        "renderer subscriptions",
        architecture::check_renderer_subscriptions(&source::sites(&repo_root())),
    );
}

#[test]
fn renderer_world_queries_are_the_listed_ones() {
    report(
        "renderer queries",
        architecture::check_renderer_queries(&architecture::renderer_queries(&repo_root())),
    );
}
