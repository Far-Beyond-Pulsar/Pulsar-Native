//! CPU-side resolution of indexed sublevels and peer-linked portals.
//!
//! This module is the authored-data boundary for the sublevel/portal system.
//! It consumes `SubLevelActorRow` and the authored peer-based
//! `PortalRow`, then produces resolved contexts and portal mappings for
//! the existing coordinate-space, portal-view, and portal-chain projection
//! code. It intentionally does not write SceneDB rows or GPU buffers.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use glam::{Mat4, Vec2, Vec4};
use pulsar_scenedb::Entity;

use helio_mesh_api::{SubLevelActorRow, SubLevelIndex, DEFAULT_SUBLEVEL_INDEX};

use crate::components::{PortalRow, PortalViewRow, NO_PORTAL_PEER};
use crate::portal_math::portal_view_map;

/// A component row together with its owning SceneDB entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubLevelActorRecord {
    /// Entity carrying the actor component.
    pub entity: Entity,
    /// Authored actor placement.
    pub component: SubLevelActorRow,
}

/// A component row together with its owning SceneDB entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalRecord {
    /// Entity carrying the portal component.
    pub entity: Entity,
    /// Authored portal relationship and local aperture.
    pub component: PortalRow,
}

/// Authored contents associated with one indexed sublevel.
///
/// The index itself is held by [`SubLevelResolver`]. A sublevel is content,
/// not an ECS component or a placement entity; actor components inside it are
/// what create nested runtime contexts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubLevelContents {
    /// Actors that place other indexed sublevels inside this sublevel.
    pub actors: Vec<SubLevelActorRecord>,
    /// Portals authored in this sublevel.
    pub portals: Vec<PortalRecord>,
}

impl SubLevelContents {
    /// Build contents from the corresponding SceneDB rows.
    pub fn new(
        actors: impl IntoIterator<Item = (Entity, SubLevelActorRow)>,
        portals: impl IntoIterator<Item = (Entity, PortalRow)>,
    ) -> Self {
        Self {
            actors: actors
                .into_iter()
                .map(|(entity, component)| SubLevelActorRecord { entity, component })
                .collect(),
            portals: portals
                .into_iter()
                .map(|(entity, component)| PortalRecord { entity, component })
                .collect(),
        }
    }
}

/// A resolved placement of one indexed sublevel.
#[derive(Clone, Debug, PartialEq)]
pub struct SubLevelRuntimeContext {
    /// Indexed content space being placed.
    pub sublevel_index: SubLevelIndex,
    /// Actor entities from the root to this placement, in traversal order.
    pub actor_path: Vec<Entity>,
    /// Transform from this sublevel's authored coordinates into the root
    /// level's coordinates.
    pub transform: Mat4,
}

/// A portal occurrence in one resolved runtime context.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalOccurrence {
    /// Authored portal entity.
    pub entity: Entity,
    /// The resolved sublevel/actor context containing the portal.
    pub context: SubLevelRuntimeContext,
    /// Portal-local to root-level transform for this occurrence.
    pub transform: Mat4,
    /// Portal aperture half extents in local X/Y.
    pub half_extent: [f32; 2],
    /// Authored peer entity.
    pub peer: Entity,
}

/// A resolved mapping that displays the target portal's context through the
/// source portal.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalProjection {
    /// Portal visible to the current view.
    pub source: PortalOccurrence,
    /// Peer portal whose context supplies the displayed contents.
    pub target: PortalOccurrence,
    /// Maps target-context coordinates into the source portal's render space.
    /// This is the standard target-content-to-source-opening view map, which
    /// includes the portal-local 180-degree turn needed to place the target
    /// level beyond the source opening. It is intentionally distinct from
    /// the teleport map used when an actor crosses a portal.
    pub target_to_source: Mat4,
}

impl PortalProjection {
    /// Convert the derived projection into the existing GPU portal-view row.
    ///
    /// The coordinate-space slot is assigned by the later projection bridge;
    /// this method deliberately does not allocate or mutate one.
    pub fn to_portal_view(&self, coordinate_space: u32) -> PortalViewRow {
        PortalViewRow {
            transform: self.source.transform.to_cols_array(),
            inverse_transform: self.source.transform.inverse().to_cols_array(),
            half_extent: self.source.half_extent,
            coordinate_space,
            _pad: 0,
        }
    }
}

/// A bounded recursive chain of portal projections.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPortalChain {
    /// Portal entities in outermost-to-deepest order.
    pub portals: Vec<Entity>,
    /// One target-to-source mapping for each portal link.
    pub projections: Vec<PortalProjection>,
    /// True when the authored peer graph continues beyond the requested
    /// depth and was intentionally truncated.
    pub truncated: bool,
}

