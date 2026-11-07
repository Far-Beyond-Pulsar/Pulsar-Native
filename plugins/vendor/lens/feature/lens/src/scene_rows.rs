//! The lens's scene rows: the camera lens components' rows, which are the
//! lens pass's rows bit for bit (a row join, see `helio_scene_join::RowJoin`;
//! the pass reads its elements from the host-registered `"lens_elements"`).
use helio_scene_join::{BufferKey, RowJoin, RowJoinKeys, RowTable};

/// `CameraLensSourceRow`: the [`crate::LensRow`] pass row.
pub const CAMERA_LENS_SOURCE_ROW_BYTES: u64 = std::mem::size_of::<crate::LensRow>() as u64;

pub const CAMERA_LENSES_KEY: BufferKey = BufferKey::of("camera_lenses");

/// The lens's row join, from the camera lens rows at `camera_lens`.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, camera_lens: BufferKey) -> RowJoin {
    RowJoin::new(
        device,
        "LensRows",
        keys,
        vec![RowTable::new(
            "Lens Rows",
            CAMERA_LENSES_KEY,
            camera_lens,
            CAMERA_LENS_SOURCE_ROW_BYTES,
            (CAMERA_LENS_SOURCE_ROW_BYTES / 4) as u32,
            0,
        )],
    )
}
