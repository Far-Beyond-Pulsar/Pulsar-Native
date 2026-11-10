//! Where implementers come from: one registration seam for every kind.
//!
//! A [`TraitImplementerSource`] tells the [`TraitIndex`](crate::TraitIndex)
//! how to find one kind of implementer:
//!
//! - a **kind** id, which is what [`TraitImplementer::kind`] says
//!   (`pulsar_reflection::TraitImplementer`);
//! - a **scan** from project files to implementers ([`claims`] picks the
//!   files, [`scan`] reads one: its id, the traits it declares, and how the
//!   runtime reaches it), and/or a **list** of implementers that are not files
//!   (native components);
//! - the **binding** each implementer carries ([`Binding`]): the callables
//!   the runtime's dispatcher (`pulsar_game`'s `RuntimeTraits`) resolves its
//!   methods to.
//!
//! The Blueprint plugin registers the Blueprint source (its root crate, an
//! editor tool: Blueprints compile to script classes, so the runtime meets
//! them as [`Binding::ScriptClass`] through the persisted index and needs no
//! Blueprint parser). This crate registers the native source over every
//! enabled [`NativeTraitImpl`]: a plugin's component implements a trait by
//! binding each trait method onto one of its registered methods
//! (`pulsar_reflection::ComponentMethodRegistration`), checked with the same
//! conformance rules ([`native_conformance`]). A plugin's own asset kind (a
//! behaviour tree, a state machine) registers a source and gets indexing,
//! conformance and dispatch with no other change.
//!
//! Sources are origin-gated contributions (`pulsar_plugins::registered`):
//! a disabled plugin's source and its native implementers are absent, and
//! what referenced them sees "Missing trait".
//!
//! [`claims`]: TraitImplementerSource::claims
//! [`scan`]: TraitImplementerSource::scan

use serde::{Deserialize, Serialize};

use crate::conformance::{self, ConformanceIssue, ProvidedMethod, ProvidedParam, ProvidedReturn, Severity};
use crate::db::TraitDb;

/// What an implementer declares about traits, as authored: ids (current)
/// and project-relative trait paths (the pre-id format, a fallback hint).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declared {
    pub ids: Vec<String>,
    pub paths: Vec<String>,
}

/// How the runtime reaches an implementer's methods.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Binding {
    /// A script class (`classes/<Name>/`, project-relative): a trait method
    /// is the class's handler for the event `<Class>.<method>`.
    ScriptClass { dir: String },
    /// A native component class: a trait method is the component method it
    /// is bound to ([`NativeTraitImpl::methods`]).
    Component { class: String },
}

/// One implementer a source found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    /// A project-relative asset path for a file, `component:<Class>` for a
    /// native component.
    pub id: String,
    pub declared: Declared,
    pub binding: Option<Binding>,
}

/// One kind of implementer (see the module docs).
pub struct TraitImplementerSource {
    pub origin: &'static str,
    /// Stable kind id, prefixed by its owner (`pulsar.blueprints.blueprint`).
    pub kind: &'static str,
    pub label: &'static str,
    /// Whether this source reads the project file at this project-relative
    /// path. `None`: it reads no files.
    pub claims: Option<fn(&str) -> bool>,
    /// Read one claimed file: `None` for one that is malformed or not an
    /// implementer after all.
    pub scan: Option<fn(&str, &[u8]) -> Option<Scanned>>,
    /// Implementers that are not files, listed whenever the index loads.
    pub list: Option<fn() -> Vec<Scanned>>,
}

impl pulsar_plugins::Contribution for TraitImplementerSource {
    const CATEGORY: &'static str = "traits.implementer_sources";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.kind.to_owned()
    }

    fn label(&self) -> String {
        self.label.to_owned()
    }
}

pulsar_plugins::registration!(TraitImplementerSource);

/// The enabled sources, in kind order.
pub fn sources() -> Vec<&'static TraitImplementerSource> {
    let mut all: Vec<_> = pulsar_plugins::registered::<TraitImplementerSource>().collect();
    all.sort_by_key(|source| source.kind);
    all
}

/// The source that reads the file at `relative`, if any.
pub fn source_for(relative: &str) -> Option<&'static TraitImplementerSource> {
    sources()
        .into_iter()
        .find(|source| source.claims.is_some_and(|claims| claims(relative)))
}

// ── Native components ────────────────────────────────────────────────────────

/// The kind of native component implementers.
pub const NATIVE_KIND: &str = "pulsar.native.component";

/// A native component implements a trait: each trait method is bound to one
/// of the component's registered methods (the by-name methods scripts and
/// the editor call, `ComponentMethodRegistration`), so a script calling
/// `health.respawn()` and the dispatcher calling `Respawnable.respawn` run the
/// same function. Submitted by the plugin's root crate ([`implements!`]).
///
/// [`implements!`]: crate::implements
pub struct NativeTraitImpl {
    pub origin: &'static str,
    /// The component class (`HealthComponent`).
    pub class: &'static str,
    /// The trait's id (`pulsar.health.Respawnable`).
    pub trait_id: &'static str,
    /// `(trait method, component method)`; a trait method not listed binds
    /// to the component method of the same name.
    pub methods: &'static [(&'static str, &'static str)],
}