/// Errors found while resolving authored sublevel and portal relationships.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionError {
    /// An actor references a sublevel with no registered content.
    MissingSubLevel {
        /// Referenced index.
        index: SubLevelIndex,
        /// Actor that made the reference.
        actor: Entity,
    },
    /// An actor path revisited an index, which would recurse forever.
    ActorCycle {
        /// Repeated index.
        repeated: SubLevelIndex,
        /// Index path at the point of detection.
        path: Vec<SubLevelIndex>,
    },
    /// An enabled portal has no peer handle.
    MissingPortalPeer {
        /// Portal without a peer.
        source: Entity,
    },
    /// A peer handle does not identify an indexed portal.
    UnknownPortalPeer {
        /// Source portal.
        source: Entity,
        /// Missing peer.
        peer: Entity,
    },
    /// Portal recursion depth must be non-zero. The upper bound is supplied
    /// by the caller at runtime; there is no fixed-depth GPU ABI anymore.
    InvalidRecursionDepth {
        /// Requested depth.
        requested: usize,
        /// Maximum supported depth.
        maximum: usize,
    },
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSubLevel { index, actor } => {
                write!(f, "actor {actor:?} references missing sublevel {index}")
            }
            Self::ActorCycle { repeated, path } => {
                write!(f, "sublevel actor cycle at {repeated}: {path:?}")
            }
            Self::MissingPortalPeer { source } => {
                write!(f, "portal {source:?} has no peer")
            }
            Self::UnknownPortalPeer { source, peer } => {
                write!(f, "portal {source:?} references unknown peer {peer:?}")
            }
            Self::InvalidRecursionDepth { requested, maximum } => {
                write!(
                    f,
                    "portal recursion depth {requested} exceeds maximum {maximum}"
                )
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Resolves indexed sublevel content into runtime contexts and peer-linked
/// portal projections.
#[derive(Clone, Debug)]
pub struct SubLevelResolver {
    contents: BTreeMap<SubLevelIndex, SubLevelContents>,
}

impl Default for SubLevelResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SubLevelResolver {
    /// Create a resolver with the default level (sublevel zero) present as an
    /// identity content space.
    pub fn new() -> Self {
        let mut contents = BTreeMap::new();
        contents.insert(DEFAULT_SUBLEVEL_INDEX, SubLevelContents::default());
        Self { contents }
    }

    /// Replace or insert the authored contents for an indexed sublevel.
    pub fn insert_sublevel(&mut self, index: SubLevelIndex, contents: SubLevelContents) {
        self.contents.insert(index, contents);
    }

    /// Return authored contents for an index.
    pub fn sublevel(&self, index: SubLevelIndex) -> Option<&SubLevelContents> {
        self.contents.get(&index)
    }

    /// Resolve the root and every reachable direct/nested actor placement.
    pub fn resolve_contexts(&self) -> Result<Vec<SubLevelRuntimeContext>, ResolutionError> {
        let mut resolved = Vec::new();
        let root = SubLevelRuntimeContext {
            sublevel_index: DEFAULT_SUBLEVEL_INDEX,
            actor_path: Vec::new(),
            transform: Mat4::IDENTITY,
        };
        resolved.push(root.clone());
        self.walk_contexts(&root, &mut vec![DEFAULT_SUBLEVEL_INDEX], &mut resolved)?;
        Ok(resolved)
    }

    /// Resolve all portal occurrences in directly placed contexts.
    pub fn resolve_portals(&self) -> Result<Vec<PortalOccurrence>, ResolutionError> {
        let contexts = self.resolve_contexts()?;
        let mut occurrences = Vec::new();
        for context in contexts {
            let Some(contents) = self.contents.get(&context.sublevel_index) else {
                return Err(ResolutionError::MissingSubLevel {
                    index: context.sublevel_index,
                    actor: context
                        .actor_path
                        .last()
                        .copied()
                        .unwrap_or(Entity::DANGLING),
                });
            };
            for record in &contents.portals {
                if !record.component.is_enabled() {
                    continue;
                }
                let Some(peer) = record.component.peer_entity() else {
                    return Err(ResolutionError::MissingPortalPeer {
                        source: record.entity,
                    });
                };
                occurrences.push(PortalOccurrence {
                    entity: record.entity,
                    context: context.clone(),
                    transform: context.transform * record.component.transform(),
                    half_extent: record.component.half_extent,
                    peer,
                });
            }
        }
        Ok(occurrences)
    }

    /// Resolve the first-hop source/peer mappings for all directly visible
    /// portal occurrences. A peer in an uninstanced indexed sublevel receives
    /// a canonical identity context so portals do not require a cosmetic
    /// `SubLevelActor` merely to establish a cross-sublevel relationship.
    pub fn resolve_projections(&self) -> Result<Vec<PortalProjection>, ResolutionError> {
        let occurrences = self.resolve_portals()?;
        let index = self.portal_index();
        let mut projections = Vec::new();
        for source in occurrences.iter().cloned() {
            let targets = self.resolve_peer_targets(&source, &occurrences, &index)?;
            projections.extend(targets.into_iter().map(|target| PortalProjection {
                target_to_source: portal_view_map(source.transform, target.transform),
                source: source.clone(),
                target,
            }));
        }
        Ok(projections)
    }

    /// Resolve bounded recursive portal chains. Loops are valid portal
    /// topology; the caller-supplied depth is what makes traversal finite.
    /// Every prefix is returned: a recursive portal image is built from the
    /// one-hop, two-hop, ... views, not only from the deepest walk.
    ///
    /// After a projection through `source` into its peer's context, the next
    /// hop is any portal authored in that target context except the entry
    /// peer itself. Following the peer immediately would just compose a
    /// portal map with its inverse (`A -> B -> A`), which collapses the image
    /// back onto the camera and makes every ordinary two-way pair appear to
    /// stop at one recursion. Continuing through the target context is the
    /// same rule used by a real portal view: render the target level, then
    /// follow whichever portals are present in that level.
    pub fn resolve_chains(
        &self,
        max_depth: usize,
    ) -> Result<Vec<ResolvedPortalChain>, ResolutionError> {
        self.resolve_chains_bounded(max_depth, None)
    }

    /// Resolve recursive chains with an optional runtime row budget.
    ///
    /// A highly connected portal graph can have exponentially many valid
    /// paths. `max_chains` is therefore a work/allocation budget, not a
    /// recursion-depth cap. Bounded traversal is deterministic and
    /// breadth-first: complete recursion layers are emitted first, and a
    /// partial layer is sampled evenly across the whole frontier. That keeps
    /// a bounded frame spatially balanced instead of spending its budget on
    /// one arbitrary deep branch.
    pub fn resolve_chains_bounded(
        &self,
        max_depth: usize,
        max_chains: Option<usize>,
    ) -> Result<Vec<ResolvedPortalChain>, ResolutionError> {
        self.resolve_chains_internal(max_depth, max_chains, None, None)
    }

    /// Resolve recursive chains while pruning branches whose portal aperture
    /// cannot intersect the current camera view. This is deliberately a
    /// conservative screen-space test: an aperture that crosses the camera
    /// near plane is retained, while a branch is expanded only when its
    /// projected rectangle overlaps the visible rectangle inherited from its
    /// parent portal.
    ///
    /// This stage must run before chain rows are materialized. GPU instance
    /// culling cannot prevent the CPU from exploding a highly connected graph
    /// if every mathematical path has already been serialized into a chain.
    pub fn resolve_chains_visible(
        &self,
        max_depth: usize,
        max_chains: Option<usize>,
        view_projection: Mat4,
    ) -> Result<Vec<ResolvedPortalChain>, ResolutionError> {
        self.resolve_chains_internal(max_depth, max_chains, Some(view_projection), None)
    }

    /// Variant of [`Self::resolve_chains_visible`] that also supplies the
    /// camera position. The extra point lets traversal reject back-facing
    /// apertures in world space instead of relying on projected winding,
    /// which is ambiguous when a portal crosses the near plane.
    pub fn resolve_chains_visible_from_camera(
        &self,
        max_depth: usize,
        max_chains: Option<usize>,
        view_projection: Mat4,
        camera_position: glam::Vec3,
    ) -> Result<Vec<ResolvedPortalChain>, ResolutionError> {
        self.resolve_chains_internal(
            max_depth,
            max_chains,
            Some(view_projection),
            Some(camera_position),
        )
    }

    fn resolve_chains_internal(
        &self,
        max_depth: usize,
        max_chains: Option<usize>,
        view_projection: Option<Mat4>,
        camera_position: Option<glam::Vec3>,
    ) -> Result<Vec<ResolvedPortalChain>, ResolutionError> {
        if max_depth == 0 {
            return Err(ResolutionError::InvalidRecursionDepth {
                requested: max_depth,
                maximum: usize::MAX,
            });
        }
        if max_chains == Some(0) {
            return Ok(Vec::new());
        }

        let occurrences = self.resolve_portals()?;
        let index = self.portal_index();

        #[derive(Clone)]
        struct ChainState {
            source: PortalOccurrence,
            portals: Vec<Entity>,
            projections: Vec<PortalProjection>,
            context_to_outer: Mat4,
            visible_rect: ScreenRect,
        }

        // One frontier is one recursion depth. Keeping it separate from the
        // output is what lets the bounded path select remain layer-balanced.
        let mut frontier: Vec<ChainState> = occurrences
            .iter()
            .cloned()
            .map(|source| ChainState {
                source,
                portals: Vec::new(),
                projections: Vec::new(),
                context_to_outer: Mat4::IDENTITY,
                visible_rect: ScreenRect::full(),
            })
            .collect();
        let mut chains = Vec::new();

        for _depth in 0..max_depth {
            if frontier.is_empty() {
                break;
            }

            let mut layer = Vec::new();
            let mut next_frontier = Vec::new();
            for state in frontier.drain(..) {
                let ChainState {
                    source,
                    mut portals,
                    mut projections,
                    context_to_outer,
                    visible_rect,
                } = state;
                let Some(source_rect) = view_projection
                    .and_then(|view_projection| {
                        portal_screen_rect(
                            &source,
                            context_to_outer,
                            view_projection,
                            camera_position,
                        )
                    })
                    .map(|rect| visible_rect.intersect(rect))
                    .flatten()
                    .or_else(|| view_projection.is_none().then_some(visible_rect))
                else {
                    continue;
                };
                portals.push(source.entity);
                let targets = self.resolve_peer_targets(&source, &occurrences, &index)?;
                for target in targets {
                    projections.push(PortalProjection {
                        target_to_source: portal_view_map(source.transform, target.transform),
                        source: source.clone(),
                        target: target.clone(),
                    });
                    layer.push(ResolvedPortalChain {
                        portals: portals.clone(),
                        projections: projections.clone(),
                        truncated: portals.len() == max_depth,
                    });

                    if portals.len() < max_depth {
                        let child_context_to_outer = context_to_outer
                            * projections
                                .last()
                                .expect("projection was pushed for this target")
                                .target_to_source;
                        let next_sources = self
                            .portals_in_context(&target.context, &occurrences)
                            .into_iter()
                            .filter(|candidate| candidate.entity != target.entity)
                            .filter_map(|next_source| {
                                let next_visible_rect = view_projection
                                    .and_then(|view_projection| {
                                        portal_screen_rect(
                                            &next_source,
                                            child_context_to_outer,
                                            view_projection,
                                            camera_position,
                                        )
                                    })
                                    .and_then(|rect| source_rect.intersect(rect))
                                    .or_else(|| view_projection.is_none().then_some(source_rect))?;
                                Some(ChainState {
                                    source: next_source,
                                    portals: portals.clone(),
                                    projections: projections.clone(),
                                    context_to_outer: child_context_to_outer,
                                    visible_rect: next_visible_rect,
                                })
                            });
                        next_frontier.extend(next_sources);
                    }
                    projections.pop();
                }
            }

            let Some(limit) = max_chains else {
                chains.extend(layer);
                frontier = next_frontier;
                continue;
            };

            let remaining = limit.saturating_sub(chains.len());
            if remaining == 0 {
                break;
            }
            if layer.len() <= remaining {
                chains.extend(layer);
                frontier = next_frontier;
            } else {
                // The layer is ordered by root and then by parent branch. A
                // strided selection gives every branch a share of the
                // remaining budget and is deterministic across frames.
                chains.extend(select_evenly(layer, remaining));
                break;
            }
        }

        Ok(chains)
    }

    fn walk_contexts(
        &self,
        context: &SubLevelRuntimeContext,
        ancestry: &mut Vec<SubLevelIndex>,
        resolved: &mut Vec<SubLevelRuntimeContext>,
    ) -> Result<(), ResolutionError> {
        let contents = self
            .contents
            .get(&context.sublevel_index)
            .expect("resolved context must have registered contents");
        for actor in &contents.actors {
            if !actor.component.is_enabled() {
                continue;
            }
            let child = actor.component.sublevel_index;
            if ancestry.contains(&child) {
                return Err(ResolutionError::ActorCycle {
                    repeated: child,
                    path: ancestry.clone(),
                });
            }
            if !self.contents.contains_key(&child) {
                return Err(ResolutionError::MissingSubLevel {
                    index: child,
                    actor: actor.entity,
                });
            }
            let mut actor_path = context.actor_path.clone();
            actor_path.push(actor.entity);
            let child_context = SubLevelRuntimeContext {
                sublevel_index: child,
                actor_path,
                transform: context.transform * actor.component.transform(),
            };
            resolved.push(child_context.clone());
            ancestry.push(child);
            self.walk_contexts(&child_context, ancestry, resolved)?;
            ancestry.pop();
        }
        Ok(())
    }

    fn portal_index(&self) -> HashMap<Entity, (SubLevelIndex, PortalRecord)> {
        self.contents
            .iter()
            .flat_map(|(&index, contents)| {
                contents
                    .portals
                    .iter()
                    .copied()
                    .map(move |record| (record.entity, (index, record)))
            })
            .collect()
    }

    fn resolve_peer_targets(
        &self,
        source: &PortalOccurrence,
        occurrences: &[PortalOccurrence],
        portal_index: &HashMap<Entity, (SubLevelIndex, PortalRecord)>,
    ) -> Result<Vec<PortalOccurrence>, ResolutionError> {
        let Some(&(target_index, target_record)) = portal_index.get(&source.peer) else {
            return Err(ResolutionError::UnknownPortalPeer {
                source: source.entity,
                peer: source.peer,
            });
        };

        // A same-sublevel peer belongs to the exact same actor placement as
        // the source. This is what makes portals in one indexed level pair
        // correctly even when that level is nested/instanced.
        if target_index == source.context.sublevel_index {
            return Ok(vec![
                self.occurrence_in_context(target_record, source.context.clone())
            ]);
        }

        // Cross-sublevel peers may have multiple actor placements. Preserve
        // each reachable occurrence; if none is placed, use the target
        // sublevel's canonical local context.
        let placed: Vec<_> = occurrences
            .iter()
            .filter(|occurrence| occurrence.entity == source.peer)
            .cloned()
            .collect();
        if !placed.is_empty() {
            return Ok(placed);
        }

        Ok(vec![self.occurrence_in_context(
            target_record,
            SubLevelRuntimeContext {
                sublevel_index: target_index,
                actor_path: Vec::new(),
                transform: Mat4::IDENTITY,
            },
        )])
    }

    fn occurrence_in_context(
        &self,
        record: PortalRecord,
        context: SubLevelRuntimeContext,
    ) -> PortalOccurrence {
        let peer = record
            .component
            .peer_entity()
            .unwrap_or_else(|| Entity::from_bits(NO_PORTAL_PEER));
        PortalOccurrence {
            entity: record.entity,
            transform: context.transform * record.component.transform(),
            half_extent: record.component.half_extent,
            peer,
            context,
        }
    }

    fn context_matches(left: &SubLevelRuntimeContext, right: &SubLevelRuntimeContext) -> bool {
        left.sublevel_index == right.sublevel_index && left.actor_path == right.actor_path
    }

    /// Return the portals that can be seen from a resolved target context.
    ///
    /// Reachable actor placements already occur in `occurrences`. A
    /// cross-sublevel peer can instead resolve to a canonical, uninstanced
    /// context; in that case materialize that context from the indexed
    /// sublevel contents so it can still continue through its other portals.
    fn portals_in_context(
        &self,
        context: &SubLevelRuntimeContext,
        occurrences: &[PortalOccurrence],
    ) -> Vec<PortalOccurrence> {
        let mut candidates: Vec<_> = occurrences
            .iter()
            .filter(|occurrence| Self::context_matches(&occurrence.context, context))
            .cloned()
            .collect();
        if !candidates.is_empty() {
            return candidates;
        }

        let Some(contents) = self.contents.get(&context.sublevel_index) else {
            return candidates;
        };
        candidates.extend(contents.portals.iter().filter_map(|record| {
            record
                .component
                .is_enabled()
                .then(|| self.occurrence_in_context(*record, context.clone()))
        }));
        candidates
    }
}

#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    min: Vec2,
    max: Vec2,
}

