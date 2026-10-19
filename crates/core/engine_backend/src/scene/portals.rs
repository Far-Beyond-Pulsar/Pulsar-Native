//! Portals: pairing authored portals and feeding Helio's portal passes.
//!
//! The only authored thing is `PortalComponent` (`helio_component`): an opening,
//! its size, and a **Link ID**. This module turns the authored portals of a
//! world into what the portal passes draw, each frame:
//!
//! 1. [`pair_portals`] groups enabled, visible portals by Link ID. Exactly two
//!    with the same non-zero ID are partners; one alone or three or more are
//!    reported as problems and drawn as nothing.
//! 2. Each portal's pose is its object's world pose (rotation and translation;
//!    scale sizes the opening), relative to the frame's world origin like every
//!    other row the renderer reads.
//! 3. Helio's `SubLevelResolver` + `PortalProjectionBridge` expand the pairs into
//!    views and recursion chains, taking their coordinate-space slots from the
//!    host's one table ([`SpaceTable`](crate::scene::SpaceTable)), so they cannot
//!    collide with rigid frames.
//! 4. The rows go to the [`PortalProjectionInput`] a scene derivation uploads, and
//!    the live counts go to the renderer.
//!
//! Portals inside a level instance need nothing extra: placed content is real
//! objects in the world, so a portal in a displayed level pairs with a portal
//! outside it exactly like any other.

use std::collections::{BTreeMap, HashSet};

use glam::{DMat4, DQuat, DVec3, Mat4};
use helio_component::components::PortalComponent as AuthoredPortal;
use helio_default_graphs::portal_projection::PortalProjectionInput;
use helio_pass_portal_cull::components::PortalComponent as PortalRow;
use helio_pass_portal_cull::{
    PortalProjectionBridge, PortalProjectionConfig, SubLevelContents, SubLevelResolver,
};
use pulsar_scene_model::attachments::enabled_components;
use pulsar_scene_model::frames::world_dmat4_of;
use pulsar_scene_model::Visibility;
use pulsar_scenedb::{Entity, World};

/// Chains the projection may produce (a pair of facing portals recurses; this
/// bounds a scene with many).
const MAX_CHAINS: usize = 256;

/// One enabled, visible authored portal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portal {
    /// The component instance (unique even if an object holds several).
    pub instance: Entity,
    pub owner: Entity,
    pub link_id: i32,
    pub width: f32,
    pub height: f32,
}

/// Why a Link ID shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortalProblem {
    /// One enabled portal has this ID and no partner.
    Alone { link_id: i32 },
    /// More than two portals share this ID.
    Crowded { link_id: i32, count: usize },
}

impl std::fmt::Display for PortalProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alone { link_id } => {
                write!(f, "portal Link ID {link_id} has no partner (needs exactly two)")
            }
            Self::Crowded { link_id, count } => write!(
                f,
                "portal Link ID {link_id} is shared by {count} portals (needs exactly two)"
            ),
        }
    }
}

/// The partners found in a world, and the Link IDs that did not pair.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pairing {
    pub pairs: Vec<(Portal, Portal)>,
    pub problems: Vec<PortalProblem>,
}

/// Every enabled, visible authored portal.
pub fn authored_portals(world: &World) -> Vec<Portal> {
    let mut portals: Vec<Portal> = enabled_components::<AuthoredPortal>(world)
        .filter(|(_, owner, portal)| {
            portal.enabled
                && portal.link_id != 0
                && world
                    .get::<Visibility>(*owner)
                    .is_none_or(|visibility| visibility.visible)
        })
        .map(|(instance, owner, portal)| Portal {
            instance,
            owner,
            link_id: portal.link_id,
            width: portal.width,
            height: portal.height,
        })
        .collect();
    portals.sort_by_key(|portal| portal.instance.bits());
    portals
}

/// Group portals by Link ID: exactly two make a pair.
pub fn pair_portals(world: &World) -> Pairing {
    let mut by_id: BTreeMap<i32, Vec<Portal>> = BTreeMap::new();
    for portal in authored_portals(world) {
        by_id.entry(portal.link_id).or_default().push(portal);
    }
    let mut pairing = Pairing::default();
    for (link_id, group) in by_id {
        match group.as_slice() {
            [a, b] => pairing.pairs.push((*a, *b)),
            [_] => pairing.problems.push(PortalProblem::Alone { link_id }),
            many => pairing.problems.push(PortalProblem::Crowded {
                link_id,
                count: many.len(),
            }),
        }
    }
    pairing
}

/// A portal's pose and opening: the object's world rotation and translation
/// (relative to `origin`), and half the opening's extent scaled by the object's
/// world scale along its own X and Y.
fn pose_of(world: &World, portal: &Portal, origin: DVec3) -> Option<(Mat4, [f32; 2])> {
    let matrix = world_dmat4_of(world, portal.owner)?;
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    if !(scale.x.is_finite() && scale.y.is_finite()) {
        return None;
    }
    let pose = DMat4::from_rotation_translation(
        if rotation.is_finite() { rotation } else { DQuat::IDENTITY },
        translation - origin,
    );
    let half = [
        (f64::from(portal.width) * scale.x.abs() * 0.5) as f32,
        (f64::from(portal.height) * scale.y.abs() * 0.5) as f32,
    ];
    Some((pose.as_mat4(), half))
}

/// The resolver for `pairing`: one sublevel (the default level, in which all
/// portals of the world live), each portal naming its partner.
pub fn build_resolver(world: &World, pairing: &Pairing, origin: DVec3) -> SubLevelResolver {
    let mut rows = Vec::with_capacity(pairing.pairs.len() * 2);
    for (a, b) in &pairing.pairs {
        for (portal, partner) in [(a, b), (b, a)] {
            let Some((pose, half)) = pose_of(world, portal, origin) else {
                continue;
            };
            rows.push((
                portal.instance,
                PortalRow::new(Some(partner.instance), pose, half),
            ));
        }
    }
    let mut resolver = SubLevelResolver::new();
    resolver.insert_sublevel(0, SubLevelContents::new([], rows));
    resolver
}

