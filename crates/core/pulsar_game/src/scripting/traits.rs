//! The one trait dispatcher: what an entity implements, and calling a trait
//! method on it, for script classes and native components alike.
//!
//! An entity implements a trait through its **script class** (a compiled
//! Blueprint, or any source whose implementers bind to a script class) or
//! through a **native component** attached to it
//! (`pulsar_traits::NativeTraitImpl`). Both come from the project's one trait
//! index (`pulsar_traits::TraitIndex`): each implementer carries the binding
//! the runtime reaches it through ([`Binding`]), so this module never reads an
//! editor format. The index is the stored one when the project has it, else
//! what the engine and the enabled plugins bring alone (`TraitIndex::live`,
//! enough for the role traits and every native implementer).
//!
//! - A class's **declared traits** are its implementer entry's
//!   (`Binding::ScriptClass` with the class's project-relative folder); its
//!   **effective traits** add everything those extend. A class the index does
//!   not list (content that ships without the index) is [`TraitCheck::Unknown`],
//!   not a "no": the role binding is then the only claim there is.
//! - A trait is named by id (or anything `TraitDb::resolve` accepts); the
//!   answer never depends on a file path or a name the caller typed.
//! - **Calling** a method ([`RuntimeTraits::dispatch`]) runs the script
//!   class's handler for the event `<Class>.<method>` and each implementing
//!   component's bound method (`pulsar_traits::sources::bound_method`), through
//!   the world registry, the same function a script calling the component
//!   method runs.
//!
//! The **contract** of a script class and a trait is the conformance check
//! (`pulsar_traits::conformance::check`) on what the compiled module really
//! provides: a required method without a handler is a violation; a method with
//! a default body is optional and reacts to nothing when no node is placed.
//! Native implementers are checked when the index loads (its problems).
//!
//! Unreal works the same way: one interface call (`Execute_<Method>`)
//! dispatches to a C++ or a Blueprint implementation, and the interfaces live
//! on the generated class, so a cooked game needs no graph.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use pulsar_class::{ClassEntry, ClassId};
use pulsar_scenedb::{Entity, World};
use pulsar_script_runtime::ScriptRuntime;
use pulsar_traits::conformance::{
    self, normalize_name, signature_text, IssueKind, ProvidedMethod, ProvidedParam, ProvidedReturn,
    Severity,
};
use pulsar_traits::sources::{bound_method, component_implementer_id};
use pulsar_traits::{Binding, TraitIndex};

use super::driver::DriverReport;

/// Whether a class implements a trait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraitCheck {
    Implements,
    DoesNot,
    /// The class's declarations cannot be read.
    Unknown,
}

/// The traits one class declares.
pub(super) struct ClassTraits {
    /// Declared trait ids; `None` when the index does not list the class.
    declared: Option<Vec<String>>,
    /// Declared traits and everything they extend.
    effective: BTreeSet<String>,
    /// Declared references that name no trait now.
    has_unresolved: bool,
}

/// A class's obligations under one trait, judged on its compiled handlers.
pub(super) struct Contract {
    pub check: TraitCheck,
    /// Required methods (normalized name) with no handler, and why that is a
    /// violation.
    missing: Vec<(String, String)>,
}

/// What calling a trait method on an entity did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookOutcome {
    /// An implementation ran.
    Called,
    /// The method is optional (it has a default body) and no implementer
    /// provides it: no reaction, and that is fine.
    NotHandled,
    /// The entity runs no script and no component of it implements the
    /// trait.
    NotRunning,
    /// The entity does not honour the trait: nothing on it implements it, the
    /// trait has no such method, or a required method has no handler. The
    /// dispatcher has reported it.
    Violation,
}

impl Contract {
    /// The violation for `method`, when it is required and unhandled.
    pub fn violation(&self, method: &str) -> Option<&str> {
        let wanted = normalize_name(method);
        self.missing
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, message)| message.as_str())
    }

    /// Every violation of the contract.
    pub fn violations(&self) -> Vec<String> {
        self.missing.iter().map(|(_, m)| m.clone()).collect()
    }
}

