//! Voxel worlds placed in a scene: a [`VoxelVolume`] is a world (its
//! recipe and edits, a [`Planet`]) and where it is, a [`VolumeTransform`] in
//! f64. A planet, a moon, a plane and a ship-sized block are all volumes;
//! any number share one renderer (`chunks::ChunkWorld`).
//!
//! A world's own frame is its grid's (planet-centred for a sphere, the
//! plane's for a plane): every cell, query and edit is in it. The transform
//! places that frame in the scene, a rotation then a translation, and may
//! change every frame (orbits, spin, a ship flying). World positions are
//! resolved by transform ([`Volumes`]): a ray is moved into each volume's
//! frame, and the nearest hit wins.

use crate::grid::Shape;
use crate::planet::{Matter, Planet, RayHit};
use glam::{DMat3, DQuat, DVec3};
use std::sync::Arc;

/// Where a volume's frame is in the world: `world = position + rotation *
/// local`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeTransform {
    pub position: DVec3,
    pub rotation: DQuat,
}

impl Default for VolumeTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl VolumeTransform {
    /// The world's frame itself (a planet centred at the origin).
    pub const IDENTITY: Self = Self {
        position: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
    };

    pub fn new(position: DVec3, rotation: DQuat) -> Self {
        Self {
            position,
            rotation: rotation.normalize(),
        }
    }

    pub fn from_position(position: DVec3) -> Self {
        Self {
            position,
            rotation: DQuat::IDENTITY,
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// A world point in the volume's frame.
    pub fn to_local(&self, world: DVec3) -> DVec3 {
        self.rotation.inverse() * (world - self.position)
    }

    /// A point of the volume's frame in the world.
    pub fn to_world(&self, local: DVec3) -> DVec3 {
        self.position + self.rotation * local
    }

    /// A world direction in the volume's frame.
    pub fn dir_to_local(&self, world: DVec3) -> DVec3 {
        self.rotation.inverse() * world
    }

    /// A direction of the volume's frame in the world.
    pub fn dir_to_world(&self, local: DVec3) -> DVec3 {
        self.rotation * local
    }

    /// The rotation as a matrix (local to world: its columns are the
    /// volume's axes in the world).
    pub fn matrix(&self) -> DMat3 {
        DMat3::from_quat(self.rotation)
    }
}

/// A voxel world placed in the scene. `id` is the host's, stable while the
/// volume lives (picks and hits name it).
#[derive(Clone)]
pub struct VoxelVolume {
    pub id: u64,
    pub planet: Arc<Planet>,
    pub transform: VolumeTransform,
}

impl VoxelVolume {
    pub fn new(id: u64, planet: Arc<Planet>, transform: VolumeTransform) -> Self {
        Self {
            id,
            planet,
            transform,
        }
    }
}

/// The radius about a world's own origin holding every solid cell it can
/// have (terrain, features and edits), or `None` where it is unbounded (an
/// infinite plane).
pub fn bounding_radius(planet: &Planet) -> Option<f64> {
    let grid = planet.grid();
    match grid.shape() {
        Shape::Sphere => Some(planet.outer_radius()),
        Shape::InfinitePlane => None,
        Shape::Plane => {
            let half = f64::from(grid.cells()) * grid.voxel_size() * 0.5;
            let height = planet.outer_radius().abs().max(planet.inner_radius().abs());
            Some((2.0 * half * half + height * height).sqrt())
        }
    }
}

/// Where a ray from `origin` along unit `dir` meets a ball of `radius` about
/// `centre`: the distances it enters and leaves it (entering may be behind
/// the origin), or none.
pub fn ray_ball(origin: DVec3, dir: DVec3, centre: DVec3, radius: f64) -> Option<(f64, f64)> {
    let o = origin - centre;
    let b = o.dot(dir);
    let c = o.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    Some((-b - root, -b + root))
}

/// A hit in a set of volumes: the volume (its index in the set and its id),
/// the hit in its frame (`RayHit`: cells, distance; its normal the frame's)
/// and the normal in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeHit {
    pub index: usize,
    pub id: u64,
    pub hit: RayHit,
    pub normal: DVec3,
}

/// A set of placed volumes, queried by world positions.
#[derive(Clone, Default)]
pub struct Volumes {
    pub volumes: Vec<VoxelVolume>,
}

impl Volumes {
    pub fn new(volumes: Vec<VoxelVolume>) -> Self {
        Self { volumes }
    }

