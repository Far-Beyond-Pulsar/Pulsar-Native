//! What a class implements, as the script runtime sees it.
//!
//! The editor answers this from the project's trait index; a running game
//! asks per entity, so this module resolves it from the class itself:
//!
//! - a class's **declared traits** are the ids in its Blueprint's
//!   `blueprint_metadata` (`<class dir>/graph_save.json`), resolved against
//!   the project's [`TraitDb`] ids first, paths as the pre-id fallback, so a
//!   renamed or moved trait still resolves;
//! - its **effective traits** add everything those extend (supertraits);
//! - a trait is named by id (or anything [`TraitDb::resolve`] accepts); the
//!   answer never depends on a file path or a name the caller typed.
//!
//! The database is the persisted index (`.pulsar/blueprint_trait_index.json`)
//! when the project has one, else the engine's built-in traits alone, which is
//! enough for the role traits. A class whose Blueprint cannot be read (content
//! that ships without it) is [`TraitCheck::Unknown`], not a "no": the role
//! binding is then the only claim there is and callers decide how far to
//! trust it.
//!
//! The **contract** of a class and a trait is the existing conformance check
//! ([`engine_fs::trait_conformance::check`]) run on what the compiled module
//! really provides: a handler for the event `<Class>.<method>`. A required
//! method without one is a violation; a method with a default body is
//! optional and reacts to nothing when no node is placed.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use engine_fs::trait_conformance::{
    self, normalize_name, signature_text, IssueKind, ProvidedMethod, ProvidedParam, ProvidedReturn,
    Severity,
};
use engine_fs::{BlueprintTraitIndex, TraitDb};
use pulsar_class::{ClassEntry, ClassId};

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
    /// Declared trait ids; `None` when the Blueprint could not be read.
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
    /// A handler ran.
    Called,
    /// The method is optional (it has a default body) and the class places
    /// no node for it: no reaction, and that is fine.
    NotHandled,
    /// The entity runs no script (no compiled module, or not started).
    NotRunning,
    /// The class does not honour the trait: it does not implement it, the
    /// trait has no such method, or a required method has no handler. The
    /// driver has reported it.
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

#[derive(Default)]
pub(super) struct RuntimeTraits {
    db: Option<TraitDb>,
    classes: HashMap<ClassId, Arc<ClassTraits>>,
    contracts: HashMap<(ClassId, String), Contract>,
}

impl RuntimeTraits {
    /// Forget everything derived from classes and traits: a class changed.
    pub fn clear(&mut self) {
        self.db = None;
        self.classes.clear();
        self.contracts.clear();
    }

    fn db(&mut self, project_root: &Path) -> &TraitDb {
        self.db.get_or_insert_with(|| {
            BlueprintTraitIndex::load(project_root)
                .map(|index| index.db().clone())
                .unwrap_or_else(|_| TraitDb::builtin())
        })
    }

    /// The id of the trait `reference` names (an id, a path or a unique name).
    pub fn trait_id(&mut self, project_root: &Path, reference: &str) -> Option<String> {
        self.db(project_root)
            .resolve(reference)
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
        self.db(project_root)
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
        let source = entry.dir.join("graph_save.json");
        let bytes = engine_fs::virtual_fs::read_file(&source).ok();
        let db = self.db(project_root);
        let declared = bytes
            .as_deref()
            .and_then(|bytes| engine_fs::declared_traits(db, bytes));
        let traits = match declared {
            Some(declared) => ClassTraits {
                effective: db.with_ancestors(&declared.implemented_trait_ids),
                has_unresolved: !declared.unresolved.is_empty(),
                declared: Some(declared.implemented_trait_ids),
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

    /// The class's contract under `trait_id`. `handlers` are the events its
    /// compiled module handles, as `(event name, parameter count)` (only
    /// asked for the first time);
    /// `runtime_class` is the module's name, which prefixes those events.
    pub fn contract(
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
            let db = self.db(project_root);
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
                // Everything the class declares owes its methods; an unreadable
                // Blueprint claims just the trait asked about.
                let claimed = match &traits.declared {
                    Some(declared) => declared.clone(),
                    None => vec![trait_id.to_owned()],
                };
                for issue in trait_conformance::check(db, &claimed, &provided, Severity::Error) {
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
            self.contracts.insert(
                key.clone(),
                Contract { check, missing },
            );
        }
        &self.contracts[&key]
    }
}