/// A method of a trait, found through its id.
pub(super) struct TraitMethod {
    /// The method's name in the trait, which is what the class's event
    /// `<Class>.<name>` is called.
    pub name: String,
    pub has_default: bool,
}

/// The script side of a dispatch target: a running instance of a class.
pub(super) struct ScriptTarget {
    pub entry: ClassEntry,
    /// The runtime's instance id.
    pub instance: String,
    /// The compiled module's name, which prefixes its events.
    pub runtime_class: String,
}

/// Where a trait method is called.
pub(super) struct DispatchTarget {
    pub entity: Entity,
    pub script: Option<ScriptTarget>,
}

#[derive(Default)]
pub(super) struct RuntimeTraits {
    index: Option<TraitIndex>,
    classes: HashMap<ClassId, Arc<ClassTraits>>,
    contracts: HashMap<(ClassId, String), Contract>,
}

impl RuntimeTraits {
    /// Forget everything derived from classes and traits: a class changed.
    pub fn clear(&mut self) {
        self.index = None;
        self.classes.clear();
        self.contracts.clear();
    }

    fn index(&mut self, project_root: &Path) -> &TraitIndex {
        self.index.get_or_insert_with(|| {
            TraitIndex::load(project_root).unwrap_or_else(|_| TraitIndex::live(project_root))
        })
    }

    /// The id of the trait `reference` names (an id, a path or a unique name).
    pub fn trait_id(&mut self, project_root: &Path, reference: &str) -> Option<String> {
        self.index(project_root)
            .resolve_trait(reference)
            .map(|record| record.id.clone())
    }

    /// The method `name` of trait `trait_id`, supertraits included.
    pub fn method(
        &mut self,
        project_root: &Path,
        trait_id: &str,
        name: &str,
    ) -> Option<TraitMethod> {
        let wanted = normalize_name(name);
        self.index(project_root)
            .db()
            .required_methods(trait_id)
            .into_iter()
            .find(|(_, method)| normalize_name(&method.name) == wanted)
            .map(|(_, method)| TraitMethod {
                name: method.name.clone(),
                has_default: method.has_default,
            })
    }

    fn class(&mut self, project_root: &Path, entry: &ClassEntry) -> Arc<ClassTraits> {
        if let Some(found) = self.classes.get(&entry.id) {
            return Arc::clone(found);
        }
        let dir = entry
            .dir
            .strip_prefix(project_root)
            .unwrap_or(&entry.dir)
            .to_string_lossy()
            .replace('\\', "/");
        let index = self.index(project_root);
        let traits = match index.entry_for_binding(&Binding::ScriptClass { dir }) {
            Some(implementer) => ClassTraits {
                effective: index
                    .db()
                    .with_ancestors(&implementer.implemented_trait_ids),
                has_unresolved: !implementer.unresolved.is_empty(),
                declared: Some(implementer.implemented_trait_ids.clone()),
            },
            None => ClassTraits {
                declared: None,
                effective: BTreeSet::new(),
                has_unresolved: false,
            },
        };
        let traits = Arc::new(traits);
        self.classes.insert(entry.id.clone(), Arc::clone(&traits));
        traits
    }

    /// Does `entry`'s class implement the trait `reference` names?
    pub fn check(
        &mut self,
        project_root: &Path,
        entry: &ClassEntry,
        reference: &str,
    ) -> TraitCheck {
        let Some(trait_id) = self.trait_id(project_root, reference) else {
            return TraitCheck::DoesNot;
        };
        let traits = self.class(project_root, entry);
        if traits.declared.is_none() {
            return TraitCheck::Unknown;
        }
        if traits.effective.contains(&trait_id) {
            TraitCheck::Implements
        } else if traits.has_unresolved {
            // It declares something this database does not know; that could
            // be a trait extending this one.
            TraitCheck::Unknown
        } else {
            TraitCheck::DoesNot
        }
    }