    /// The nearest hit of a ray from world `origin` along `direction`
    /// within `max_distance`, over every volume it can meet (each cast in
    /// its own frame: distances are the same in every frame).
    pub fn raycast(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_distance: f64,
        matter: Matter,
    ) -> Option<VolumeHit> {
        let dir = direction.normalize();
        let mut best: Option<VolumeHit> = None;
        for (index, v) in self.volumes.iter().enumerate() {
            let limit = best.map_or(max_distance, |b| b.hit.distance);
            if let Some(radius) = bounding_radius(&v.planet) {
                match ray_ball(origin, dir, v.transform.position, radius) {
                    Some((enter, leave)) if leave >= 0.0 && enter <= limit => {}
                    _ => continue,
                }
            }
            let t = &v.transform;
            if let Some(hit) =
                v.planet
                    .raycast_of(t.to_local(origin), t.dir_to_local(dir), limit, matter)
            {
                if hit.distance <= limit {
                    best = Some(VolumeHit {
                        index,
                        id: v.id,
                        hit,
                        normal: t.dir_to_world(hit.normal),
                    });
                }
            }
        }
        best
    }

    /// The volumes whose bounds hold world point `p`, nearest centre first,
    /// with the point in each one's frame.
    pub fn locate(&self, p: DVec3) -> Vec<(usize, DVec3)> {
        let mut out: Vec<(f64, usize, DVec3)> = self
            .volumes
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                bounding_radius(&v.planet).is_none_or(|r| p.distance(v.transform.position) <= r)
            })
            .map(|(i, v)| (p.distance(v.transform.position), i, v.transform.to_local(p)))
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|(_, i, l)| (i, l)).collect()
    }

    /// Whether world point `p` is in a solid cell of any volume.
    pub fn solid(&self, p: DVec3, matter: Matter) -> bool {
        self.locate(p).into_iter().any(|(i, local)| {
            let planet = &self.volumes[i].planet;
            planet.solid_of(planet.grid().locate(local).0, matter)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planet::PlanetRecipe;

    fn small_planet(radius: f64) -> Arc<Planet> {
        Arc::new(
            Planet::new(PlanetRecipe {
                radius_m: radius,
                voxel_size_m: 0.5,
                ..Default::default()
            })
            .unwrap(),
        )
    }

    #[test]
    fn transforms_round_trip() {
        let t = VolumeTransform::new(
            DVec3::new(1.0e7, -3.0e6, 42.5),
            DQuat::from_euler(glam::EulerRot::YXZ, 0.3, -1.1, 2.0),
        );
        let p = DVec3::new(6_371_000.0, 12.25, -0.125);
        assert!(t.to_world(t.to_local(p)).distance(p) < 1e-6);
        assert!(t.to_local(t.to_world(p)).distance(p) < 1e-6);
        let d = DVec3::new(0.3, 0.4, -0.5).normalize();
        assert!((t.dir_to_world(d).length() - 1.0).abs() < 1e-12);
        assert!(t.dir_to_local(t.dir_to_world(d)).distance(d) < 1e-12);
    }

    /// A ray in the world meets a moved, turned world where its own cast
    /// in that world's frame does, and the nearest of two volumes wins.
    #[test]
    fn world_rays_resolve_volumes_by_transform() {
        let planet = small_planet(2_000.0);
        let local_eye = DVec3::new(0.0, 2_400.0, 0.0);
        let local_dir = DVec3::new(0.05, -1.0, 0.02).normalize();
        let expected = planet.raycast(local_eye, local_dir, 1e4).unwrap();
        let t = VolumeTransform::new(
            DVec3::new(5.0e5, 1.0e4, -2.0e5),
            DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 0.5).normalize(), 0.7),
        );
        let near = VoxelVolume::new(7, planet.clone(), t);
        // A second world farther along the same ray.
        let far = VoxelVolume::new(
            9,
            small_planet(2_000.0),
            VolumeTransform::from_position(
                t.to_world(local_eye) + t.dir_to_world(local_dir) * 20_000.0,
            ),
        );
        let volumes = Volumes::new(vec![far, near]);
        let hit = volumes
            .raycast(
                t.to_world(local_eye),
                t.dir_to_world(local_dir),
                1e6,
                Matter::All,
            )
            .unwrap();
        assert_eq!(hit.id, 7);
        assert_eq!(hit.hit.cell, expected.cell);
        assert!((hit.hit.distance - expected.distance).abs() < 1e-6);
        assert!(hit.normal.distance(t.dir_to_world(expected.normal)) < 1e-9);
        // A point under the near world's surface is in it; one in the gap
        // between them is in neither.
        let under = t.to_world(planet.grid().cell_center(expected.cell));
        assert!(volumes.solid(under, Matter::All));
        assert!(!volumes.solid(
            t.to_world(local_eye) + t.dir_to_world(local_dir) * 9_000.0,
            Matter::All
        ));
    }
}
