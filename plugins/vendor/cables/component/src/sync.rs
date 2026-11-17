//! Pairing connectors by cable ID, each frame, as portals pair by Link ID.
//!
//! [`CableSync`] is a space producer (`engine_backend::scene::SpaceProducer`):
//! the engine makes one per renderer and runs it every frame. It reads the
//! world's enabled [`CableConnectorComponent`]s and [`CableComponent`]
//! definitions, pairs every two connectors that share the ID of an enabled
//! definition, and publishes the frame's [`CableSpans`] under
//! [`CABLE_SPANS`]. The payload is replaced only when it changed, so a still
//! scene republishes nothing.
//!
//! Appearance and the texture slot stay on the GPU (the definition rows);
//! this only decides which two points each cable joins.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use cables_api::{CABLE_SPANS, CableSpan, CableSpans, MAX_BUNDLE_LANES};
use engine_backend::scene::{SpaceProducer, SpaceTable};
use glam::DVec3;
use pulsar_scene_model::Visibility;
use pulsar_scene_model::attachments::enabled_components;
use pulsar_scene_model::hierarchy_transform::world_matrix_of;
use pulsar_scenedb::World;

use crate::{CableComponent, CableConnectorComponent, CableRenderMode};

/// Connectors a single ID may pair before it is reported as crowded: `n`
/// connectors make `n(n-1)/2` cables.
const MAX_CONNECTORS_PER_ID: usize = 64;

/// The most spans one frame publishes.
const MAX_SPANS: usize = 4096;

/// Why a cable ID shows less than authored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CableProblem {
    /// An enabled connector's ID has no enabled definition.
    NoDefinition { id: u32 },
    /// Several enabled definitions share this ID (the first, by instance, wins).
    DuplicateDefinition { id: u32, count: usize },
    /// An ID with more connectors than [`MAX_CONNECTORS_PER_ID`].
    Crowded { id: u32, count: usize },
}

impl std::fmt::Display for CableProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDefinition { id } => write!(
                f,
                "cable ID {id} has connectors but no enabled cable definition"
            ),
            Self::DuplicateDefinition { id, count } => write!(
                f,
                "cable ID {id} is defined by {count} cables (the first is used)"
            ),
            Self::Crowded { id, count } => write!(
                f,
                "cable ID {id} has {count} connectors (at most {MAX_CONNECTORS_PER_ID} are paired)"
            ),
        }
    }
}

/// The spans of a world and the IDs that did not fully draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pairing {
    pub spans: CableSpans,
    pub problems: Vec<CableProblem>,
}

struct Definition {
    tube: bool,
    lanes: u32,
}

/// Pair the connectors of `world` by ID, relative to `origin`.
pub fn pair_cables(world: &World, origin: DVec3) -> Pairing {
    let visible = |owner| {
        world
            .get::<Visibility>(owner)
            .is_none_or(|visibility| visibility.visible)
    };

    let mut cables: Vec<_> = enabled_components::<CableComponent>(world)
        .filter(|(_, owner, cable)| cable.enabled && cable.id != 0 && visible(*owner))
        .map(|(instance, _, cable)| (instance.bits(), cable))
        .collect();
    cables.sort_by_key(|(bits, _)| *bits);
    let mut definitions: BTreeMap<u32, (Definition, usize)> = BTreeMap::new();
    for (_, cable) in cables {
        let tube = cable.render_mode == CableRenderMode::ThreeD;
        let definition = Definition {
            tube,
            lanes: if tube {
                cable.bundle_count.clamp(1, MAX_BUNDLE_LANES)
            } else {
                1
            },
        };
        definitions
            .entry(cable.id)
            .and_modify(|(_, count)| *count += 1)
            .or_insert((definition, 1));
    }

    let mut ends: BTreeMap<u32, Vec<(u64, [f32; 3])>> = BTreeMap::new();
    for (instance, owner, connector) in enabled_components::<CableConnectorComponent>(world) {
        if !connector.enabled || connector.id == 0 || !visible(owner) {
            continue;
        }
        let Some(matrix) = world_matrix_of(world, owner) else {
            continue;
        };
        let at = matrix.w_axis.truncate() - origin;
        if !at.is_finite() {
            continue;
        }
        ends.entry(connector.id)
            .or_default()
            .push((instance.bits(), at.as_vec3().to_array()));
    }

    let mut pairing = Pairing::default();
    for (id, (_, count)) in &definitions {
        if *count > 1 {
            pairing.problems.push(CableProblem::DuplicateDefinition {
                id: *id,
                count: *count,
            });
        }
    }
    for (id, mut points) in ends {
        let Some((definition, _)) = definitions.get(&id) else {
            pairing.problems.push(CableProblem::NoDefinition { id });
            continue;
        };
        // A stable order, so a cable keeps its identity while the scene is still.
        points.sort_by_key(|(bits, _)| *bits);
        if points.len() > MAX_CONNECTORS_PER_ID {
            pairing.problems.push(CableProblem::Crowded {
                id,
                count: points.len(),
            });
            points.truncate(MAX_CONNECTORS_PER_ID);
        }
        for (i, (_, from)) in points.iter().enumerate() {
            for (_, to) in &points[i + 1..] {
                if pairing.spans.0.len() == MAX_SPANS {
                    return pairing;
                }
                pairing.spans.0.push(CableSpan {
                    id,
                    from: *from,
                    to: *to,
                    tube: definition.tube,
                    lanes: definition.lanes,
                });
            }
        }
    }
    pairing
}

/// Builds the frame's cable spans each frame and publishes them to the
/// cable feature.
pub struct CableSync {
    /// Problems already reported, so a standing problem is logged once.
    reported: HashSet<CableProblem>,
    spans: Arc<CableSpans>,
}

impl Default for CableSync {
    fn default() -> Self {
        Self {
            reported: HashSet::new(),
            spans: Arc::new(CableSpans::default()),
        }
    }
}

impl SpaceProducer for CableSync {
    fn apply(
        &mut self,
        world: &World,
        origin: DVec3,
        _table: &mut SpaceTable,
        payloads: &mut helio_core::ScenePayloads,
    ) {
        let pairing = pair_cables(world, origin);
        let current: HashSet<CableProblem> = pairing.problems.iter().copied().collect();
        for problem in &current {
            if self.reported.insert(*problem) {
                tracing::warn!("{problem}");
            }
        }
        self.reported.retain(|problem| current.contains(problem));
        if *self.spans != pairing.spans {
            self.spans = Arc::new(pairing.spans);
        }
        payloads.insert(&CABLE_SPANS, self.spans.clone());
    }

    fn derivation(
        &self,
        _device: &wgpu::Device,
    ) -> Option<Box<dyn helio_scene_join::SceneDerivation>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_read_as_sentences() {
        assert!(
            CableProblem::NoDefinition { id: 3 }
                .to_string()
                .contains("no enabled cable definition")
        );
        assert!(
            CableProblem::Crowded { id: 3, count: 70 }
                .to_string()
                .contains("70 connectors")
        );
    }
}
