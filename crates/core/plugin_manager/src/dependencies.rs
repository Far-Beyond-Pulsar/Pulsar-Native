//! Dependency resolution between plugins.
//!
//! [`plan_load_order`] takes the manifests of the plugins found and decides
//! which load, in what order, and why the others do not. It runs on manifests
//! alone, before any plugin constructor.
//!
//! Rules:
//! - a plugin loads after every plugin in `requires`, and after every plugin
//!   in `optional` that is present and in range;
//! - a plugin whose `requires` is missing or out of range is rejected, and so,
//!   transitively, is everything that requires it;
//! - plugins on a cycle are rejected, and so is everything that requires them;
//! - of two plugins with the same id the first one found wins;
//! - among plugins that are ready, input order is kept, so a directory with
//!   no dependencies loads exactly as before.

use std::collections::HashMap;
use std::fmt;

use plugin_editor_api::manifest::satisfies;
use plugin_editor_api::{PluginDependency, PluginId, PluginManifest};

/// Why a plugin is not loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    /// A required plugin was not found.
    Missing { dependency: PluginDependency },
    /// A required plugin was found, at a version outside the requirement.
    VersionMismatch {
        dependency: PluginDependency,
        found: String,
    },
    /// A required plugin is itself rejected.
    DependencyRejected { dependency: PluginId },
    /// The plugin is on a dependency cycle; `path` runs from it back to it.
    Cycle { path: Vec<PluginId> },
    /// Another plugin with the same id was found first.
    DuplicateId,
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { dependency } => write!(
                f,
                "requires plugin {} ({}), which is not installed",
                dependency.id, dependency.requirement
            ),
            Self::VersionMismatch { dependency, found } => write!(
                f,
                "requires plugin {} {}, found version {}",
                dependency.id, dependency.requirement, found
            ),
            Self::DependencyRejected { dependency } => {
                write!(f, "requires plugin {dependency}, which did not load")
            }
            Self::Cycle { path } => {
                let path: Vec<&str> = path.iter().map(PluginId::as_str).collect();
                write!(f, "is on a dependency cycle: {}", path.join(" -> "))
            }
            Self::DuplicateId => write!(f, "has the same id as a plugin found earlier"),
        }
    }
}

/// The outcome of [`plan_load_order`]. Indices refer to the input slice.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoadPlan {
    /// Plugins to load, dependencies first.
    pub order: Vec<usize>,
    /// Plugins not to load, with the reason.
    pub rejected: Vec<(usize, DependencyError)>,
}

