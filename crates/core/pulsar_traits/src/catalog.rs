//! Traits in the plugin catalog: the trait definitions each plugin's folder
//! serves (`plugins/<id>/traits/*.trait.json`), as entries of category
//! [`DEFINITION`] under the plugin that provides them, read from the files
//! themselves (`engine_fs::provided_files`), never from a second list.
//!
//! Roles and native implementers list the trait they use as a child
//! ([`crate::sources::TRAIT_USE`]); the catalog checks a use of another
//! plugin's trait is a requirement of the user's plugin.

use crate::records::{embedded_id, is_trait_asset};

/// The category of a trait definition a plugin's folder serves.
pub const DEFINITION: &str = "traits.definition";

fn definitions(
    providers: impl Iterator<Item = &'static engine_fs::FileProvider>,
) -> Vec<pulsar_plugins::Summary> {
    let mut out = Vec::new();
    for provider in providers {
        for (path, bytes) in engine_fs::virtual_fs::provided_files(provider, is_trait_asset) {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            let Some(id) = embedded_id(&value) else {
                continue;
            };
            let label = value
                .get("displayName")
                .and_then(|name| name.as_str())
                .map_or_else(
                    || format!("{id} ({path})"),
                    |name| format!("{name} ({path})"),
                );
            out.push(pulsar_plugins::Summary::new(
                DEFINITION,
                id,
                label,
                provider.origin,
            ));
        }
    }
    out
}

fn all_definitions() -> Vec<pulsar_plugins::Summary> {
    definitions(pulsar_plugins::all_registered::<engine_fs::FileProvider>())
}

fn active_definitions() -> Vec<pulsar_plugins::Summary> {
    definitions(pulsar_plugins::registered::<engine_fs::FileProvider>())
}

// Definitions are files of a folder a provider registered: their source
// reads the providers (the engine's own folder included, linked).
inventory::submit! {
    pulsar_plugins::ContributionSource {
        category: DEFINITION,
        all: all_definitions,
        active: active_definitions,
        linked: Vec::new,
    }
}
