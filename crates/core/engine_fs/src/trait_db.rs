//! The trait database: every trait of a project and who implements it.
//!
//! [`TraitDb`] is the resolved, in-memory model behind
//! [`BlueprintTraitIndex`](crate::BlueprintTraitIndex). It knows nothing
//! about any one language: an *asset* is a project-relative path and a
//! *trait* is a [`TraitRecord`]. Any plugin that reads assets can add its
//! implementers to the index; Blueprints are the first. Plugins query it
//! through `pulsar_reflection::TRAIT_REGISTRY` (see [`TraitDb::publish`]),
//! which is the language-agnostic surface.
//!
//! All lookups are map-based. Supertraits are modelled: an asset that
//! implements a trait also implements everything that trait extends, and the
//! result says which trait it came through.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::trait_records::{is_trait_asset, normalize_relative_path, TraitMethodRecord, TraitRecord};

/// How an asset implements a trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Implementation<'a> {
    /// Project-relative path of the implementing asset.
    pub asset: &'a str,
    /// `None` when the asset declares the trait itself; otherwise the id of
    /// the declared trait that extends it.
    pub via: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct TraitDb {
    traits: BTreeMap<String, TraitRecord>,
    by_path: HashMap<String, String>,
    by_name: HashMap<String, Vec<String>>,
    /// Trait id -> ids of its resolved direct supertraits, sorted.
    supers: HashMap<String, Vec<String>>,
    /// Trait id -> supertrait references that name no known trait.
    unresolved_supers: BTreeMap<String, Vec<String>>,
    /// Traits that extend themselves, directly or not.
    cyclic: BTreeSet<String>,
    /// Asset -> declared trait ids.
    declared: BTreeMap<String, BTreeSet<String>>,
    /// Trait id -> asset -> via (`None` = declared).
    implementers: HashMap<String, BTreeMap<String, Option<String>>>,
    /// Asset -> effective trait ids.
    effective: BTreeMap<String, BTreeSet<String>>,
}

