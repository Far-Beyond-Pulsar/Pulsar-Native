//! The registrations a kind submits so the editor can pick and frame its
//! content: geometry ([`ContentGeometryRegistration`]) and editor icons
//! ([`ViewIconRegistration`]).
use glam::{Vec2, Vec3};
use pulsar_scenedb::{Entity, World};

/// A kind's content geometry, as the editor's picking, framing and debug
/// views read it (the meshes' triangles): its box in an object's local
/// space, a ray against it, and its triangles. The kind's component crate
/// submits one; the readers name no kind. How others do it: Unreal's
/// primitive components answer `LineTraceComponent` and give `Bounds`;
/// Bevy's picking backends (the mesh backend among them) each test their
/// own content. Ours is a backend per kind, by registration.
pub struct ContentGeometryRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"meshes.geometry"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    /// The box around `object`'s enabled content of this kind, in the
    /// object's local space; `None` without any (or none loaded yet).
    pub local_bounds: fn(world: &World, object: Entity) -> Option<(Vec3, Vec3)>,
    /// A ray in `object`'s local space (`origin`, `direction`; the
    /// direction's length makes the hit parameter a world distance) against
    /// its content of this kind: `None` when it has no content to test
    /// (picking falls back to the box), `Some(None)` for a miss, `Some(t)`
    /// for the nearest hit.
    pub hit:
        fn(world: &World, object: Entity, origin: Vec3, direction: Vec3) -> Option<Option<f32>>,
    /// Every enabled triangle of this kind in `world`, in its object's local
    /// space, until `visit` returns `false`: `visit(object, triangle)`.
    pub triangles: fn(world: &World, visit: &mut dyn FnMut(Entity, [Vec3; 3]) -> bool),
}

/// Every content triangle in `world` (every [`ContentGeometryRegistration`]),
/// in its object's local space, until `visit` returns `false`.
pub fn content_triangles(world: &World, visit: &mut dyn FnMut(Entity, [Vec3; 3]) -> bool) {
    let mut going = true;
    for registration in pulsar_plugins::registered::<ContentGeometryRegistration>() {
        (registration.triangles)(world, &mut |object, triangle| {
            going = going && visit(object, triangle);
            going
        });
        if !going {
            return;
        }
    }
}

impl pulsar_plugins::Contribution for ContentGeometryRegistration {
    const CATEGORY: &'static str = "render.content_geometry";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(ContentGeometryRegistration);

/// A kind's editor icon (the lights' billboards): a camera-facing quad
/// drawn at a constant size on screen, which picking tests instead of a box.
/// The kind's plugin root submits one; picking names no kind.
pub struct ViewIconRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"lights.icons"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    /// `object`'s icon, when it has one of this kind: its width and height
    /// in world units per unit of view depth.
    pub icon: fn(world: &World, object: Entity) -> Option<[f32; 2]>,
}

impl pulsar_plugins::Contribution for ViewIconRegistration {
    const CATEGORY: &'static str = "render.view_icon";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(ViewIconRegistration);

/// `object`'s editor icon size (the first [`ViewIconRegistration`] that has
/// one).
pub(super) fn view_icon(world: &World, object: Entity) -> Option<Vec2> {
    pulsar_plugins::registered::<ViewIconRegistration>()
        .find_map(|registration| (registration.icon)(world, object))
        .map(Vec2::from_array)
}
