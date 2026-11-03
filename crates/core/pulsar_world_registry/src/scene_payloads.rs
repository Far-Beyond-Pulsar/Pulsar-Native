//! CPU payloads scene content publishes for the renderer's features.
//!
//! A plugin whose Helio feature reads CPU data (a voxel world, its pick
//! requests) registers how its components publish it: under the key of the
//! feature's payload contract (`helio_core::PayloadContract`), as the
//! contract's type. A host gathers every registered payload each frame
//! ([`scene_payloads`]) and hands them to its renderer by key; it knows no
//! payload, and the features know no component.

use std::any::Any;
use std::sync::Arc;

use pulsar_scenedb::World;

/// A payload every scene may publish.
pub struct ScenePayloadRegistration {
    /// The payload contract's key.
    pub key: &'static str,
    /// The scene's payload, or `None` when it has nothing to publish under
    /// the key. Problems a user should see (a world that does not build, a
    /// bad setting) go into the list.
    pub publish: fn(&World, &mut Vec<String>) -> Option<Arc<dyn Any + Send + Sync>>,
    /// Whether the content is still settling (a world building, requests
    /// waiting on the renderer): a host rendering on demand keeps drawing
    /// frames, and stepping the scene, meanwhile. Cheap: asked every frame,
    /// idle ones too.
    pub settling: fn(&World) -> bool,
    /// The feature reading it samples stochastically and needs temporal
    /// resolve while it has content: hosts prefer TSR then.
    pub temporal: bool,
}

inventory::collect!(ScenePayloadRegistration);

/// What the scene publishes this frame.
#[derive(Default)]
pub struct ScenePayloads {
    /// Payloads by key.
    pub payloads: Vec<(&'static str, Arc<dyn Any + Send + Sync>)>,
    /// Problems the publishers reported.
    pub problems: Vec<String>,
    /// Whether published content wants temporal resolve.
    pub temporal: bool,
}

/// Every registered payload `world` publishes.
pub fn scene_payloads(world: &World) -> ScenePayloads {
    let mut out = ScenePayloads::default();
    for registration in inventory::iter::<ScenePayloadRegistration> {
        if let Some(payload) = (registration.publish)(world, &mut out.problems) {
            out.temporal |= registration.temporal;
            out.payloads.push((registration.key, payload));
        }
    }
    out
}

/// Whether any registered content of `world` is still settling.
pub fn scene_settling(world: &World) -> bool {
    inventory::iter::<ScenePayloadRegistration>
        .into_iter()
        .any(|registration| (registration.settling)(world))
}
