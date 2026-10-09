//! The Rust export: a script module as an `Actor` source file.
//!
//! [`generate_actor`] writes the file a game project compiles as a class's
//! `events/events.rs`: the module's generated code (see the crate docs), an
//! `Actor` whose callbacks forward to
//! `pulsar_game::scripting::export::ExportedScript`, and the prefab
//! component hydration an actor does at `begin_play`.
//!
//! The actor holds its script state (the instance's variables and waiting
//! calls) in the struct, so two actors of one class never share it, and
//! every function runs on the VM with the same budget, call-depth limit and
//! error reporting as an interpreted class. Waits use the clock described
//! in `ExportedScript`'s docs.

use std::fmt::Write as _;

use pulsar_script_vm::{Module, SubscriptionScope};

use crate::{generate_with, CodegenError, Options};

/// A prefab component the actor makes sure exists on its entity.
#[derive(Clone, Debug)]
pub struct ComponentSpec {
    /// A class in the reflection registry.
    pub class_name: String,
    /// The prefab's property values, as JSON text.
    pub property_defaults_json: String,
    pub enabled: bool,
}

/// Why a class could not be exported.
#[derive(Debug)]
pub enum ExportError {
    Codegen(CodegenError),
    /// The class uses something an exported actor cannot do. Exporting it
    /// anyway would silently drop behaviour, so nothing is generated.
    Unsupported(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codegen(error) => error.fmt(f),
            Self::Unsupported(what) => write!(f, "cannot export as Rust: {what}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<CodegenError> for ExportError {
    fn from(error: CodegenError) -> Self {
        Self::Codegen(error)
    }
}

/// `MyClass`, `my-class`, `my class` -> `my_class`.
pub fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_upper = false;
    for (i, ch) in name.char_indices() {
        if ch.is_uppercase() {
            if i != 0 && !prev_upper && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_upper = true;
        } else if ch == '-' || ch == ' ' {
            out.push('_');
            prev_upper = false;
        } else {
            out.push(ch);
            prev_upper = false;
        }
    }
    out
}

/// `my_class`, `my-class`, `my class` -> `MyClass`.
pub fn pascal_case(name: &str) -> String {
    name.split(['_', '-', ' '])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}

const VM: &str = "pulsar_game::scripting::export::vm";

/// The `events.rs` source for class `class_name`, compiled to `module`.
pub fn generate_actor(
    class_name: &str,
    module: &Module,
    components: &[ComponentSpec],
) -> Result<String, ExportError> {
    if let Some((index, subscription)) = module.subscriptions.iter().enumerate().next() {
        let event = subscription.event.to_string();
        if let SubscriptionScope::Component(variable) = subscription.scope {
            let source = module.variables.get(variable as usize).map_or_else(
                || format!("component-reference variable {variable}"),
                |value| {
                    format!(
                        "component-reference variable {variable} (`{}`: {})",
                        value.name, value.ty
                    )
                },
            );
            return Err(ExportError::Unsupported(format!(
                "`{class_name}` subscription #{index} for event `{event}` uses {source}; exported actors do not \
                 have the runtime event context needed to resolve that live reference, queue delivery, and dispatch \
                 its handler. Use the VM compile target for this class."
            )));
        }
        return Err(ExportError::Unsupported(format!(
            "`{class_name}` subscription #{index} for event `{event}` uses the `{:?}` scope; exported actors do \
             not have the runtime event context needed to queue delivery and dispatch its handler. Use the VM \
             compile target for this class.",
            subscription.scope
        )));
    }
    if let Some(declaration) = module.events.first() {
        return Err(ExportError::Unsupported(format!(
            "`{class_name}` declares event `{}`; exported actors do not have the session event host needed to \
             register its descriptor and publish payloads. Use the VM compile target for this class.",
            declaration.name
        )));
    }
    let ident = snake_case(class_name);
    let ty = pascal_case(class_name);
    let class_source = generate_with(
        module,
        &Options {
            vm_crate: VM.to_owned(),
        },
    )?;
    let enabled: Vec<&ComponentSpec> = components.iter().filter(|c| c.enabled).collect();

    let mut begin_play = String::new();
    if !enabled.is_empty() {
        begin_play.push_str("        Self::__init_components(_entity, _world);\n");
        begin_play.push_str("        Self::__run_component_begin_plays(_entity, _world);\n");
    }
    begin_play.push_str("        self.script.begin_play(program(), _entity, _world);\n");

    let mut component_helpers = String::new();
    if !enabled.is_empty() {
        let mut init_body = String::new();
        let mut begin_plays_body = String::new();
        for component in &enabled {
            let class = &component.class_name;
            let json = component
                .property_defaults_json
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let _ = write!(
                init_body,
                r#"        if pulsar_world_registry::instances::resolve_instance(world, entity, "{class}", 0).is_none() {{
            // The class default, decoded once for every actor of this class.
            static __DEFAULT: pulsar_world_registry::instances::DefaultCache =
                pulsar_world_registry::instances::DefaultCache::new();
            if let Err(__e) = pulsar_world_registry::instances::attach_cached_default(
                world, entity, "{class}", "{json}", &__DEFAULT,
            ) {{
                tracing::error!("blueprint `{ident}`: attaching {class} failed: {{__e}}");
            }}
        }}
"#
            );
            let _ = write!(
                begin_plays_body,
                r#"        if pulsar_reflection::REGISTRY.get_method("{class}", "begin_play").is_some() {{
            if let Err(__e) = pulsar_world_registry::dispatch::invoke_component_method(
                world,
                entity,
                "{class}",
                0,
                "begin_play",
                vec![],
            ) {{
                tracing::error!("blueprint `{ident}`: {class}::begin_play failed: {{__e}}");
            }}
        }}
"#
            );
        }
        let _ = write!(
            component_helpers,
            r#"
impl {ty} {{
    /// Ensure every enabled prefab component is attached to the actor's
    /// scene object in the LIVE world (#651), as a component instance
    /// (Pulsar-Native#1035).
    ///
    /// Idempotent and scene-respecting: a component is attached only when
    /// the object has no instance of its class, so per-instance values the
    /// scene already attached win over the defaults baked in at compile
    /// time. Failures log and continue: one bad component never blocks the
    /// actor.
    pub fn __init_components(entity: Entity, world: &mut World) {{
{init_body}    }}

    /// Call `begin_play` on each declared component class that implements
    /// it, through the same live-world dispatcher graph nodes use.
    pub fn __run_component_begin_plays(entity: Entity, world: &mut World) {{
{begin_plays_body}    }}
}}
"#
        );
    }

    let class_source: String = class_source
        .lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_owned()
            } else {
                format!("    {line}\n")
            }
        })
        .collect();

    Ok(format!(
        r#"//! Blueprint actor: `{ident}`
//! Generated by pulsar_script_codegen. Do not hand-edit: changes are overwritten.
//!
//! The class's functions are generated Rust that runs on the engine's script
//! VM (same instances, natives, instruction budget and call-depth limit as
//! an interpreted class). Each actor owns its variables and waiting calls.
//! Component access goes through `pulsar_world_registry`'s dispatcher
//! against the LIVE world each `Actor` callback receives (#651).

use pulsar_game::prelude::*;
use pulsar_game::scripting::export::{{vm, ExportedScript}};
use engine_class_derive::EngineClass;

const CLASS: &str = "{ident}";

#[derive(EngineClass)]
pub struct {ty} {{
    script: ExportedScript,
}}

impl {ty} {{
    pub fn new() -> Self {{
        Self {{ script: ExportedScript::new(CLASS, program()) }}
    }}

    /// The value of script variable `name` (for tests and tooling).
    pub fn variable(&self, name: &str) -> Option<vm::Value> {{
        self.script.variable(program(), name).cloned()
    }}
}}

impl Default for {ty} {{
    fn default() -> Self {{
        Self::new()
    }}
}}

/// A clone is a new actor of the class, with fresh script state.
impl Clone for {ty} {{
    fn clone(&self) -> Self {{
        Self::new()
    }}
}}
{component_helpers}
impl Actor for {ty} {{
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {{
{begin_play}    }}

    fn end_play(&mut self, _entity: Entity, _world: &mut World) {{
        self.script.end_play(program(), _entity, _world);
    }}

    // Signature MUST match the pinned `pulsar_scenedb::Actor` exactly.
    // `Actor::tick` is deliberately TIME-FREE there (see that trait's doc);
    // the script reads time from `ExportedScript`'s clock. Do not add
    // parameters here without changing SceneDB first: a mismatch is E0053 in
    // every generated project. Guarded by pulsar_game's
    // `blueprint_codegen_drift` probes (Pulsar-Native#652).
    fn tick(&mut self, _entity: Entity, _world: &mut World) {{
        self.script.tick(program(), _entity, _world);
    }}
}}

/// The class linked against the engine's natives, shared by every actor.
fn program() -> &'static vm::Program {{
    static NATIVES: std::sync::OnceLock<vm::NativeRegistry> = std::sync::OnceLock::new();
    static PROGRAM: std::sync::OnceLock<vm::Program> = std::sync::OnceLock::new();
    PROGRAM.get_or_init(|| {{
        let natives = NATIVES.get_or_init(vm::NativeRegistry::with_engine_natives);
        class::link(natives, None, &vm::CapabilityPolicy::allow_all())
            .unwrap_or_else(|error| panic!("blueprint `{{CLASS}}` does not link: {{error}}"))
    }})
}}

#[allow(clippy::all, dead_code)]
mod class {{
{class_source}}}
"#
    ))
}

/// The files of one class's `events/` directory in a project that has none
/// yet (the layout the project builder scans). Existing projects only ever
/// need `events.rs`.
pub fn class_files(
    class_name: &str,
    module: &Module,
    components: &[ComponentSpec],
) -> Result<Vec<(String, String)>, ExportError> {
    let ident = snake_case(class_name);
    let base = format!("src/classes/{ident}");
    Ok(vec![
        (
            format!("{base}/mod.rs"),
            format!("//! Blueprint class module: `{ident}`\n//! Generated. Do not hand-edit: changes are overwritten.\n\npub mod events;\npub use events::*;\n"),
        ),
        (
            format!("{base}/events/mod.rs"),
            "//! Blueprint event module (generated).\n\npub mod events;\npub use events::*;\n".to_owned(),
        ),
        (format!("{base}/events/events.rs"), generate_actor(class_name, module, components)?),
    ])
}