pub fn plan_load_order(manifests: &[PluginManifest]) -> LoadPlan {
    let mut plan = LoadPlan::default();
    let mut rejected: HashMap<usize, DependencyError> = HashMap::new();

    // The first plugin with an id owns it.
    let mut owner: HashMap<&PluginId, usize> = HashMap::new();
    for (index, manifest) in manifests.iter().enumerate() {
        if owner.contains_key(&manifest.id) {
            rejected.insert(index, DependencyError::DuplicateId);
        } else {
            owner.insert(&manifest.id, index);
        }
    }

    // Reject plugins whose requirements are not met, until nothing changes
    // (rejecting one can reject its dependents).
    loop {
        let mut changed = false;
        for (index, manifest) in manifests.iter().enumerate() {
            if rejected.contains_key(&index) {
                continue;
            }
            for dependency in &manifest.requires {
                let error = match owner.get(&dependency.id) {
                    None => Some(DependencyError::Missing {
                        dependency: dependency.clone(),
                    }),
                    Some(&target) if rejected.contains_key(&target) => {
                        Some(DependencyError::DependencyRejected {
                            dependency: dependency.id.clone(),
                        })
                    }
                    Some(&target) if !satisfies(&manifests[target], dependency) => {
                        Some(DependencyError::VersionMismatch {
                            dependency: dependency.clone(),
                            found: manifests[target].version.clone(),
                        })
                    }
                    Some(_) => None,
                };
                if let Some(error) = error {
                    rejected.insert(index, error);
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Edges to the plugins each survivor must follow. An optional dependency
    // counts only if it is present, in range and not rejected.
    let edges: Vec<Vec<usize>> = manifests
        .iter()
        .enumerate()
        .map(|(index, manifest)| {
            if rejected.contains_key(&index) {
                return Vec::new();
            }
            let required = manifest
                .requires
                .iter()
                .filter_map(|d| owner.get(&d.id).copied());
            let optional = manifest.optional.iter().filter_map(|d| {
                let target = *owner.get(&d.id)?;
                (!rejected.contains_key(&target) && satisfies(&manifests[target], d))
                    .then_some(target)
            });
            required.chain(optional).collect()
        })
        .collect();

    // Order the survivors, ready plugins in input order.
    let mut placed = vec![false; manifests.len()];
    loop {
        let next = (0..manifests.len()).find(|&i| {
            !placed[i] && !rejected.contains_key(&i) && edges[i].iter().all(|&d| placed[d])
        });
        match next {
            Some(i) => {
                placed[i] = true;
                plan.order.push(i);
            }
            None => break,
        }
    }

    // Whatever is left is on a cycle or requires something that is.
    let stuck: Vec<usize> = (0..manifests.len())
        .filter(|&i| !placed[i] && !rejected.contains_key(&i))
        .collect();
    for &index in &stuck {
        let error = match find_cycle(index, &edges, &stuck) {
            Some(cycle) => DependencyError::Cycle {
                path: cycle.iter().map(|&i| manifests[i].id.clone()).collect(),
            },
            None => {
                let blocker = edges[index]
                    .iter()
                    .find(|&&d| stuck.contains(&d))
                    .map(|&d| manifests[d].id.clone())
                    .unwrap_or_else(|| manifests[index].id.clone());
                DependencyError::DependencyRejected {
                    dependency: blocker,
                }
            }
        };
        rejected.insert(index, error);
    }

    let mut rejected: Vec<_> = rejected.into_iter().collect();
    rejected.sort_by_key(|(index, _)| *index);
    plan.rejected = rejected;
    plan
}

/// A path from `start` back to `start` through `within`, if there is one.
fn find_cycle(start: usize, edges: &[Vec<usize>], within: &[usize]) -> Option<Vec<usize>> {
    fn walk(
        node: usize,
        start: usize,
        edges: &[Vec<usize>],
        within: &[usize],
        path: &mut Vec<usize>,
        seen: &mut Vec<usize>,
    ) -> bool {
        for &next in &edges[node] {
            if !within.contains(&next) {
                continue;
            }
            if next == start {
                path.push(start);
                return true;
            }
            if seen.contains(&next) {
                continue;
            }
            seen.push(next);
            path.push(next);
            if walk(next, start, edges, within, path, seen) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = vec![start];
    let mut seen = vec![start];
    walk(start, start, edges, within, &mut path, &mut seen).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, version: &str) -> PluginManifest {
        PluginManifest::new(id, id, version, "test")
    }

    fn ids(manifests: &[PluginManifest], plan: &LoadPlan) -> Vec<String> {
        plan.order
            .iter()
            .map(|&i| manifests[i].id.to_string())
            .collect()
    }

    #[test]
    fn independent_plugins_keep_input_order() {
        let ms = [m("c", "1.0.0"), m("a", "1.0.0"), m("b", "1.0.0")];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["c", "a", "b"]);
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn dependency_loads_before_dependent() {
        let ms = [
            m("character", "0.1.0")
                .requires(PluginDependency::new("skeletal", "^0.1"))
                .requires(PluginDependency::new("physics", "^0.1")),
            m("physics", "0.1.2"),
            m("skeletal", "0.1.0"),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["physics", "skeletal", "character"]);
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn missing_dependency_rejects_plugin_and_its_dependents() {
        let ms = [
            m("top", "1.0.0").requires(PluginDependency::any("middle")),
            m("middle", "1.0.0").requires(PluginDependency::any("absent")),
            m("free", "1.0.0"),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["free"]);
        assert!(matches!(
            plan.rejected[0],
            (0, DependencyError::DependencyRejected { .. })
        ));
        assert!(matches!(
            plan.rejected[1],
            (1, DependencyError::Missing { .. })
        ));
    }

    #[test]
    fn out_of_range_version_is_rejected_with_the_found_version() {
        let ms = [
            m("a", "1.0.0").requires(PluginDependency::new("b", "^2")),
            m("b", "1.5.0"),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["b"]);
        match &plan.rejected[0].1 {
            DependencyError::VersionMismatch { found, .. } => assert_eq!(found, "1.5.0"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cycle_is_rejected_with_its_path_and_does_not_hide_other_plugins() {
        let ms = [
            m("a", "1.0.0").requires(PluginDependency::any("b")),
            m("b", "1.0.0").requires(PluginDependency::any("c")),
            m("c", "1.0.0").requires(PluginDependency::any("a")),
            m("d", "1.0.0").requires(PluginDependency::any("a")),
            m("ok", "1.0.0"),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["ok"]);
        assert_eq!(plan.rejected.len(), 4);
        match &plan.rejected[0].1 {
            DependencyError::Cycle { path } => {
                let path: Vec<&str> = path.iter().map(PluginId::as_str).collect();
                assert_eq!(path, ["a", "b", "c", "a"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            plan.rejected[3].1,
            DependencyError::DependencyRejected { .. }
        ));
    }

    #[test]
    fn self_dependency_is_a_cycle() {
        let ms = [m("a", "1.0.0").requires(PluginDependency::any("a"))];
        let plan = plan_load_order(&ms);
        assert!(plan.order.is_empty());
        assert!(matches!(plan.rejected[0].1, DependencyError::Cycle { .. }));
    }

    #[test]
    fn optional_dependency_orders_when_present_and_is_ignored_when_absent() {
        let ms = [
            m("user", "1.0.0").optional(PluginDependency::any("extra")),
            m("extra", "1.0.0"),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["extra", "user"]);

        let ms = [m("user", "1.0.0").optional(PluginDependency::any("extra"))];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["user"]);
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn optional_cycle_with_out_of_range_version_does_not_block() {
        let ms = [
            m("a", "1.0.0").optional(PluginDependency::new("b", "^9")),
            m("b", "1.0.0").requires(PluginDependency::any("a")),
        ];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["a", "b"]);
    }

    #[test]
    fn duplicate_id_keeps_the_first() {
        let ms = [m("a", "1.0.0"), m("a", "2.0.0")];
        let plan = plan_load_order(&ms);
        assert_eq!(ids(&ms, &plan), ["a"]);
        assert_eq!(plan.rejected, vec![(1, DependencyError::DuplicateId)]);
    }
}