/// Builds the portal rows each frame and feeds them to the portal passes.
pub struct PortalSync {
    input: PortalProjectionInput,
    bridge: PortalProjectionBridge,
    /// Problems already reported, so a standing problem is logged once.
    reported: HashSet<PortalProblem>,
    counts: Option<(u32, u32)>,
}

impl Default for PortalSync {
    fn default() -> Self {
        Self {
            input: PortalProjectionInput::default(),
            bridge: PortalProjectionBridge::with_config(PortalProjectionConfig {
                max_chains: Some(MAX_CHAINS),
                ..PortalProjectionConfig::default()
            })
            .expect("a nonzero chain depth"),
            reported: HashSet::new(),
            counts: None,
        }
    }
}

impl PortalSync {
    /// The rows the portal derivation uploads (clone it into the derivation).
    pub fn input(&self) -> PortalProjectionInput {
        self.input.clone()
    }

    /// Rebuild the portals of `world` (relative to `origin`), taking the spaces
    /// they need from `table`. Returns the live `(views, chains)` when they
    /// changed since the last call.
    pub fn apply(
        &mut self,
        world: &World,
        origin: DVec3,
        table: &mut crate::scene::SpaceTable,
    ) -> Option<(u32, u32)> {
        let pairing = pair_portals(world);
        let current: HashSet<PortalProblem> = pairing.problems.iter().copied().collect();
        for problem in &current {
            if self.reported.insert(*problem) {
                tracing::warn!("{problem}");
            }
        }
        self.reported.retain(|problem| current.contains(problem));

        let counts = if pairing.pairs.is_empty() {
            self.input.clear();
            (0, 0)
        } else {
            let resolver = build_resolver(world, &pairing, origin);
            match self.bridge.build_into(&resolver, table) {
                Ok(frame) => {
                    self.input.set(&frame, table.matrices());
                    (frame.counts.portal_view_count, frame.counts.portal_chain_count)
                }
                Err(error) => {
                    tracing::warn!("portals not drawn: {error}");
                    self.input.clear();
                    (0, 0)
                }
            }
        };
        (self.counts != Some(counts)).then(|| {
            self.counts = Some(counts);
            counts
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::world_ext::{SceneWorldExt, SpawnObject};
    use pulsar_scene_model::Transform;

    fn portal_at(world: &mut World, link_id: i32, position: [f32; 3]) -> Entity {
        let object = world
            .spawn_object(SpawnObject::new("portal").with_transform(Transform {
                position,
                ..Transform::default()
            }))
            .unwrap();
        pulsar_world_registry::attach_value(
            world,
            object,
            AuthoredPortal {
                link_id,
                ..AuthoredPortal::default()
            },
        )
        .unwrap();
        object
    }

    #[test]
    fn two_portals_with_one_link_id_are_partners() {
        let mut world = World::new();
        portal_at(&mut world, 5, [0.0; 3]);
        portal_at(&mut world, 5, [20.0, 0.0, 0.0]);
        portal_at(&mut world, 0, [1.0, 0.0, 0.0]);
        let pairing = pair_portals(&world);
        assert_eq!(pairing.pairs.len(), 1, "an unlinked portal pairs with nothing");
        assert!(pairing.problems.is_empty());
    }

    #[test]
    fn one_or_three_portals_with_an_id_are_reported() {
        let mut world = World::new();
        portal_at(&mut world, 1, [0.0; 3]);
        for _ in 0..3 {
            portal_at(&mut world, 2, [0.0; 3]);
        }
        let pairing = pair_portals(&world);
        assert!(pairing.pairs.is_empty());
        assert_eq!(
            pairing.problems,
            [
                PortalProblem::Alone { link_id: 1 },
                PortalProblem::Crowded { link_id: 2, count: 3 }
            ]
        );
    }

    #[test]
    fn a_hidden_or_disabled_portal_leaves_its_partner_alone() {
        let mut world = World::new();
        let a = portal_at(&mut world, 7, [0.0; 3]);
        portal_at(&mut world, 7, [5.0, 0.0, 0.0]);
        assert_eq!(pair_portals(&world).pairs.len(), 1);
        world
            .get_mut::<Visibility>(a)
            .unwrap()
            .visible = false;
        let pairing = pair_portals(&world);
        assert!(pairing.pairs.is_empty());
        assert_eq!(pairing.problems, [PortalProblem::Alone { link_id: 7 }]);
    }

    #[test]
    fn the_projection_gets_views_and_chains_through_the_shared_table() {
        let mut world = World::new();
        portal_at(&mut world, 3, [0.0, 0.0, 0.0]);
        portal_at(&mut world, 3, [50.0, 0.0, 0.0]);
        let mut sync = PortalSync::default();
        let mut table = crate::scene::SpaceTable::new(vec![Mat4::IDENTITY, Mat4::IDENTITY]);
        let counts = sync.apply(&world, DVec3::ZERO, &mut table);
        let (views, chains) = counts.expect("the counts changed from nothing");
        assert!(views >= 2 && chains >= 2, "a view and a chain per direction ({views}, {chains})");
        assert!(table.matrices().len() > 2, "the projection's spaces came after the frame slot");
        assert_eq!(
            sync.apply(&world, DVec3::ZERO, &mut crate::scene::SpaceTable::new(vec![Mat4::IDENTITY, Mat4::IDENTITY])),
            None,
            "unchanged counts are not reported again"
        );
    }
}
