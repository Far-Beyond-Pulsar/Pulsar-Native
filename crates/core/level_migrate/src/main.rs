//! Standalone best-effort repair tool for `.level` files whose component
//! JSON was saved in the wrong shape (Pulsar-Native#561 -- e.g. a
//! `#[sub_props]`-nested component stored flat/leaf-keyed, or an enum
//! stored as a raw index instead of its string tag).
//!
//! Rebuilds each affected component via the exact same reflection
//! machinery the editor's "add component" path uses (create a fresh
//! `Default` instance, apply each leaf through the real reflected setter,
//! then call the class's own `to_json()`), and verifies the result the
//! same way the engine verifies live edits: through
//! `pulsar_world_registry::hydrate_world_component_for_class`, the actual
//! typed-`Deserialize` path used at load/edit time. Nothing is guessed at
//! by this tool's own logic -- if the real hydrate path doesn't accept a
//! component before OR after the repair attempt, that component is left
//! exactly as it was found and reported, not silently dropped or corrupted
//! further.
//!
//! Always makes a `<file>.bak` backup before writing anything (skipped if
//! that backup already exists, so re-running this on an already-migrated
//! file never overwrites the one true original with a "fixed" copy).

// Force every crate that defines `#[register_world_component]` classes into
// the binary. This tool never references any of these types directly --
// everything goes through `class_name: &str` plus the reflection/world
// registries -- so without a live symbol reference the linker is free to
// drop their `#[used]` inventory-registration statics entirely (same
// reasoning as `pulsar_scene::loader`'s forced re-exports).
#[allow(unused_imports)]
use helio_component::{
    FoliageComponent as _ForceLink_FoliageComponent, LODComponent as _ForceLink_LODComponent,
    LightComponent as _ForceLink_LightComponent,
    MaterialOverrideComponent as _ForceLink_MaterialOverrideComponent,
    PlanetTerrainComponent as _ForceLink_PlanetTerrainComponent,
    PortalComponent as _ForceLink_PortalComponent,
    PostProcessVolumeComponent as _ForceLink_PostProcessVolumeComponent,
    ReflectionCaptureComponent as _ForceLink_ReflectionCaptureComponent,
    ScriptComponent as _ForceLink_ScriptComponent,
    StaticMeshComponent as _ForceLink_StaticMeshComponent,
    WaterVolumeComponent as _ForceLink_WaterVolumeComponent,
};
#[allow(unused_imports)]
use pulsar_physics::{
    PhysicsComponent as _ForceLink_PhysicsComponent,
    RigidbodyComponent as _ForceLink_RigidbodyComponent,
};

use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::World;
use pulsar_world_registry::hydrate_world_component_for_class;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    println!("=================================================================");
    println!(" Pulsar Level Repair -- .level component data repair tool");
    println!("=================================================================");
    println!();
    println!("WARNING: this is a BEST-EFFORT repair tool, not a guarantee. It");
    println!("reconstructs each affected component's data through the engine's");
    println!("own current type definitions, but it cannot recover information");
    println!("that was never stored correctly in the first place -- a leaf value");
    println!("that can't be parsed just falls back to that field's default.");
    println!("Always re-check the result in the editor afterward, especially for");
    println!("a file that matters. Nothing is overwritten without a backup.");
    println!();
    println!("The original file is ALWAYS copied to '<file>.bak' before this");
    println!("tool writes anything (skipped only if that backup already exists,");
    println!("so re-running this tool never clobbers your one true original).");
    println!();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<PathBuf> = if args.is_empty() {
        match prompt_for_path() {
            Some(p) => vec![p],
            None => {
                println!("No path given -- nothing to do.");
                pause_and_exit(1);
            }
        }
    } else {
        args.into_iter().map(PathBuf::from).collect()
    };

    let mut any_failed = false;
    for path in &paths {
        println!("-----------------------------------------------------------------");
        println!("File: {}", path.display());
        if let Err(err) = migrate_file(path) {
            eprintln!("  ERROR: {err}");
            any_failed = true;
        }
    }

    println!("-----------------------------------------------------------------");
    println!("Done.");
    pause_and_exit(if any_failed { 1 } else { 0 });
}