impl TraitDb {
    /// Build the database from trait records and each asset's declared
    /// trait ids. Ids that name no record are ignored here; the index keeps
    /// them as unresolved references. When two records share an id the one
    /// with the smaller path wins (the index never produces that).
    pub fn build(
        records: impl IntoIterator<Item = TraitRecord>,
        declarations: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Self {
        let mut db = Self::default();
        for record in records {
            if db.traits.contains_key(&record.id) {
                continue;
            }
            db.by_path.insert(record.path.clone(), record.id.clone());
            db.by_name
                .entry(record.name.clone())
                .or_default()
                .push(record.id.clone());
            db.traits.insert(record.id.clone(), record);
        }

        let mut linked = Vec::new();
        for record in db.traits.values() {
            let mut resolved = BTreeSet::new();
            let mut missing = Vec::new();
            let mut extends_itself = false;
            for reference in &record.supertraits {
                match db.lookup(reference) {
                    Some(found) if found.id != record.id => {
                        resolved.insert(found.id.clone());
                    }
                    Some(_) => extends_itself = true,
                    None => missing.push(reference.clone()),
                }
            }
            linked.push((record.id.clone(), resolved, missing, extends_itself));
        }
        for (id, resolved, missing, extends_itself) in linked {
            if extends_itself {
                db.cyclic.insert(id.clone());
            }
            if !missing.is_empty() {
                db.unresolved_supers.insert(id.clone(), missing);
            }
            db.supers.insert(id, resolved.into_iter().collect());
        }
        let ids: Vec<String> = db.traits.keys().cloned().collect();
        for id in ids {
            if db.reaches_itself(&id) {
                db.cyclic.insert(id);
            }
        }

        for (asset, trait_ids) in declarations {
            let ids: BTreeSet<String> = trait_ids
                .into_iter()
                .filter(|id| db.traits.contains_key(id))
                .collect();
            db.declared.insert(asset, ids);
        }
        let declared: Vec<(String, Vec<String>)> = db
            .declared
            .iter()
            .map(|(asset, ids)| (asset.clone(), ids.iter().cloned().collect()))
            .collect();
        for (asset, ids) in declared {
            for id in &ids {
                db.add_implementation(&asset, id, None);
                for ancestor in db.ancestors(id) {
                    db.add_implementation(&asset, &ancestor, Some(id.clone()));
                }
            }
        }
        db
    }

    /// A database of the engine's built-in traits alone: what a runtime has
    /// when its content ships no trait index.
    pub fn builtin() -> Self {
        Self::build(
            crate::builtin_traits::all()
                .iter()
                .map(|builtin| builtin.to_record()),
            Vec::new(),
        )
    }

    /// `ids` (those that exist) and every trait they extend: the traits an
    /// asset declaring `ids` effectively implements.
    pub fn with_ancestors<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a String>,
    ) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for id in ids {
            if self.traits.contains_key(id) {
                out.insert(id.clone());
                out.extend(self.ancestors(id));
            }
        }
        out
    }

    fn add_implementation(&mut self, asset: &str, trait_id: &str, via: Option<String>) {
        let slot = self
            .implementers
            .entry(trait_id.to_owned())
            .or_default()
            .entry(asset.to_owned())
            .or_insert(via.clone());
        // Declaring a trait outranks inheriting it.
        if via.is_none() {
            *slot = None;
        }
        self.effective
            .entry(asset.to_owned())
            .or_default()
            .insert(trait_id.to_owned());
    }

    fn reaches_itself(&self, start: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&str> = self.supers.get(start).into_iter().flatten().map(String::as_str).collect();
        while let Some(next) = stack.pop() {
            if next == start {
                return true;
            }
            if seen.insert(next) {
                stack.extend(self.supers.get(next).into_iter().flatten().map(String::as_str));
            }
        }
        false
    }

    fn lookup(&self, reference: &str) -> Option<&TraitRecord> {
        if let Some(found) = self.traits.get(reference) {
            return Some(found);
        }
        let normalized = normalize_relative_path(reference).filter(|p| is_trait_asset(p))?;
        self.by_path
            .get(&normalized)
            .and_then(|id| self.traits.get(id))
    }

    /// Every trait, in id order.
    pub fn traits(&self) -> impl Iterator<Item = &TraitRecord> {
        self.traits.values()
    }

    pub fn len(&self) -> usize {
        self.traits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.traits.is_empty()
    }

    /// The trait with this id.
    pub fn get(&self, id: &str) -> Option<&TraitRecord> {
        self.traits.get(id)
    }

    /// The trait a reference names: an id, else a project-relative path
    /// (either slash style), else a name that exactly one trait has.
    pub fn resolve(&self, reference: &str) -> Option<&TraitRecord> {
        if let Some(found) = self.lookup(reference) {
            return Some(found);
        }
        match self.by_name.get(reference).map(Vec::as_slice) {
            Some([only]) => self.traits.get(only),
            _ => None,
        }
    }

    /// The id of the trait now stored at `path`.
    pub fn id_for_path(&self, path: &str) -> Option<&str> {
        let normalized = normalize_relative_path(path)?;
        self.by_path.get(&normalized).map(String::as_str)
    }

    /// Direct supertraits of `id` that exist, by id.
    pub fn supertraits(&self, id: &str) -> &[String] {
        self.supers.get(id).map_or(&[], Vec::as_slice)
    }

    /// Supertrait references of `id` that name no trait.
    pub fn unresolved_supertraits(&self, id: &str) -> &[String] {
        self.unresolved_supers.get(id).map_or(&[], Vec::as_slice)
    }

    /// Whether `id` extends itself. Such a trait is still usable; its
    /// closure just stops at the repeat.
    pub fn is_cyclic(&self, id: &str) -> bool {
        self.cyclic.contains(id)
    }

    /// Every trait `id` extends, transitively, nearest first, without `id`.
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut order = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        seen.insert(id);
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        queue.push_back(id);
        while let Some(current) = queue.pop_front() {
            for parent in self.supertraits(current) {
                if seen.insert(parent.as_str()) {
                    order.push(parent.clone());
                    queue.push_back(parent.as_str());
                }
            }
        }
        order
    }

    /// The methods an implementer of `id` must provide: its own and every
    /// supertrait's. A method repeated up the chain appears once, nearest
    /// trait first.
    pub fn required_methods(&self, id: &str) -> Vec<(&TraitRecord, &TraitMethodRecord)> {
        let mut chain = vec![id.to_owned()];
        chain.extend(self.ancestors(id));
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for trait_id in chain {
            let Some(record) = self.traits.get(&trait_id) else {
                continue;
            };
            for method in &record.methods {
                if seen.insert(method.name.as_str()) {
                    out.push((record, method));
                }
            }
        }
        out
    }

    /// Assets implementing the trait `reference` names, declared or
    /// inherited, in path order.
    pub fn implementers(&self, reference: &str) -> Vec<Implementation<'_>> {
        let Some(record) = self.resolve(reference) else {
            return Vec::new();
        };
        self.implementers
            .get(&record.id)
            .into_iter()
            .flatten()
            .map(|(asset, via)| Implementation {
                asset: asset.as_str(),
                via: via.as_deref(),
            })
            .collect()
    }

    /// The traits an asset declares, by id.
    pub fn declared_traits(&self, asset: &str) -> Vec<&TraitRecord> {
        let Some(asset) = normalize_relative_path(asset) else {
            return Vec::new();
        };
        self.declared
            .get(&asset)
            .into_iter()
            .flatten()
            .filter_map(|id| self.traits.get(id))
            .collect()
    }

    /// The traits an asset implements, declared or inherited.
    pub fn traits_of(&self, asset: &str) -> Vec<&TraitRecord> {
        let Some(asset) = normalize_relative_path(asset) else {
            return Vec::new();
        };
        self.effective
            .get(&asset)
            .into_iter()
            .flatten()
            .filter_map(|id| self.traits.get(id))
            .collect()
    }

    /// Whether the asset implements the trait `reference` names.
    pub fn implements(&self, asset: &str, reference: &str) -> bool {
        let (Some(asset), Some(record)) = (normalize_relative_path(asset), self.resolve(reference))
        else {
            return false;
        };
        self.effective
            .get(&asset)
            .is_some_and(|ids| ids.contains(&record.id))
    }

    /// Publish this database to `pulsar_reflection::TRAIT_REGISTRY` under
    /// the project's scope, replacing the previous snapshot.
    #[cfg(feature = "editor")]
    pub fn publish(&self, project_root: &std::path::Path) {
        pulsar_reflection::TRAIT_REGISTRY.publish(&scope_key(project_root), self.snapshot());
    }

    /// This database as a reflection snapshot.
    #[cfg(feature = "editor")]
    pub fn snapshot(&self) -> pulsar_reflection::TraitSnapshot {
        use pulsar_reflection::{
            TraitDescriptor, TraitImplementer, TraitMethodDescriptor, TraitParamDescriptor,
        };
        let traits = self
            .traits
            .values()
            .map(|record| TraitDescriptor {
                id: record.id.clone(),
                name: record.name.clone(),
                display_name: record.display_name.clone(),
                description: record.description.clone(),
                path: is_trait_asset(&record.path).then(|| record.path.clone()),
                builtin: record.builtin,
                contract_version: record.contract_version,
                supertraits: self.supertraits(&record.id).to_vec(),
                methods: record
                    .methods
                    .iter()
                    .map(|method| TraitMethodDescriptor {
                        name: method.name.clone(),
                        params: method
                            .params
                            .iter()
                            .map(|param| TraitParamDescriptor {
                                name: param.name.clone(),
                                type_name: param.type_name.clone(),
                            })
                            .collect(),
                        return_type: method.return_type.clone(),
                        doc: method.doc.clone(),
                        has_default: method.has_default,
                    })
                    .collect(),
            })
            .collect();
        let implementations = self
            .implementers
            .iter()
            .flat_map(|(trait_id, assets)| {
                assets.iter().map(move |(asset, via)| {
                    (
                        trait_id.clone(),
                        TraitImplementer {
                            asset: asset.clone(),
                            kind: "blueprint".to_owned(),
                            via: via.clone(),
                        },
                    )
                })
            })
            .collect();
        pulsar_reflection::TraitSnapshot::new(traits, implementations)
    }
}