    /// The native component classes on `entity` that implement `trait_id`,
    /// with each one's index among the entity's instances of its class.
    fn native_implementers(
        &mut self,
        project_root: &Path,
        world: &World,
        entity: Entity,
        trait_id: &str,
    ) -> Vec<(String, u32)> {
        let db = self.index(project_root).db();
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::new();
        for record in pulsar_world_registry::instances::component_metadata_records(world, entity) {
            let class = record.class_name;
            let at = seen.entry(class.clone()).or_insert(0);
            let index = *at;
            *at += 1;
            if db.implements(&component_implementer_id(&class), trait_id) {
                out.push((class, index));
            }
        }
        out
    }

    /// The class's contract under `trait_id`. `handlers` are the events its
    /// compiled module handles, as `(event name, parameter count)` (only
    /// asked for the first time); `runtime_class` is the module's name, which
    /// prefixes those events.
    fn contract(
        &mut self,
        project_root: &Path,
        entry: &ClassEntry,
        trait_id: &str,
        runtime_class: &str,
        handlers: impl FnOnce() -> Vec<(String, usize)>,
    ) -> &Contract {
        let key = (entry.id.clone(), trait_id.to_owned());
        if !self.contracts.contains_key(&key) {
            let check = self.check(project_root, entry, trait_id);
            let traits = self.class(project_root, entry);
            let db = self.index(project_root).db();
            let mut missing = Vec::new();
            if check != TraitCheck::DoesNot {
                let prefix = format!("{runtime_class}.");
                let provided: Vec<ProvidedMethod> = handlers()
                    .iter()
                    .filter_map(|(event, params)| {
                        Some(ProvidedMethod {
                            name: event.strip_prefix(&prefix)?.to_owned(),
                            params: (0..*params)
                                .map(|index| ProvidedParam {
                                    name: format!("arg{index}"),
                                    type_name: None,
                                })
                                .collect(),
                            returns: ProvidedReturn::Unknown,
                        })
                    })
                    .collect();
                // Everything the class declares owes its methods; a class the
                // index does not list claims just the trait asked about.
                let claimed = match &traits.declared {
                    Some(declared) => declared.clone(),
                    None => vec![trait_id.to_owned()],
                };
                for issue in conformance::check(db, &claimed, &provided, Severity::Error) {
                    let (IssueKind::MissingMethod, Some(method)) = (issue.kind, &issue.method)
                    else {
                        continue;
                    };
                    let name = normalize_name(method);
                    let signature = claimed
                        .iter()
                        .flat_map(|id| db.required_methods(id))
                        .find(|(_, m)| normalize_name(&m.name) == name)
                        .map_or_else(|| method.clone(), |(_, m)| signature_text(m));
                    missing.push((
                        name,
                        format!(
                            "class '{}' declares trait {} but its compiled script has no handler \
                             for the required method `{signature}`; recompile the class",
                            entry.name, issue.trait_name,
                        ),
                    ));
                }
            }
            self.contracts
                .insert(key.clone(), Contract { check, missing });
        }
        &self.contracts[&key]
    }

