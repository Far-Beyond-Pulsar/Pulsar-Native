//! Architecture checks (Pulsar-Native#1035, Phase 6): the removed paths stay
//! removed, JSON decodes stay at the record boundary, and renderers neither
//! scan the CPU scene nor subscribe to it.
//!
//! Each check is narrow on purpose. Boundary codecs (level files, reflection
//! save codecs, asset files) and real gameplay events stay allowed; what is
//! checked is a call site that would bring back a removed mechanism. An
//! allowed site is listed with its reason, so a new one fails until it is
//! listed (or removed).

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::source;

/// Site patterns whose mechanism Phases 2–6 removed. None may have a
/// production call site.
pub const RETIRED_PATTERNS: &[(&str, &str)] = &[
    (
        "destructive-drain",
        "Phase 5: observers read independent journal cursors",
    ),
    (
        "panel-drain",
        "Phase 5/6: the properties panel follows an object feed",
    ),
    ("script-drain", "Phase 5: scripts read independent cursors"),
    (
        "subscribe",
        "Phase 6: SceneDB's shared-queue subscription API is gone",
    ),
    (
        "gpu-refresh",
        "Phase 2: SceneDB's normal write path keeps the GPU mirror current",
    ),
    (
        "render-mark",
        "Phase 2: renderers need no synthetic change notification",
    ),
    ("render-arm", "Phase 2: renderers arm no subscriptions"),
    (
        "pending-world-writes",
        "Phase 4: components write the World directly",
    ),
    (
        "cpu-projection",
        "Phase 2: no CPU projection into pass-owned rows",
    ),
    (
        "render-props-sync",
        "Phase 3: no JSON RenderProps derived from components",
    ),
    (
        "behavior-dispatch",
        "Phase 4: no generic runtime behavior dispatch",
    ),
    (
        "force-resync",
        "Phase 5: scene replacement needs no repair path",
    ),
];

/// The only files that decode a JSON component record into a live
/// component (`json-hydrate`, `record-attach`): the level-file, migration and
/// record-tool boundary. Live edits, history, class instantiation and
/// scripts use typed values.
pub const RECORD_BOUNDARY_FILES: &[(&str, &str)] = &[
    (
        "crates/core/engine_backend/src/scene/runtime_level.rs",
        "level load: each record decoded once into a component instance",
    ),
    (
        "crates/core/level_migrate/src/main.rs",
        "offline level migration tool",
    ),
    (
        "crates/core/pulsar_class/src/world.rs",
        "attach_components: a record-boundary helper (level-load tests); instantiation clones typed template values",
    ),
    (
        "crates/core/pulsar_world_registry/src/instances.rs",
        "the record boundary API itself (attach_record, replace_records)",
    ),
    (
        "crates/editor/ui_level_editor/src/core/scene_edit/components.rs",
        "editor record boundary: a level's records attached at load, add from a record",
    ),
];

/// Source roots of the renderer: the editor's Helio bridge, the Helio
/// crates and the engine's shared render components. Every plugin's
/// renderer parts are too: its Helio feature crates and its standalone host
/// (`plugins/vendor/<kind>/feature/`, `.../host/`, [`PLUGIN_RENDERER_DIRS`]);
/// a plugin's component, editor and world crates are not the renderer.
pub const RENDERER_ROOTS: &[&str] = &[
    "crates/core/engine_backend/src/subsystems/render/",
    "crates/renderer/helio/crates/",
    "crates/core/helio_component/",
];

/// The directories of a plugin (`plugins/vendor/<kind>/<dir>/`) that are
/// renderer: its Helio features and its standalone host.
pub const PLUGIN_RENDERER_DIRS: &[&str] = &["feature", "host"];

/// The renderer's `World::query` call sites, with why each is not a
/// per-frame scan that discovers or projects render components. Rendering
/// reads the GPU mirror; incremental CPU work reads change cursors.
pub const RENDERER_QUERY_SITES: &[(&str, usize, &str)] = &[
    (
        "crates/core/engine_backend/src/subsystems/render/helio_renderer/interaction.rs",
        2,
        "picking and the nearest hit under a ray: run on a pointer event, not per frame",
    ),
    (
        "crates/core/engine_backend/src/subsystems/render/helio_renderer/camera_ground.rs",
        1,
        "the camera's ground: the instances of components with a `ground_altitude` method (by method, no kind named), found again only when the world revision changes",
    ),
    (
        "plugins/vendor/meshes/host/src/lib.rs",
        3,
        "bake_scene_from_world: a standalone host's explicit one-shot bake input, not the frame path",
    ),
    (
        "plugins/vendor/meshes/host/src/ray_tracing.rs",
        3,
        "a standalone host's HLFS acceleration structure: one full build, then change cursors",
    ),
];

