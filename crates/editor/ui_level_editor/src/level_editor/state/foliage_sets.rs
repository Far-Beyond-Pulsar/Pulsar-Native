//! Foliage Sets — reusable authoring configuration for foliage placement.
//!
//! A [`FoliageSet`] is a named, individually enable-able collection of
//! [`FoliageMember`]s; each member is one source mesh with its *own*
//! [`MemberPlacement`] rules (density, scale, orientation). Painting scatters
//! instances for **every enabled member of every enabled set**, each by its
//! own rules — a "Meadow" set can mix dense small bushes and sparse large
//! trees without either knowing about the other.
//!
//! Pure editor-side authoring configuration: no scene database, renderer, or
//! GPUI types live here. Placement tools consume these definitions.

use serde::{Deserialize, Serialize};

/// Stable identity of a [`FoliageSet`] within a library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SetId(pub u64);

/// Stable identity of a [`FoliageMember`] within a library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemberId(pub u64);

/// Brush-wide density multiplier applied on top of every member's own
/// density (0 paints nothing, 1 is each member's authored density). A
/// newtype so `Default` is 1.0 — a bare `f32` field would default to 0 and
/// silently paint nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushDensity(pub f32);

impl Default for BrushDensity {
    fn default() -> Self {
        Self(1.0)
    }
}

impl BrushDensity {
    pub fn set(&mut self, value: f32) {
        self.0 = value.clamp(0.0, 1.0);
    }
}

/// Per-member placement rules.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberPlacement {
    /// Instances per 100 m² at full brush density.
    pub density: f32,
    /// Uniform scale range; each instance draws uniformly from it.
    pub scale_min: f32,
    pub scale_max: f32,
    /// Rotate each instance to a random yaw about the surface normal.
    pub random_yaw: bool,
    /// Orient each instance's up axis to the surface normal (otherwise world
    /// up).
    pub align_to_normal: bool,
    /// Vertical offset along the surface normal, in meters (negative sinks
    /// the mesh into the ground, hiding a bare trunk base).
    pub ground_offset_m: f32,
}

impl Default for MemberPlacement {
    fn default() -> Self {
        Self {
            density: 4.0,
            scale_min: 0.8,
            scale_max: 1.2,
            random_yaw: true,
            align_to_normal: false,
            ground_offset_m: 0.0,
        }
    }
}

impl MemberPlacement {
    pub const DENSITY_MAX: f32 = 500.0;
    pub const SCALE_MIN_LIMIT: f32 = 0.05;
    pub const SCALE_MAX_LIMIT: f32 = 20.0;
    pub const OFFSET_LIMIT: f32 = 10.0;

    pub fn set_density(&mut self, value: f32) {
        self.density = value.clamp(0.0, Self::DENSITY_MAX);
    }

    /// The two scale sliders clamp against each other so min can never cross
    /// max.
    pub fn set_scale_min(&mut self, value: f32) {
        self.scale_min = value
            .clamp(Self::SCALE_MIN_LIMIT, Self::SCALE_MAX_LIMIT)
            .min(self.scale_max);
    }

    pub fn set_scale_max(&mut self, value: f32) {
        self.scale_max = value
            .clamp(Self::SCALE_MIN_LIMIT, Self::SCALE_MAX_LIMIT)
            .max(self.scale_min);
    }

    pub fn set_ground_offset(&mut self, value: f32) {
        self.ground_offset_m = value.clamp(-Self::OFFSET_LIMIT, Self::OFFSET_LIMIT);
    }
}

/// One source mesh inside a set, with its own placement rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoliageMember {
    pub id: MemberId,
    /// Project-relative mesh asset path (the same string `StaticMeshComponent::
    /// mesh_asset` takes). Empty until the user picks one.
    pub mesh: String,
    pub enabled: bool,
    pub placement: MemberPlacement,
}

impl FoliageMember {
    /// Short name for lists: the mesh's file name, or a placeholder while no
    /// mesh has been chosen.
    pub fn display_name(&self) -> String {
        if self.mesh.is_empty() {
            return String::new();
        }
        self.mesh
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.mesh)
            .to_string()
    }

    /// A member paints only when its mesh is chosen *and* it is enabled.
    pub fn is_paintable(&self) -> bool {
        self.enabled && !self.mesh.is_empty()
    }
}

/// A named group of members that is enabled/disabled as a unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoliageSet {
    pub id: SetId,
    pub name: String,
    pub enabled: bool,
    /// UI expansion state, kept with the set so it survives panel rebuilds.
    pub expanded: bool,
    pub members: Vec<FoliageMember>,
}

/// What the panel's inspector is currently editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FoliageSelection {
    Set(SetId),
    Member(SetId, MemberId),
}

/// The whole library plus the current selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FoliageSetLibrary {
    pub sets: Vec<FoliageSet>,
    pub selection: Option<FoliageSelection>,
    next_id: u64,
}

impl FoliageSetLibrary {
    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Add an empty, enabled, expanded set and select it.
    pub fn add_set(&mut self) -> SetId {
        let id = SetId(self.alloc_id());
        let name = format!("Set {}", self.sets.len() + 1);
        self.sets.push(FoliageSet {
            id,
            name,
            enabled: true,
            expanded: true,
            members: Vec::new(),
        });
        self.selection = Some(FoliageSelection::Set(id));
        id
    }

