//! Per-viewport view settings: the camera mode a viewport looks through, its
//! view-mode (lit or debug) picker, and how both reach its renderer.
//!
//! The primary viewport reads and writes the editor's shared state, so the
//! toolbar, menus and AI tools keep driving it. A split-off viewport owns a
//! [`new_view_state`] instead (the overlay components are written against
//! `LevelEditorState`), which makes its overlays, camera mode and debug view
//! independent of every other viewport. Tools, snaps and the scene itself stay
//! shared.

use std::sync::Arc;

use engine_backend::subsystems::render::{ViewKind, ViewportDebugView, PORTAL_DEBUG_VIEW_MODE};
use gpui::*;
use ui::dropdown::{DropdownEvent, SearchableVec};

use super::ViewportPanel;
use crate::state::{CameraMode, LevelEditorState};
use crate::ui::toolbar::view_mode_dropdown::{
    ViewModeDropdown, ViewModeOption, ViewModePickerState, ViewModeSelection,
};

/// The renderer view a camera mode asks for.
pub(crate) fn view_kind_of(mode: CameraMode) -> ViewKind {
    match mode {
        CameraMode::Perspective => ViewKind::Perspective,
        CameraMode::Orthographic => ViewKind::Orthographic,
        CameraMode::Top => ViewKind::Top,
        CameraMode::Front => ViewKind::Front,
        CameraMode::Side => ViewKind::Side,
    }
}

/// A fresh view state for a split-off viewport, looking through `mode`.
pub(crate) fn new_view_state(mode: CameraMode) -> Arc<parking_lot::RwLock<LevelEditorState>> {
    let mut state = LevelEditorState::new();
    state.editor.set_camera_mode(mode);
    // The floating bars start collapsed to their buttons: four copies of the
    // full bars would cover most of a quarter-size viewport.
    state.overlays.state.camera_mode_selector_collapsed = true;
    state.overlays.state.viewport_options_collapsed = true;
    Arc::new(parking_lot::RwLock::new(state))
}

/// The view-mode picker of a split-off viewport (the primary one uses the
/// toolbar's), with the debug views it was built for.
pub(crate) struct OwnViewPicker {
    pub(crate) picker: Entity<ViewModePickerState>,
    debug_views: Vec<ViewportDebugView>,
    _subscription: Subscription,
}

impl OwnViewPicker {
    fn new<V: 'static>(
        window: &mut Window,
        cx: &mut Context<V>,
        view_state: &Arc<parking_lot::RwLock<LevelEditorState>>,
        debug_views: Vec<ViewportDebugView>,
    ) -> Self {
        let selected = ViewModeDropdown::selected(view_state.read().editor.viewport_debug_mode);
        let picker = ViewModeDropdown::new_state(window, cx, &debug_views, selected);
        let state = view_state.clone();
        let subscription = cx.subscribe(
            &picker,
            move |_, _, event: &DropdownEvent<SearchableVec<ViewModeOption>>, _cx| {
                let DropdownEvent::Confirm(Some(selected_mode)) = event else {
                    return;
                };
                let mut state = state.write();
                state.editor.viewport_debug_mode = match selected_mode {
                    ViewModeSelection::Lit => 0,
                    ViewModeSelection::Debug(mode) => *mode,
                };
                state.overlays.state.show_portal_debug_overlay =
                    state.editor.viewport_debug_mode == PORTAL_DEBUG_VIEW_MODE;
                state.editor.show_wireframe = false;
                state.editor.show_lighting = true;
            },
        );
        Self {
            picker,
            debug_views,
            _subscription: subscription,
        }
    }

    pub(crate) fn label(&self, cx: &App) -> SharedString {
        match self.picker.read(cx).selected_value() {
            Some(ViewModeSelection::Debug(mode)) => self
                .debug_views
                .iter()
                .find(|view| view.debug_mode == *mode)
                .map(|view| view.name.into())
                .unwrap_or_else(|| "Lit".into()),
            _ => "Lit".into(),
        }
    }
}

impl ViewportPanel {
    /// Whether this viewport has settings of its own (a split-off viewport).
    pub(crate) fn has_own_view_state(&self) -> bool {
        self.own_view
    }

    /// The camera mode of a split-off viewport (the primary one's lives in the
    /// shared state).
    pub(crate) fn own_camera_mode(&self) -> Option<CameraMode> {
        self.own_view
            .then(|| self.view_state.read().editor.camera_mode)
    }

    /// Push this viewport's settings to its renderer. Cheap (a few atomics):
    /// called from the panel's frame pump.
    ///
    /// The primary viewport's Bloom, Realtime and debug view are pushed by the
    /// toolbar; a split-off one follows the shared Bloom and Realtime toggles
    /// and has its own debug view.
    pub(crate) fn sync_view_settings<V: 'static>(
        &mut self,
        shared: &Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        if self.mailbox.is_none() {
            self.mailbox = self
                .gpu_engine
                .try_lock()
                .ok()
                .and_then(|engine| engine.editor_mailbox());
        }
        let Some(mailbox) = self.mailbox.clone() else {
            return;
        };

        let (camera_mode, debug_mode) = {
            let view = self.view_state.read();
            (view.editor.camera_mode, view.editor.viewport_debug_mode)
        };
        mailbox.set_view_kind(view_kind_of(camera_mode));

        if !self.has_own_view_state() {
            return;
        }
        {
            let shared = shared.read();
            mailbox.set_viewport_bloom(shared.editor.feature_bloom_enabled);
            mailbox.set_viewport_realtime(shared.editor.realtime_viewport);
        }
        mailbox.set_viewport_debug_mode(if debug_mode == PORTAL_DEBUG_VIEW_MODE {
            0
        } else {
            debug_mode
        });
        mailbox.set_camera_sync_locked(self.view_state.read().overlays.state.camera_sync_locked);

        // The renderer advertises its debug views once its graph exists.
        let debug_views = mailbox.viewport_debug_views();
        let stale = self
            .own_picker
            .as_ref()
            .is_none_or(|own| own.debug_views != debug_views);
        if stale {
            self.own_picker = Some(OwnViewPicker::new(
                window,
                cx,
                &self.view_state.clone(),
                debug_views,
            ));
        } else if let Some(own) = &self.own_picker {
            let selected = ViewModeDropdown::selected(debug_mode);
            if own.picker.read(cx).selected_value().copied() != Some(selected) {
                own.picker.update(cx, |picker, cx| {
                    picker.set_selected_value(&selected, window, cx);
                });
            }
        }
    }
}