/// The key a project's snapshot is published under (shared with plugins
/// through `pulsar_reflection`).
#[cfg(feature = "editor")]
pub fn scope_key(project_root: &std::path::Path) -> String {
    pulsar_reflection::trait_scope_key(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trait_records::{TraitMethodRecord, TraitParamRecord};

    fn record(id: &str, name: &str, supers: &[&str], methods: &[&str]) -> TraitRecord {
        TraitRecord {
            id: id.into(),
            path: format!("types/traits/{}.trait.json", name.to_lowercase()),
            name: name.into(),
            display_name: name.into(),
            description: None,
            builtin: false,
            contract_version: 0,
            supertraits: supers.iter().map(|s| s.to_string()).collect(),
            methods: methods
                .iter()
                .map(|m| TraitMethodRecord {
                    name: (*m).into(),
                    params: vec![TraitParamRecord {
                        name: "x".into(),
                        type_name: "f32".into(),
                    }],
                    return_type: "()".into(),
                    doc: None,
                    has_default: false,
                })
                .collect(),
            type_index_id: None,
        }
    }

    fn db() -> TraitDb {
        TraitDb::build(
            vec![
                record("a", "Base", &[], &["tick", "shared"]),
                record("b", "Mid", &["a"], &["shared", "mid"]),
                record("c", "Leaf", &["types/traits/mid.trait.json"], &["leaf"]),
            ],
            vec![
                ("bp/one/graph_save.json".into(), vec!["c".into()]),
                ("bp/two/graph_save.json".into(), vec!["a".into(), "missing".into()]),
            ],
        )
    }

    #[test]
    fn supertraits_resolve_by_id_and_by_path() {
        let db = db();
        assert_eq!(db.supertraits("c"), ["b"]);
        assert_eq!(db.ancestors("c"), ["b", "a"]);
        assert!(db.unresolved_supertraits("c").is_empty());
    }

    #[test]
    fn implementers_include_inherited_ones_with_their_route() {
        let db = db();
        let base = db.implementers("a");
        assert_eq!(
            base,
            vec![
                Implementation { asset: "bp/one/graph_save.json", via: Some("c") },
                Implementation { asset: "bp/two/graph_save.json", via: None },
            ]
        );
        assert_eq!(db.implementers("Leaf").len(), 1);
        assert!(db.implements("bp\\one\\graph_save.json", "Base"));
        assert!(!db.implements("bp/two/graph_save.json", "Leaf"));
        let names: Vec<_> = db
            .traits_of("bp/one/graph_save.json")
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["Base", "Mid", "Leaf"]);
        assert_eq!(db.declared_traits("bp/one/graph_save.json").len(), 1);
    }

    #[test]
    fn required_methods_merge_the_chain_nearest_first() {
        let db = db();
        let methods: Vec<_> = db
            .required_methods("c")
            .into_iter()
            .map(|(owner, m)| format!("{}.{}", owner.name, m.name))
            .collect();
        assert_eq!(
            methods,
            ["Leaf.leaf", "Mid.shared", "Mid.mid", "Base.tick"]
        );
    }

    #[test]
    fn cycles_are_flagged_and_do_not_hang() {
        let db = TraitDb::build(
            vec![
                record("a", "A", &["b"], &[]),
                record("b", "B", &["a"], &[]),
                record("s", "Self", &["s"], &[]),
            ],
            vec![("x".to_string(), vec!["a".to_string()])],
        );
        assert!(db.is_cyclic("a") && db.is_cyclic("b") && db.is_cyclic("s"));
        assert!(db.implements("x", "b"));
        assert_eq!(db.ancestors("a"), ["b"]);
    }

    #[test]
    fn missing_supertraits_are_reported_not_fatal() {
        let db = TraitDb::build(vec![record("a", "A", &["gone"], &[])], Vec::new());
        assert_eq!(db.unresolved_supertraits("a"), ["gone"]);
        assert!(db.supertraits("a").is_empty());
    }

    #[test]
    fn resolve_prefers_id_then_path_then_unique_name() {
        let db = TraitDb::build(
            vec![
                record("a", "Same", &[], &[]),
                {
                    let mut other = record("b", "Same", &[], &[]);
                    other.path = "elsewhere/same.trait.json".into();
                    other
                },
                record("c", "Unique", &[], &[]),
            ],
            Vec::new(),
        );
        assert_eq!(db.resolve("b").unwrap().id, "b");
        assert_eq!(db.resolve("elsewhere\\same.trait.json").unwrap().id, "b");
        assert!(db.resolve("Same").is_none());
        assert_eq!(db.resolve("Unique").unwrap().id, "c");
        assert_eq!(db.id_for_path("types/traits/unique.trait.json"), Some("c"));
    }

    #[cfg(feature = "editor")]
    #[test]
    fn publishes_to_the_reflection_registry() {
        let db = db();
        let dir = std::path::Path::new("D:/published/project");
        db.publish(dir);
        let snapshot = pulsar_reflection::TRAIT_REGISTRY
            .snapshot(&scope_key(dir))
            .expect("published");
        assert_eq!(snapshot.traits().count(), 3);
        assert_eq!(snapshot.implementers("Base").len(), 2);
        assert!(snapshot.implements("bp/one/graph_save.json", "Base"));
        assert_eq!(snapshot.get("Leaf").unwrap().supertraits, ["b"]);
        pulsar_reflection::TRAIT_REGISTRY.remove(&scope_key(dir));
    }
}
