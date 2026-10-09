//! Generates Rust for every conformance fixture module, so the tests run
//! the generated code, compiled by rustc in the profile under test.

use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/fixtures.rs"]
mod fixtures;

fn identifier(name: &str) -> String {
    let mut id = String::from("m_");
    id.extend(name.chars().map(|c| {
        if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            '_'
        }
    }));
    id
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=src/fixtures.rs");
    println!("cargo:rerun-if-changed=fixtures");

    let mut modules = fixtures::hand_written();
    let mut json: Vec<_> = std::fs::read_dir(manifest_dir.join("fixtures"))
        .map(|dir| dir.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    json.sort();
    for path in json
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".module.json"))
    {
        let text = std::fs::read_to_string(&path).expect("fixture is readable");
        modules.push(
            pulsar_script_vm::Module::from_json(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        );
    }

    let mut out = String::new();
    let mut arms = String::new();
    for module in &modules {
        let id = identifier(&module.name);
        let source = pulsar_script_codegen::generate(module)
            .unwrap_or_else(|e| panic!("fixture `{}`: {e}", module.name));
        let _ = writeln!(
            out,
            "#[allow(clippy::all, dead_code)]\npub mod {id} {{\n{source}\n}}\n"
        );
        let _ = writeln!(
            arms,
            "        {:?} => Some({id}::link(registry, events, policy)),",
            module.name
        );
    }
    let _ = write!(
        out,
        "/// Link the generated code for fixture `name`; `None` if there is none.\n\
         pub fn link(\n    name: &str,\n    registry: &pulsar_script_vm::NativeRegistry,\n    events: Option<&dyn pulsar_script_vm::EventCatalog>,\n    policy: &pulsar_script_vm::CapabilityPolicy,\n) -> Option<Result<pulsar_script_vm::Program, pulsar_script_vm::LinkError>> {{\n    match name {{\n{arms}        _ => None,\n    }}\n}}\n"
    );
    std::fs::write(out_dir.join("generated.rs"), out).expect("write generated.rs");
}