fn in_renderer(path: &str) -> bool {
    RENDERER_ROOTS.iter().any(|root| path.starts_with(root))
        || path
            .strip_prefix("plugins/vendor/")
            .and_then(|rest| rest.split('/').nth(1))
            .is_some_and(|dir| PLUGIN_RENDERER_DIRS.contains(&dir))
}

/// Retired patterns that still have production call sites.
pub fn check_retired(sites: &BTreeMap<(String, String), usize>) -> Vec<String> {
    let mut problems = Vec::new();
    for (id, reason) in RETIRED_PATTERNS {
        assert!(
            source::SITE_PATTERNS.iter().any(|p| p.id == *id),
            "retired pattern {id} is not a site pattern"
        );
        for ((pattern, file), count) in sites {
            if pattern == id {
                problems.push(format!("{file}: {count} `{id}` call site(s) ({reason})"));
            }
        }
    }
    problems
}

/// JSON record decodes outside [`RECORD_BOUNDARY_FILES`].
pub fn check_record_boundary(sites: &BTreeMap<(String, String), usize>) -> Vec<String> {
    sites
        .iter()
        .filter(|((pattern, _), _)| pattern == "json-hydrate" || pattern == "record-attach")
        .filter(|((_, file), _)| !RECORD_BOUNDARY_FILES.iter().any(|(f, _)| f == file))
        .map(|((pattern, file), count)| {
            format!(
                "{file}: {count} `{pattern}` call site(s) outside the record boundary; \
                 write the typed value instead"
            )
        })
        .collect()
}

/// Object subscriptions in renderer code: bulk readers read the `World`.
pub fn check_renderer_subscriptions(sites: &BTreeMap<(String, String), usize>) -> Vec<String> {
    sites
        .iter()
        .filter(|((pattern, file), _)| pattern == "object-subscribe" && in_renderer(file))
        .map(|((_, file), count)| {
            format!(
                "{file}: {count} object subscription(s); renderers read the World or the \
                 GPU mirror, subscriptions are for views"
            )
        })
        .collect()
}

/// `World::query` call sites per renderer file.
pub fn renderer_queries(root: &Path) -> BTreeMap<String, usize> {
    let query = Regex::new(r"\.query(_items)?::<").unwrap();
    let mut out = BTreeMap::new();
    for file in source::production_files(root) {
        if !in_renderer(&file.path) {
            continue;
        }
        let hits: usize = file
            .lines
            .iter()
            .map(|(_, line)| query.find_iter(line).count())
            .sum();
        if hits > 0 {
            out.insert(file.path.clone(), hits);
        }
    }
    out
}

/// Renderer query sites that differ from [`RENDERER_QUERY_SITES`].
pub fn check_renderer_queries(found: &BTreeMap<String, usize>) -> Vec<String> {
    let mut problems = Vec::new();
    for (file, count) in found {
        match RENDERER_QUERY_SITES.iter().find(|(f, _, _)| f == file) {
            Some((_, listed, _)) if listed == count => {}
            Some((_, listed, _)) => problems.push(format!(
                "{file}: {count} World::query call(s), {listed} listed; a renderer must not \
                 scan the scene to discover or project components"
            )),
            None => problems.push(format!(
                "{file}: {count} unlisted World::query call(s); a renderer must not scan the \
                 scene to discover or project components"
            )),
        }
    }
    for (file, _, _) in RENDERER_QUERY_SITES {
        if !found.contains_key(*file) {
            problems.push(format!(
                "{file}: listed renderer query site is gone; remove it"
            ));
        }
    }
    problems
}

/// Source roots of the world crates whose process-wide state a plugin's
/// copy must reach through the world runtime (Pulsar-Native#1083).
/// `pulsar_reflection` and `pulsar_scenedb` check their own sources.
pub const WORLD_RUNTIME_ROOTS: &[&str] = &[
    "crates/core/pulsar_scene_model/src/",
    "crates/core/pulsar_world_registry/src/",
];