    /// Call the method `method` of the trait `trait_ref` names on `target`:
    /// its script class's handler and every implementing component's bound
    /// method.
    ///
    /// The method is found through the trait's id, supertraits included, so a
    /// renamed trait or a method inherited from a supertrait resolve the same
    /// way. What a missing implementation means depends on the contract
    /// ([`HookOutcome`]): an optional method (with a default body) nothing
    /// provides is no reaction; a required one is a violation. What is wrong
    /// is added to `problems` (the driver reports each once per session),
    /// script errors to `report`.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &mut self,
        project_root: &Path,
        runtime: &mut ScriptRuntime,
        world: &mut World,
        target: &DispatchTarget,
        trait_ref: &str,
        method: &str,
        args: &[pulsar_script_vm::Value],
        report: &mut DriverReport,
        problems: &mut Vec<String>,
    ) -> HookOutcome {
        let Some(trait_id) = self.trait_id(project_root, trait_ref) else {
            let message = format!(
                "trait '{trait_ref}' is not known to this project; `{method}` was not called"
            );
            problems.push(message);
            return HookOutcome::Violation;
        };
        let Some(declared) = self.method(project_root, &trait_id, method) else {
            let message = format!("trait {trait_ref} has no method `{method}`");
            problems.push(message);
            return HookOutcome::Violation;
        };
        let natives = self.native_implementers(project_root, world, target.entity, &trait_id);
        let mut called = false;
        let mut script_violation = false;
        let mut script_implements = false;

        if let Some(script) = &target.script {
            let rt = &*runtime;
            let contract = self.contract(
                project_root,
                &script.entry,
                &trait_id,
                &script.runtime_class,
                || {
                    rt.subscriptions(&script.runtime_class)
                        .into_iter()
                        .flatten()
                        .filter_map(|sub| Some((sub.event_name.clone()?, sub.params)))
                        .collect()
                },
            );
            let check = contract.check;
            let required_unhandled = contract.violation(method).is_some();
            let violations = contract.violations();
            problems.extend(violations);
            script_implements = check != TraitCheck::DoesNot;
            if script_implements {
                let qualified = format!("{}.{}", script.runtime_class, declared.name);
                let handlers: Vec<(pulsar_script_vm::FuncId, usize)> = runtime
                    .subscriptions(&script.runtime_class)
                    .into_iter()
                    .flatten()
                    .filter(|sub| sub.event_name.as_deref() == Some(qualified.as_str()))
                    .map(|sub| (sub.handler, sub.params))
                    .collect();
                if handlers.is_empty() {
                    script_violation = required_unhandled && !declared.has_default;
                    if !script_violation {
                        tracing::debug!(%qualified, "optional trait method has no event node in the Blueprint");
                    }
                }
                for (handler, params) in handlers {
                    let args = &args[..params.min(args.len())];
                    if let Err(error) =
                        runtime.call_function(&script.instance, handler, args, world)
                    {
                        tracing::warn!("{error}");
                        report.script_errors.push(error);
                    }
                    called = true;
                }
            }
        }

        for (class, index) in &natives {
            let Some(bound) = bound_method(class, &trait_id, &declared.name) else {
                continue;
            };
            match call_component_method(world, target.entity, class, *index, bound, args) {
                Ok(()) => called = true,
                Err(error) => {
                    let message = format!(
                        "component '{class}' implements {trait_ref} but `{bound}` failed: {error}"
                    );
                    problems.push(message);
                }
            }
        }

        if called {
            return HookOutcome::Called;
        }
        if script_violation {
            return HookOutcome::Violation;
        }
        if !script_implements && natives.is_empty() {
            if target.script.is_none() {
                return HookOutcome::NotRunning;
            }
            let name = target
                .script
                .as_ref()
                .map_or("the entity", |script| script.entry.name.as_str());
            let message =
                format!("class '{name}' does not implement trait {trait_ref}; its `{method}` was not called");
            problems.push(message);
            return HookOutcome::Violation;
        }
        HookOutcome::NotHandled
    }
}

/// Call a reflected component method with script values, converted through
/// the script VM's type bindings (as a script calling it would).
fn call_component_method(
    world: &mut World,
    entity: Entity,
    class: &str,
    index: u32,
    method: &str,
    args: &[pulsar_script_vm::Value],
) -> Result<(), String> {
    let meta = pulsar_reflection::REGISTRY
        .get_method(class, method)
        .ok_or_else(|| format!("no method `{method}` is registered"))?;
    let types = pulsar_script_vm::TypeRegistry::global();
    let boxed = meta
        .params
        .iter()
        .zip(args)
        .map(|(param, value)| {
            types
                .binding(param.type_info.type_id)
                .ok_or_else(|| format!("parameter `{}` is not script-visible", param.name))?
                .from_value(value)
        })
        .collect::<Result<Vec<_>, String>>()?;
    pulsar_world_registry::invoke_component_method(world, entity, class, index, method, boxed)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
