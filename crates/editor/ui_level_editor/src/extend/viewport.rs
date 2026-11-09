//! Viewport debug overlays a plugin draws over the scene (the portals'
//! link list, badges and ghost geometry).
//!
//! An overlay is shown from the viewport's options menu, or by choosing its
//! debug view mode (the renderer then draws the lit scene under it). While
//! shown, the viewport docks its panel at the top right and calls its
//! per-frame work; when hidden, it calls `hidden` so the plugin can clear
//! what it handed the renderer.

use std::sync::{Arc, Mutex};

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::subsystems::render::helio_renderer::HelioEditorMailbox;
use gpui::{AnyElement, App};

use super::SharedState;

/// What an overlay draws with.
pub struct OverlayContext<'a> {
    pub state: &'a SharedState,
    pub gpu_engine: &'a Arc<Mutex<GpuRenderer>>,
}

impl OverlayContext<'_> {
    /// The renderer's editor mailbox (view payloads), when it is free.
    pub fn mailbox(&self) -> Option<HelioEditorMailbox> {
        self.gpu_engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.editor_mailbox())
    }
}

/// A debug overlay of the viewport.
pub struct DebugOverlay {
    pub origin: &'static str,
    /// Stable id (the viewport keeps which are shown by id).
    pub id: &'static str,
    /// The viewport options menu's text.
    pub menu_label: &'static str,
    /// The debug view mode that shows it (`None`: menu only). Choosing it
    /// draws the lit scene with the overlay.
    pub debug_mode: Option<u32>,
    /// The floating panel (top right).
    pub panel: fn(&OverlayContext, &mut App) -> AnyElement,
    /// Per-frame work while shown in a perspective view (labels the
    /// renderer draws in the scene).
    pub perspective: fn(&OverlayContext),
    /// Clear what it gave the renderer (hidden, or not a perspective view).
    pub hidden: fn(&HelioEditorMailbox),
}

super::extension_point!(DebugOverlay: "editor.debug_overlay", |r| r.id, r.menu_label;);

/// Every registered debug overlay.
pub fn debug_overlays() -> impl Iterator<Item = &'static DebugOverlay> {
    pulsar_plugins::registered::<DebugOverlay>()
}

/// The overlays debug view `mode` shows.
pub fn overlays_of_mode(mode: u32) -> impl Iterator<Item = &'static DebugOverlay> {
    debug_overlays().filter(move |overlay| overlay.debug_mode == Some(mode))
}

/// The debug mode the renderer draws for the chosen `mode`: lit (0) when
/// an overlay draws that mode instead.
pub fn renderer_debug_mode(mode: u32) -> u32 {
    if overlays_of_mode(mode).next().is_some() {
        0
    } else {
        mode
    }
}