/// Every `static`, `thread_local!`, `inventory::collect!` (or
/// `pulsar_plugins::registration!`, which declares the list) and
/// `lazy_static!` in the world crates, with why a plugin's copy may keep its
/// own. Anything else that holds process-wide state belongs in the crate's
/// `runtime` module, or a plugin's registrations never reach the editor.
pub const WORLD_STATE_SITES: &[(&str, usize, &str)] = &[
    (
        "crates/core/pulsar_scene_model/src/frames.rs",
        1,
        "the SpaceDefiner inventory list: kinds that define coordinate spaces, statically linked plugins only (a DLL plugin defines none)",
    ),
    (
        "crates/core/pulsar_scene_model/src/motion.rs",
        1,
        "the MotionGate inventory list, read through the runtime's motion_gates",
    ),
    (
        "crates/core/pulsar_scene_model/src/runtime.rs",
        4,
        "the runtime itself: ordinal counter, motion gates, OWN, ATTACHED",
    ),
    (
        "crates/core/pulsar_world_registry/src/dispatch.rs",
        1,
        "per-copy cache of class properties, built from the shared registries",
    ),
    (
        "crates/core/pulsar_world_registry/src/lib.rs",
        5,
        "the tick, event and world component inventory lists, read through the \
         runtime; an immutable RuntimeTypeInfo; a per-copy set of component \
         event names the hub refused, already reported to the log",
    ),
    (
        "crates/core/pulsar_world_registry/src/runtime.rs",
        13,
        "the runtime itself: registration lists (classes, ticks, events, unfinished reports, and the classes' systems), OWN, ATTACHED, the host table, and each list's plugin gate (a disabled plugin's registrations skipped)",
    ),
    (
        "crates/core/pulsar_world_registry/src/scene_payloads.rs",
        1,
        "the scene payload inventory list, read through pulsar_plugins::registered (the plugin gate); statically linked plugins only",
    ),
    (
        "crates/core/pulsar_world_registry/src/scene_subsystems.rs",
        1,
        "the scene subsystem inventory list, read through pulsar_plugins::registered (the plugin gate); statically linked plugins only",
    ),
    (
        "crates/core/pulsar_world_registry/src/unfinished.rs",
        2,
        "the unfinished-class inventory list, read through the runtime; a \
         per-copy set of classes already reported to the log",
    ),
];

/// Process-wide state declarations per world-crate file.
pub fn world_state(root: &Path) -> BTreeMap<String, usize> {
    let state = Regex::new(
        r"^\s*(pub(\([^)]*\))?\s+)?static\s+(mut\s+)?[A-Z_][A-Z0-9_]*\s*:|thread_local!|inventory::collect!|pulsar_plugins::registration!|lazy_static!",
    )
    .unwrap();
    let mut out = BTreeMap::new();
    for file in source::production_files(root) {
        if !WORLD_RUNTIME_ROOTS
            .iter()
            .any(|root| file.path.starts_with(root))
        {
            continue;
        }
        let hits = file
            .lines
            .iter()
            .filter(|(_, line)| state.is_match(line))
            .count();
        if hits > 0 {
            out.insert(file.path.clone(), hits);
        }
    }
    out
}

/// World-crate state that differs from [`WORLD_STATE_SITES`].
pub fn check_world_state(found: &BTreeMap<String, usize>) -> Vec<String> {
    let mut problems = Vec::new();
    for (file, count) in found {
        match WORLD_STATE_SITES.iter().find(|(f, _, _)| f == file) {
            Some((_, listed, _)) if listed == count => {}
            Some((_, listed, _)) => problems.push(format!(
                "{file}: {count} process-wide state declaration(s), {listed} listed; state a \
                 plugin's copy must share goes through the crate's runtime (#1083)"
            )),
            None => problems.push(format!(
                "{file}: {count} unlisted process-wide state declaration(s); state a plugin's \
                 copy must share goes through the crate's runtime (#1083)"
            )),
        }
    }
    for (file, _, _) in WORLD_STATE_SITES {
        if !found.contains_key(*file) {
            problems.push(format!("{file}: listed world state is gone; remove it"));
        }
    }
    problems
}