fn prompt_for_path() -> Option<PathBuf> {
    print!("Drag a .level file onto this window (or type its path) and press Enter: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return None;
    }
    let trimmed = line.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Keeps the console window open when this was launched by double-click
/// (no controlling terminal to read output from otherwise), then exits.
fn pause_and_exit(code: i32) -> ! {
    print!("Press Enter to close this window...");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    std::process::exit(code);
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

#[derive(Default)]
struct MigrationReport {
    already_ok: usize,
    migrated: usize,
    skipped_unregistered: Vec<String>,
    failed: Vec<String>,
}

impl MigrationReport {
    fn print(&self) {
        println!("  already correct, untouched: {}", self.already_ok);
        println!("  repaired: {}", self.migrated);
        if !self.skipped_unregistered.is_empty() {
            println!(
                "  not a checkable component class (legacy/unregistered, left as-is): {}",
                self.skipped_unregistered.len()
            );
            for s in &self.skipped_unregistered {
                println!("    - {s}");
            }
        }
        if !self.failed.is_empty() {
            println!(
                "  COULD NOT REPAIR -- left unchanged, needs manual attention: {}",
                self.failed.len()
            );
            for s in &self.failed {
                println!("    - {s}");
            }
        }
    }
}

fn migrate_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("not a file: {}", path.display()));
    }

    let content = std::fs::read_to_string(path).map_err(|e| format!("failed to read: {e}"))?;
    let mut root: Value =
        serde_json::from_str(&content).map_err(|e| format!("not valid JSON: {e}"))?;

    let backup_path = backup_path_for(path);
    if backup_path.exists() {
        println!(
            "  backup already exists at {} -- leaving it alone",
            backup_path.display()
        );
    } else {
        std::fs::copy(path, &backup_path)
            .map_err(|e| format!("failed to create backup {}: {e}", backup_path.display()))?;
        println!("  backed up original to {}", backup_path.display());
    }

    let mut report = MigrationReport::default();

    // V2+ shape: each object carries its own `component_instances[]`.
    if let Some(objects) = root.get_mut("objects").and_then(Value::as_array_mut) {
        for obj in objects {
            let obj_id = obj
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            if let Some(instances) = obj
                .get_mut("component_instances")
                .and_then(Value::as_array_mut)
            {
                for inst in instances {
                    let Some(class_name) = inst
                        .get("class_name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    if let Some(data_slot) = inst.get_mut("data") {
                        migrate_entry(&obj_id, &class_name, data_slot, &mut report);
                    }
                }
            }
        }
    }

    // Top-level `components{}` map -- some scene shapes mirror instances
    // here too; migrate both so neither copy is left stale.
    if let Some(components_map) = root.get_mut("components").and_then(Value::as_object_mut) {
        for (obj_id, list) in components_map.iter_mut() {
            let Some(list) = list.as_array_mut() else {
                continue;
            };
            for entry in list {
                let Some(class_name) = entry
                    .get("class_name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    continue;
                };
                if let Some(data_slot) = entry.get_mut("data") {
                    migrate_entry(obj_id, &class_name, data_slot, &mut report);
                }
            }
        }
    }

    report.print();

    if report.migrated > 0 {
        let pretty = serde_json::to_string_pretty(&root)
            .map_err(|e| format!("failed to serialize result: {e}"))?;
        std::fs::write(path, pretty)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        println!("  wrote repaired file to {}", path.display());
    } else {
        println!("  nothing needed repair -- file left unchanged (backup was still made above)");
    }

    Ok(())
}

/// Attempts to repair one component instance's `data` in place. Leaves
/// `data_slot` completely untouched unless a repaired version is produced
/// AND verified to load cleanly through the real hydrate path.
fn migrate_entry(
    obj_id: &str,
    class_name: &str,
    data_slot: &mut Value,
    report: &mut MigrationReport,
) {
    let mut world = World::new();

    // Ask the engine's own real load path first: if this component already
    // hydrates as stored (whatever shape that happens to be), it's correct
    // -- don't touch it. This also makes re-running this tool on an
    // already-repaired file a safe no-op.
    let entity = world.spawn();
    match hydrate_world_component_for_class(class_name, &mut world, entity, &*data_slot) {
        Ok(true) => {
            report.already_ok += 1;
            return;
        }
        Ok(false) => {
            // Not a `#[register_world_component]` class at all (e.g. a
            // legacy JSON-only class like `ColliderDescriptor`) -- this
            // tool has no trusted way to know its correct shape.
            report
                .skipped_unregistered
                .push(format!("{class_name} on {obj_id}"));
            return;
        }
        Err(_) => {
            // Registered, but doesn't load as stored -- attempt repair below.
        }
    }

    let Some(rebuilt) = rebuild_component_json(class_name, data_slot) else {
        report.failed.push(format!(
            "{class_name} on {obj_id}: could not reconstruct from stored data"
        ));
        return;
    };

    // Verify the repaired shape through the exact same real hydrate path
    // before trusting it -- never write a "fix" that would just fail again
    // the next time the level loads.
    let verify_entity = world.spawn();
    match hydrate_world_component_for_class(class_name, &mut world, verify_entity, &rebuilt) {
        Ok(true) => {
            *data_slot = rebuilt;
            report.migrated += 1;
        }
        _ => {
            report.failed.push(format!(
                "{class_name} on {obj_id}: still doesn't load correctly after repair attempt -- left unchanged"
            ));
        }
    }
}

/// Rebuilds `class_name`'s data from `data`'s leaves via the reflection
/// registry: a fresh `Default` instance, each leaf applied through its real
/// setter (best-effort -- a leaf that fails to parse just keeps that
/// field's default rather than aborting the whole component), then the
/// class's own real `to_json()` for the correct nested/typed shape.
fn rebuild_component_json(class_name: &str, data: &Value) -> Option<Value> {
    let mut instance = REGISTRY.create_instance(class_name)?;
    let props = instance.get_properties();
    let obj = data.as_object()?;
    for prop in &props {
        if let Some(raw) = obj.get(prop.name) {
            if let Ok(boxed) =
                RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(prop.type_info, raw.clone())
            {
                (prop.setter)(instance.as_mut(), boxed);
            }
        }
    }
    instance.to_json().ok()
}
