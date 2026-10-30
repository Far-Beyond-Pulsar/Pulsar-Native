//! Trait queries for plugins.
//!
//! A *trait* is an engine- or project-authored contract (`*.trait.json`)
//! that assets of any language can implement: the role traits `GameMode`,
//! `PlayerController`, `PlayerCharacter`, `GameState`, or a project's own. The
//! host keeps one database of them (`engine_fs::TraitDb`) and publishes it
//! to the reflection registry; this module is the plugin-side view.
//!
//! There is no new vtable and no host callback to link against: a plugin's
//! copy of `pulsar_reflection` shares the editor's registries (see
//! `.agents/PLUGIN_SYSTEM.md`), so these functions read what the host
//! published. A plugin that is not Rust reads the same data as JSON
//! ([`ProjectTraits::to_json`]).
//!
//! ```rust,ignore
//! let traits = plugin_editor_api::ProjectTraits::for_project(&project_root);
//! for implementer in traits.implementers("GameMode") {
//!     println!("{} implements GameMode", implementer.asset);
//! }
//! ```
//!
//! A trait reference is an id (stable), a project-relative path, or a name
//! that is unique in the project. Prefer ids; they survive renames.

use std::path::Path;
use std::sync::Arc;

pub use pulsar_reflection::{
    TraitDescriptor, TraitImplementer, TraitMethodDescriptor, TraitParamDescriptor, TraitSnapshot,
};

/// One project's traits, as the host last published them.
#[derive(Clone, Debug)]
pub struct ProjectTraits {
    scope: String,
}

impl ProjectTraits {
    /// The traits of the project rooted at `project_root`.
    pub fn for_project(project_root: &Path) -> Self {
        Self {
            scope: pulsar_reflection::trait_scope_key(project_root),
        }
    }

    /// The snapshot itself, for several queries against one consistent
    /// state. `None` until the host has indexed the project.
    pub fn snapshot(&self) -> Option<Arc<TraitSnapshot>> {
        pulsar_reflection::TRAIT_REGISTRY.snapshot(&self.scope)
    }

    /// Every trait of the project.
    pub fn traits(&self) -> Vec<TraitDescriptor> {
        self.snapshot()
            .map(|snapshot| snapshot.traits().cloned().collect())
            .unwrap_or_default()
    }

    /// The trait a reference names.
    pub fn get(&self, trait_ref: &str) -> Option<TraitDescriptor> {
        self.snapshot()?.get(trait_ref).cloned()
    }

    /// Assets that implement the trait, declared or inherited through a
    /// supertrait.
    pub fn implementers(&self, trait_ref: &str) -> Vec<TraitImplementer> {
        self.snapshot()
            .map(|snapshot| snapshot.implementers(trait_ref).to_vec())
            .unwrap_or_default()
    }

    /// Traits an asset (a project-relative path) implements.
    pub fn traits_of(&self, asset: &str) -> Vec<TraitDescriptor> {
        self.snapshot()
            .map(|snapshot| snapshot.traits_of(asset).into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether the asset implements the trait.
    pub fn implements(&self, asset: &str, trait_ref: &str) -> bool {
        self.snapshot()
            .is_some_and(|snapshot| snapshot.implements(asset, trait_ref))
    }

    /// The whole snapshot as JSON; `null` before the host has indexed the
    /// project.
    pub fn to_json(&self) -> serde_json::Value {
        self.snapshot()
            .map_or(serde_json::Value::Null, |snapshot| snapshot.to_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_read_what_the_host_published() {
        let root = Path::new("D:/plugin-api-test/project");
        let traits = ProjectTraits::for_project(root);
        assert!(traits.traits().is_empty());
        assert!(traits.to_json().is_null());

        let descriptor = TraitDescriptor {
            id: "pulsar.GameMode".into(),
            name: "GameMode".into(),
            display_name: "Game Mode".into(),
            description: None,
            path: Some("types/traits/game_mode.trait.json".into()),
            builtin: true,
            contract_version: 1,
            supertraits: Vec::new(),
            methods: Vec::new(),
        };
        let implementer = TraitImplementer {
            asset: "classes/Mode/graph_save.json".into(),
            kind: "blueprint".into(),
            via: None,
        };
        // What the host does after indexing the project.
        pulsar_reflection::TRAIT_REGISTRY.publish(
            &pulsar_reflection::trait_scope_key(root),
            TraitSnapshot::new(
                vec![descriptor],
                vec![("pulsar.GameMode".into(), implementer.clone())],
            ),
        );

        assert_eq!(traits.traits().len(), 1);
        assert_eq!(traits.implementers("GameMode"), vec![implementer]);
        assert_eq!(traits.traits_of("classes/Mode/graph_save.json")[0].name, "GameMode");
        assert!(traits.implements("classes\\Mode\\graph_save.json", "pulsar.GameMode"));
        assert_eq!(traits.to_json()["traits"]["pulsar.GameMode"]["name"], "GameMode");

        pulsar_reflection::TRAIT_REGISTRY.remove(&pulsar_reflection::trait_scope_key(root));
    }
}
