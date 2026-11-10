//! Sublevel placements: indexed content placed inside its owning level.
//!
//! A sublevel is an indexed content space, not a SceneDB entity. Index zero
//! ([`DEFAULT_SUBLEVEL_INDEX`]) is the default level and always denotes
//! world space. An object carrying a [`SubLevelActorRow`] places a sublevel:
//! it defines a space (`crate::frames`), so the content placed there moves
//! with one matrix. This is the direct-instancing primitive for reusable
//! content (an aircraft, a room). It is separate from portals: a portal shows
//! a peer portal's context through an aperture and is no placement, but the
//! portals plugin's resolver reads placements to know where each sublevel's
//! portals are.
//!
//! The scene model owns it because a placement is scene structure, read by
//! the engine's space definer (`engine_backend::scene::sublevels`) and by any
//! plugin that resolves content spaces; no pass reads it on the GPU, so it is
//! no buffer contract.

/// Stable content-space index for an authored sublevel. A plain scalar:
/// resolving an index to content and projecting it into a GPU
/// coordinate-space slot is the reader's.
pub type SubLevelIndex = u32;

/// The default level's authored sublevel index.
pub const DEFAULT_SUBLEVEL_INDEX: SubLevelIndex = 0;

/// Placement of an indexed sublevel inside its owning level: a plain SceneDB
/// component on the placing object.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct SubLevelActorRow {
    pub sublevel_index: SubLevelIndex,
    pub flags: u32,
    pub transform: [[f32; 4]; 4],
}

impl SubLevelActorRow {
    /// Bit in [`Self::flags`] that enables this actor instance.
    pub const FLAG_ENABLED: u32 = 1 << 0;

    pub fn new(sublevel_index: SubLevelIndex, transform: glam::Mat4) -> Self {
        Self {
            sublevel_index,
            flags: Self::FLAG_ENABLED,
            transform: transform.to_cols_array_2d(),
        }
    }

    pub fn transform(&self) -> glam::Mat4 {
        glam::Mat4::from_cols_array_2d(&self.transform)
    }

    pub fn is_enabled(&self) -> bool {
        self.flags & Self::FLAG_ENABLED != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_stable() {
        assert_eq!(std::mem::size_of::<SubLevelActorRow>(), 72);
        assert_eq!(std::mem::align_of::<SubLevelActorRow>(), 4);
    }

    #[test]
    fn a_placement_round_trips_its_index_and_transform() {
        let actor =
            SubLevelActorRow::new(7, glam::Mat4::from_translation(glam::vec3(3.0, 4.0, 5.0)));
        assert_eq!(actor.sublevel_index, 7);
        assert_eq!(actor.flags, SubLevelActorRow::FLAG_ENABLED);
        assert_eq!(actor.transform().w_axis, glam::vec4(3.0, 4.0, 5.0, 1.0));
    }
}