    pub fn remove_set(&mut self, id: SetId) {
        self.sets.retain(|set| set.id != id);
        if matches!(self.selection, Some(FoliageSelection::Set(s) | FoliageSelection::Member(s, _)) if s == id)
        {
            self.selection = None;
        }
    }

    pub fn set(&self, id: SetId) -> Option<&FoliageSet> {
        self.sets.iter().find(|set| set.id == id)
    }

    pub fn set_mut(&mut self, id: SetId) -> Option<&mut FoliageSet> {
        self.sets.iter_mut().find(|set| set.id == id)
    }

    /// Add a member (with default placement) to `set` and select it. Returns
    /// `None` if the set does not exist.
    pub fn add_member(&mut self, set: SetId, mesh: String) -> Option<MemberId> {
        let id = MemberId(self.alloc_id());
        let target = self.set_mut(set)?;
        target.expanded = true;
        target.members.push(FoliageMember {
            id,
            mesh,
            enabled: true,
            placement: MemberPlacement::default(),
        });
        self.selection = Some(FoliageSelection::Member(set, id));
        Some(id)
    }

    pub fn remove_member(&mut self, set: SetId, member: MemberId) {
        if let Some(target) = self.set_mut(set) {
            target.members.retain(|m| m.id != member);
        }
        if self.selection == Some(FoliageSelection::Member(set, member)) {
            self.selection = Some(FoliageSelection::Set(set));
        }
    }

    pub fn member_mut(&mut self, set: SetId, member: MemberId) -> Option<&mut FoliageMember> {
        self.set_mut(set)?.members.iter_mut().find(|m| m.id == member)
    }

    /// Every member a brush stamp should scatter: enabled members (with a
    /// mesh) of enabled sets, paired with their set for grouping.
    pub fn paintable_members(&self) -> impl Iterator<Item = (&FoliageSet, &FoliageMember)> {
        self.sets
            .iter()
            .filter(|set| set.enabled)
            .flat_map(|set| set.members.iter().map(move |m| (set, m)))
            .filter(|(_, member)| member.is_paintable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with_mesh(mesh: &str) -> (FoliageSetLibrary, SetId, MemberId) {
        let mut lib = FoliageSetLibrary::default();
        let set = lib.add_set();
        let member = lib.add_member(set, mesh.to_string()).unwrap();
        (lib, set, member)
    }

    #[test]
    fn adding_a_set_selects_it_and_ids_never_repeat() {
        let mut lib = FoliageSetLibrary::default();
        let a = lib.add_set();
        let b = lib.add_set();
        assert_ne!(a, b);
        assert_eq!(lib.selection, Some(FoliageSelection::Set(b)));
    }

    #[test]
    fn ids_are_not_reused_after_removal() {
        let mut lib = FoliageSetLibrary::default();
        let a = lib.add_set();
        lib.remove_set(a);
        let b = lib.add_set();
        assert_ne!(a, b);
    }

    #[test]
    fn removing_the_selected_set_clears_the_selection() {
        let (mut lib, set, _) = library_with_mesh("m.mesh");
        lib.remove_set(set);
        assert_eq!(lib.selection, None);
    }

    #[test]
    fn removing_the_selected_member_falls_back_to_its_set() {
        let (mut lib, set, member) = library_with_mesh("m.mesh");
        lib.remove_member(set, member);
        assert_eq!(lib.selection, Some(FoliageSelection::Set(set)));
    }

    #[test]
    fn adding_a_member_to_a_missing_set_is_refused() {
        let mut lib = FoliageSetLibrary::default();
        assert!(lib.add_member(SetId(99), "m.mesh".into()).is_none());
    }

    #[test]
    fn only_enabled_members_of_enabled_sets_with_a_mesh_paint() {
        let (mut lib, set, member) = library_with_mesh("meshes/tree.mesh");
        assert_eq!(lib.paintable_members().count(), 1);

        lib.member_mut(set, member).unwrap().enabled = false;
        assert_eq!(lib.paintable_members().count(), 0);
        lib.member_mut(set, member).unwrap().enabled = true;

        lib.set_mut(set).unwrap().enabled = false;
        assert_eq!(lib.paintable_members().count(), 0);
        lib.set_mut(set).unwrap().enabled = true;

        lib.member_mut(set, member).unwrap().mesh.clear();
        assert_eq!(lib.paintable_members().count(), 0);
    }

    #[test]
    fn scale_bounds_cannot_cross() {
        let mut placement = MemberPlacement::default();
        placement.set_scale_min(100.0);
        assert!(placement.scale_min <= placement.scale_max);
        placement.set_scale_max(0.0);
        assert!(placement.scale_max >= placement.scale_min);
    }

    #[test]
    fn display_name_is_the_file_name() {
        let (mut lib, set, member) = library_with_mesh("meshes/trees\\oak.mesh");
        assert_eq!(lib.member_mut(set, member).unwrap().display_name(), "oak.mesh");
    }
}