impl pulsar_plugins::Contribution for NativeTraitImpl {
    const CATEGORY: &'static str = "traits.native_implementers";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        format!("{}: {}", self.class, self.trait_id)
    }
}

pulsar_plugins::registration!(NativeTraitImpl);

/// Declare that a component class implements a trait, from a plugin's root
/// crate:
///
/// ```ignore
/// pulsar_traits::implements!(HealthComponent: "pulsar.health.Respawnable");
/// pulsar_traits::implements!(HealthComponent: "pulsar.health.Damageable" {
///     take_damage => apply_damage,
/// });
/// ```
#[macro_export]
macro_rules! implements {
    ($class:ident : $trait_id:literal $({ $($method:ident => $bound:ident),* $(,)? })?) => {
        $crate::inventory::submit! {
            $crate::NativeTraitImpl {
                origin: $crate::pulsar_plugins::origin!(),
                class: stringify!($class),
                trait_id: $trait_id,
                methods: &[$($((stringify!($method), stringify!($bound))),*)?],
            }
        }
    };
}

/// The implementer id of a native component class.
pub fn component_implementer_id(class: &str) -> String {
    format!("component:{class}")
}

/// The component method a trait method is bound to for `class`.
pub fn bound_method<'a>(class: &str, trait_id: &str, method: &'a str) -> Option<&'a str> {
    let wanted = conformance::normalize_name(method);
    let mut implements = false;
    for registration in pulsar_plugins::registered::<NativeTraitImpl>() {
        if registration.class != class || registration.trait_id != trait_id {
            continue;
        }
        implements = true;
        if let Some((_, bound)) = registration
            .methods
            .iter()
            .find(|(name, _)| conformance::normalize_name(name) == wanted)
        {
            // `bound` is 'static; hand back the caller's lifetime.
            return Some(bound);
        }
    }
    implements.then_some(method)
}

fn list_native() -> Vec<Scanned> {
    let mut by_class: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    for registration in pulsar_plugins::registered::<NativeTraitImpl>() {
        by_class
            .entry(registration.class)
            .or_default()
            .push(registration.trait_id.to_owned());
    }
    by_class
        .into_iter()
        .map(|(class, mut ids)| {
            ids.sort();
            ids.dedup();
            Scanned {
                id: component_implementer_id(class),
                declared: Declared {
                    ids,
                    paths: Vec::new(),
                },
                binding: Some(Binding::Component {
                    class: class.to_owned(),
                }),
            }
        })
        .collect()
}

inventory::submit! {
    TraitImplementerSource {
        origin: pulsar_plugins::engine_origin!(),
        kind: NATIVE_KIND,
        label: "Native components",
        claims: None,
        scan: None,
        list: Some(list_native),
    }
}

/// The methods a component class provides, from its registered methods.
pub fn component_methods(class: &str) -> Vec<ProvidedMethod> {
    pulsar_reflection::inventory::iter::<pulsar_reflection::ComponentMethodRegistration>
        .into_iter()
        .filter(|registration| registration.class_name == class)
        .flat_map(|registration| (registration.methods)())
        .map(|method| ProvidedMethod {
            name: method.name.to_owned(),
            params: method
                .params
                .iter()
                .map(|param| ProvidedParam {
                    name: param.name.to_owned(),
                    type_name: Some(param.type_info.type_name.to_owned()),
                })
                .collect(),
            returns: match &method.return_type {
                None => ProvidedReturn::Unit,
                Some(ret) => ProvidedReturn::Value(Some(ret.type_info.type_name.to_owned())),
            },
        })
        .collect()
}

/// Does each native implementer provide what its traits require, through
/// its bindings? One issue per missing or mismatched method, as errors.
pub fn native_conformance(db: &TraitDb) -> Vec<(String, ConformanceIssue)> {
    let mut out = Vec::new();
    for registration in pulsar_plugins::registered::<NativeTraitImpl>() {
        let methods = component_methods(registration.class);
        // Present each component method under the trait method bound to it.
        let provided: Vec<ProvidedMethod> = db
            .required_methods(registration.trait_id)
            .into_iter()
            .filter_map(|(_, required)| {
                let bound = bound_method(registration.class, registration.trait_id, &required.name)?;
                let wanted = conformance::normalize_name(bound);
                methods
                    .iter()
                    .find(|method| conformance::normalize_name(&method.name) == wanted)
                    .map(|method| ProvidedMethod {
                        name: required.name.clone(),
                        ..method.clone()
                    })
            })
            .collect();
        for issue in conformance::check(
            db,
            &[registration.trait_id.to_owned()],
            &provided,
            Severity::Error,
        ) {
            out.push((component_implementer_id(registration.class), issue));
        }
    }
    out
}