impl ScreenRect {
    fn full() -> Self {
        Self {
            min: Vec2::splat(-1.0),
            max: Vec2::splat(1.0),
        }
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let rect = Self {
            min: self.min.max(other.min),
            max: self.max.min(other.max),
        };
        (rect.min.x <= rect.max.x && rect.min.y <= rect.max.y).then_some(rect)
    }
}

fn portal_screen_rect(
    portal: &PortalOccurrence,
    context_to_outer: Mat4,
    view_projection: Mat4,
    camera_position: Option<glam::Vec3>,
) -> Option<ScreenRect> {
    let transform = context_to_outer * portal.transform;
    if let Some(camera_position) = camera_position {
        let center = transform.w_axis.truncate();
        let front = -transform.z_axis.truncate().normalize();
        if (camera_position - center).dot(front) <= 0.0 {
            return None;
        }
    }
    let corners = [
        Vec2::new(-portal.half_extent[0], -portal.half_extent[1]),
        Vec2::new(portal.half_extent[0], -portal.half_extent[1]),
        Vec2::new(portal.half_extent[0], portal.half_extent[1]),
        Vec2::new(-portal.half_extent[0], portal.half_extent[1]),
    ];
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let mut has_front_corner = false;
    let mut has_visible_depth = false;
    let mut crosses_near_plane = false;
    for corner in corners {
        let clip = view_projection * transform * Vec4::new(corner.x, corner.y, 0.0, 1.0);
        // A portal crossing the near plane is conservatively retained. The
        // recursive renderer, rather than this coarse traversal, owns the
        // exact aperture/half-space test.
        if clip.w <= 1e-5 {
            crosses_near_plane = true;
            continue;
        }
        has_front_corner = true;
        let ndc = clip.truncate() / clip.w;
        has_visible_depth |= (-0.001..=1.001).contains(&ndc.z);
        let ndc_xy = ndc.truncate();
        min = min.min(ndc_xy);
        max = max.max(ndc_xy);
    }
    if !has_front_corner {
        return None;
    }
    if !has_visible_depth {
        return None;
    }
    if crosses_near_plane {
        // A small portal should not straddle the camera plane during ordinary
        // recursive traversal. Keep only a genuinely front-facing crossing;
        // a back-facing/edge-on crossing is a numerical artifact and is a
        // particularly bad source of combinatorial expansion.
        let center_clip = view_projection * transform * Vec4::W;
        return (center_clip.w > 1e-5).then_some(ScreenRect::full());
    }
    let rect = ScreenRect {
        min: min.max(Vec2::splat(-1.0)),
        max: max.min(Vec2::splat(1.0)),
    };
    (rect.min.x <= rect.max.x && rect.min.y <= rect.max.y).then_some(rect)
}

