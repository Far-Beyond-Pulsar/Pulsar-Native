//! The setup illustrations shown beside each calibration method: rendered scenes of a room with
//! a mannequin, the cameras and their fields of view, and the distances that matter. They are
//! PNGs embedded in the plugin (`assets/*.png`), made by `tools/render_setups.py` (Blender), and
//! decoded once on first use.

use gpui::RenderImage;
use std::sync::{Arc, OnceLock};

/// An illustration of a camera setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diagram {
    Ring,
    Arc,
    Line,
    Body,
    /// The space to keep clear around the subject.
    Space,
}

impl Diagram {
    const ALL: [Diagram; 5] = [Diagram::Ring, Diagram::Arc, Diagram::Line, Diagram::Body, Diagram::Space];

    fn png(self) -> &'static [u8] {
        match self {
            Diagram::Ring => include_bytes!("../assets/ring.png"),
            Diagram::Arc => include_bytes!("../assets/arc.png"),
            Diagram::Line => include_bytes!("../assets/line.png"),
            Diagram::Body => include_bytes!("../assets/body.png"),
            Diagram::Space => include_bytes!("../assets/space.png"),
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|d| *d == self).unwrap_or(0)
    }

    /// The picture, decoded on first use (`None` if it could not be decoded).
    pub fn image(self) -> Option<Arc<RenderImage>> {
        static CACHE: OnceLock<Vec<Option<Arc<RenderImage>>>> = OnceLock::new();
        CACHE.get_or_init(|| Self::ALL.iter().map(|d| decode(d.png())).collect())[self.index()].clone()
    }
}

/// The PNG as a BGRA image the UI can draw.
fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_illustration_decodes() {
        for diagram in Diagram::ALL {
            let decoded = image::load_from_memory(diagram.png()).expect("a PNG");
            assert_eq!((decoded.width(), decoded.height()), (1280, 800), "{diagram:?}");
            assert!(decode(diagram.png()).is_some(), "{diagram:?}");
        }
    }
}