fn select_evenly<T>(items: Vec<T>, count: usize) -> Vec<T> {
    debug_assert!(count > 0 && count < items.len());
    let total = items.len();
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            // Select the item that crosses each evenly spaced budget
            // boundary. This avoids biasing the first branch when the layer
            // is not divisible by the budget.
            let selected = ((index * count) / total) != (((index + 1) * count) / total);
            selected.then_some(item)
        })
        .take(count)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(
        world: &mut pulsar_scenedb::World,
        index: SubLevelIndex,
        transform: Mat4,
    ) -> SubLevelActorRecord {
        let entity = world.spawn();
        SubLevelActorRecord {
            entity,
            component: SubLevelActorRow::new(index, transform),
        }
    }

    #[test]
    fn index_zero_is_identity_context() {
        let resolver = SubLevelResolver::new();
        let contexts = resolver.resolve_contexts().unwrap();
        assert_eq!(
            contexts,
            vec![SubLevelRuntimeContext {
                sublevel_index: DEFAULT_SUBLEVEL_INDEX,
                actor_path: Vec::new(),
                transform: Mat4::IDENTITY,
            }]
        );
    }

    #[test]
    fn nested_actor_transforms_compose_in_path_order() {
        let mut world = pulsar_scenedb::World::new();
        let outer = actor(
            &mut world,
            1,
            Mat4::from_translation(glam::vec3(10.0, 0.0, 0.0)),
        );
        let inner = actor(
            &mut world,
            2,
            Mat4::from_translation(glam::vec3(0.0, 5.0, 0.0)),
        );

        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents {
                actors: vec![outer],
                portals: Vec::new(),
            },
        );
        resolver.insert_sublevel(
            1,
            SubLevelContents {
                actors: vec![inner],
                portals: Vec::new(),
            },
        );
        resolver.insert_sublevel(2, SubLevelContents::default());

        let contexts = resolver.resolve_contexts().unwrap();
        let nested = contexts
            .iter()
            .find(|context| context.sublevel_index == 2)
            .unwrap();
        assert_eq!(nested.actor_path, vec![outer.entity, inner.entity]);
        assert_eq!(
            nested.transform.transform_point3(glam::Vec3::ZERO),
            glam::vec3(10.0, 5.0, 0.0)
        );
    }

    #[test]
    fn same_sublevel_portals_pair_in_their_runtime_context() {
        let mut world = pulsar_scenedb::World::new();
        let a_entity = world.spawn();
        let b_entity = world.spawn();
        let a = PortalRecord {
            entity: a_entity,
            component: PortalRow::new(
                Some(b_entity),
                Mat4::from_translation(glam::vec3(0.0, 0.0, 0.0)),
                [1.0, 2.0],
            ),
        };
        let b = PortalRecord {
            entity: b_entity,
            component: PortalRow::new(
                Some(a_entity),
                Mat4::from_translation(glam::vec3(10.0, 0.0, 0.0)),
                [1.0, 2.0],
            ),
        };
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents {
                actors: Vec::new(),
                portals: vec![a, b],
            },
        );

        let projection = resolver
            .resolve_projections()
            .unwrap()
            .into_iter()
            .find(|projection| projection.source.entity == a_entity)
            .unwrap();
        assert_eq!(projection.target.entity, b_entity);
        assert_eq!(projection.target.context.sublevel_index, 0);
        assert_eq!(projection.target.context.actor_path, Vec::<Entity>::new());
        assert_eq!(
            projection
                .target_to_source
                .transform_point3(glam::vec3(10.0, 0.0, 0.0)),
            glam::Vec3::ZERO
        );
    }

    #[test]
    fn recursive_chains_continue_through_the_target_context() {
        let mut world = pulsar_scenedb::World::new();
        let a_entity = world.spawn();
        let b_entity = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        a_entity,
                        PortalRow::new(
                            Some(b_entity),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, 6.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        b_entity,
                        PortalRow::new(
                            Some(a_entity),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, -6.0)),
                            [1.0, 1.0],
                        ),
                    ),
                ],
            ),
        );

        let chains = resolver.resolve_chains(3).unwrap();
        assert!(chains
            .iter()
            .any(|chain| chain.portals == vec![a_entity, a_entity, a_entity]));
        assert!(chains.iter().all(|chain| {
            chain
                .portals
                .windows(2)
                .all(|pair| pair != [a_entity, b_entity] && pair != [b_entity, a_entity])
        }));
    }

    #[test]
    fn bounded_recursion_fills_even_layers_before_sampling_frontier() {
        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (a, PortalRow::new(Some(b), Mat4::IDENTITY, [1.0, 1.0])),
                    (b, PortalRow::new(Some(a), Mat4::IDENTITY, [1.0, 1.0])),
                ],
            ),
        );

        let chains = resolver.resolve_chains_bounded(10, Some(10)).unwrap();
        assert_eq!(chains.len(), 10);
        assert_eq!(
            chains
                .iter()
                .filter(|chain| chain.portals.len() == 1)
                .count(),
            2
        );
        assert_eq!(
            chains
                .iter()
                .filter(|chain| chain.portals.len() == 5)
                .count(),
            2
        );
        assert!(chains.iter().all(|chain| chain.portals.len() <= 5));
    }

    #[test]
    fn uncapped_recursion_reaches_requested_runtime_depth() {
        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (a, PortalRow::new(Some(b), Mat4::IDENTITY, [1.0, 1.0])),
                    (b, PortalRow::new(Some(a), Mat4::IDENTITY, [1.0, 1.0])),
                ],
            ),
        );

        let chains = resolver.resolve_chains(10).unwrap();
        assert_eq!(chains.len(), 20);
        assert!(chains.iter().any(|chain| chain.portals.len() == 10));
    }

    #[test]
    fn visible_recursion_prunes_portals_behind_camera_before_expansion() {
        let mut world = pulsar_scenedb::World::new();
        let front = world.spawn();
        let front_peer = world.spawn();
        let back = world.spawn();
        let back_peer = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        front,
                        PortalRow::new(
                            Some(front_peer),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, -4.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        front_peer,
                        PortalRow::new(
                            Some(front),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, -8.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        back,
                        PortalRow::new(
                            Some(back_peer),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, 4.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        back_peer,
                        PortalRow::new(
                            Some(back),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, 8.0)),
                            [1.0, 1.0],
                        ),
                    ),
                ],
            ),
        );

        let view_projection = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_2,
            1.0,
            0.1,
            100.0,
        );
        let chains = resolver
            .resolve_chains_visible(3, None, view_projection)
            .unwrap();
        assert!(!chains.iter().any(|chain| chain.portals[0] == back));
        assert!(!chains.iter().any(|chain| chain.portals[0] == back_peer));
        assert!(!chains.is_empty());
    }

    #[test]
    fn visible_cube_frontier_does_not_expand_every_wall_path() {
        let mut world = pulsar_scenedb::World::new();
        let entities: Vec<_> = (0..6).map(|_| world.spawn()).collect();
        let faces = [
            (glam::Vec3::X, glam::Vec3::Y),
            (glam::Vec3::NEG_X, glam::Vec3::Y),
            (glam::Vec3::Y, glam::Vec3::Z),
            (glam::Vec3::NEG_Y, glam::Vec3::Z),
            (glam::Vec3::Z, glam::Vec3::Y),
            (glam::Vec3::NEG_Z, glam::Vec3::Y),
        ];
        let opposite = [1usize, 0, 3, 2, 5, 4];
        let mut portals = Vec::new();
        for (index, (normal, up_hint)) in faces.into_iter().enumerate() {
            let right = up_hint.cross(normal).normalize();
            let up = normal.cross(right).normalize();
            let pose = crate::portal_math::portal_pose_facing(normal * 6.0, -normal, up);
            portals.push((
                entities[index],
                PortalRow::new(Some(entities[opposite[index]]), pose.transform, [1.6, 1.6]),
            ));
        }
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(0, SubLevelContents::new([], portals));
        let position = glam::Vec3::new(4.0, 3.0, 4.0);
        let forward = glam::Vec3::new(
            -std::f32::consts::FRAC_1_SQRT_2 * 0.883,
            -0.469,
            -std::f32::consts::FRAC_1_SQRT_2 * 0.883,
        )
        .normalize();
        let view =
            glam::camera::rh::view::look_at_mat4(position, position + forward, glam::Vec3::Y);
        let projection = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_4,
            1280.0 / 720.0,
            0.1,
            300.0,
        );
        let chains = resolver
            .resolve_chains_visible_from_camera(10, None, projection * view, position)
            .unwrap();
        assert!(chains.len() < 10_000);
    }

    #[test]
    fn cross_sublevel_portal_uses_target_canonical_context() {
        let mut world = pulsar_scenedb::World::new();
        let source_entity = world.spawn();
        let target_entity = world.spawn();
        let source = PortalRecord {
            entity: source_entity,
            component: PortalRow::new(Some(target_entity), Mat4::IDENTITY, [1.0, 1.0]),
        };
        let target = PortalRecord {
            entity: target_entity,
            component: PortalRow::new(
                Some(source_entity),
                Mat4::from_translation(glam::vec3(20.0, 0.0, 0.0)),
                [1.0, 1.0],
            ),
        };
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents {
                actors: Vec::new(),
                portals: vec![source],
            },
        );
        resolver.insert_sublevel(
            7,
            SubLevelContents {
                actors: Vec::new(),
                portals: vec![target],
            },
        );

        let projection = resolver
            .resolve_projections()
            .unwrap()
            .into_iter()
            .find(|projection| projection.source.entity == source_entity)
            .unwrap();
        assert_eq!(projection.target.context.sublevel_index, 7);
        assert!(projection.target.context.actor_path.is_empty());
        assert_eq!(
            projection
                .target_to_source
                .transform_point3(glam::Vec3::new(20.0, 0.0, 0.0)),
            glam::Vec3::ZERO
        );
    }

    #[test]
    fn actor_cycles_and_missing_peers_are_rejected() {
        let mut world = pulsar_scenedb::World::new();
        let cycle_actor = actor(&mut world, 1, Mat4::IDENTITY);
        let back_actor = actor(&mut world, 0, Mat4::IDENTITY);
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents {
                actors: vec![cycle_actor],
                portals: Vec::new(),
            },
        );
        resolver.insert_sublevel(
            1,
            SubLevelContents {
                actors: vec![back_actor],
                portals: Vec::new(),
            },
        );
        assert!(matches!(
            resolver.resolve_contexts(),
            Err(ResolutionError::ActorCycle { .. })
        ));

        let missing_peer = world.spawn();
        let mut peer_resolver = SubLevelResolver::new();
        peer_resolver.insert_sublevel(
            0,
            SubLevelContents {
                actors: Vec::new(),
                portals: vec![PortalRecord {
                    entity: missing_peer,
                    component: PortalRow::new(None, Mat4::IDENTITY, [1.0, 1.0]),
                }],
            },
        );
        assert!(matches!(
            peer_resolver.resolve_projections(),
            Err(ResolutionError::MissingPortalPeer { source }) if source == missing_peer
        ));
    }

    #[test]
    fn recursion_depth_is_runtime_configured() {
        let resolver = SubLevelResolver::new();
        assert!(resolver.resolve_chains(4).is_ok());
        assert!(matches!(
            resolver.resolve_chains(0),
            Err(ResolutionError::InvalidRecursionDepth { .. })
        ));
    }
}
