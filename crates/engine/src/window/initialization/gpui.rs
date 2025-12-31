//! GPUI window initialization
//!
//! This module contains the complete logic for initializing GPUI applications,
//! loading fonts, setting up keybindings, and creating window content.

use gpui::*;
use winit::window::WindowId;
use raw_window_handle::HasWindowHandle;
use crate::assets::Assets;
use crate::OpenSettings;
use ui_core::ToggleCommandPalette;
use ui_common::menu::{AboutApp, ShowDocumentation};
use engine_state::{EngineState, WindowRequest};
use crate::window::{WinitGpuiApp, WindowState};
use crate::window::initialization::window_content;
use std::sync::Arc;

/// Initialize a GPUI window with full setup
///
/// This function performs the complete GPUI initialization sequence:
/// 1. Calculate window bounds and create external window handle
/// 2. Load fonts (JetBrains Mono)
/// 3. Initialize UI components (ui, themes, terminal)
/// 4. Setup keybindings (Ctrl-, Ctrl-Space, etc.)
/// 5. Register global actions (Settings, About, Documentation)
/// 6. Create window content based on window type
/// 7. Store window metadata in EngineState
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window to initialize
/// * `engine_state` - Shared engine state
pub fn initialize_gpui_window(
    app: &mut WinitGpuiApp,
    window_id: &WindowId,
    engine_state: &EngineState,
) {
    let window_state = app.windows.get_mut(window_id).expect("Window state must exist");

    let winit_window = window_state.winit_window.clone();
    let scale_factor = winit_window.scale_factor() as f32;
    let size = winit_window.inner_size();

    // GPUI bounds must be in LOGICAL pixels (physical / scale)
    let logical_width = size.width as f32 / scale_factor;
    let logical_height = size.height as f32 / scale_factor;

    let bounds = Bounds {
        origin: point(px(0.0), px(0.0)),
        size: gpui::size(px(logical_width), px(logical_height)),
    };

    tracing::debug!("🖥️ Creating GPUI window: physical {}x{}, scale {}, logical {}x{}",
        size.width, size.height, scale_factor, logical_width, logical_height);

    let gpui_raw_handle = winit_window
        .window_handle()
        .expect("Failed to get window handle")
        .as_raw();

    let external_handle = ExternalWindowHandle {
        raw_handle: gpui_raw_handle,
        bounds,
        scale_factor,
        surface_handle: None,
    };

    tracing::debug!("✨ Opening GPUI window on external winit window...");

    // Initialize GPUI components (fonts, themes, keybindings)
    let gpui_app = &mut window_state.gpui_app;

    // Clone engine_state for use in closures
    let engine_state_for_actions = engine_state.clone();

    // Load custom fonts
    gpui_app.update(|gpui_app| {
        if let Some(font_data) = Assets::get("fonts/JetBrainsMono-Regular.ttf") {
            match gpui_app.text_system().add_fonts(vec![font_data.data]) {
                Ok(_) => tracing::debug!("Successfully loaded JetBrains Mono font"),
                Err(e) => tracing::debug!("Failed to load JetBrains Mono font: {:?}", e),
            }
        } else {
            tracing::debug!("Could not find JetBrains Mono font file");
        }

        // Initialize GPUI components
        ui::init(gpui_app);
        crate::themes::init(gpui_app);
        ui_terminal::init(gpui_app);

        // Setup keybindings
        gpui_app.bind_keys([
            KeyBinding::new("ctrl-,", OpenSettings, None),
            KeyBinding::new("ctrl-space", ToggleCommandPalette, None),
            // Blueprint editor keybindings handled by plugin
        ]);

        let engine_state = engine_state_for_actions.clone();
        gpui_app.on_action(move |_: &OpenSettings, _app_cx| {
            tracing::debug!("⚙️ Settings window requested - creating new window!");
            engine_state.request_window(WindowRequest::Settings);
        });

        let engine_state = engine_state_for_actions.clone();
        gpui_app.on_action(move |_: &AboutApp, _app_cx| {
            tracing::debug!("ℹ️ About window requested - creating new window!");
            engine_state.request_window(WindowRequest::About);
        });

        let engine_state = engine_state_for_actions.clone();
        gpui_app.on_action(move |_: &ShowDocumentation, _app_cx| {
            tracing::debug!("📖 Documentation window requested - creating new window!");
            engine_state.request_window(WindowRequest::Documentation);
        });

        gpui_app.activate(true);
    });

    tracing::debug!("✨ GPUI components initialized");

    // Store window_id in EngineState metadata BEFORE opening GPUI window
    let window_id_u64 = unsafe { std::mem::transmute::<_, u64>(*window_id) };
    tracing::debug!("[WINDOW-INIT] 🔖 Window ID for this window: {}", window_id_u64);
    engine_state.set_metadata("latest_window_id".to_string(), window_id_u64.to_string());

    // If this is a project editor window, also store it with a special key
    if matches!(&window_state.window_type, Some(WindowRequest::ProjectEditor { .. })) {
        engine_state.set_metadata("current_project_window_id".to_string(), window_id_u64.to_string());
        tracing::debug!("[WINDOW-INIT] 🎨 This is a ProjectEditor window with ID: {}", window_id_u64);
    }

    // Capture window_id_u64 and engine_state for use in the closure
    let captured_window_id = window_id_u64;
    let engine_state_for_events = engine_state.clone();
    tracing::debug!("[WINDOW-INIT] 📦 Captured window_id for closure: {}", captured_window_id);

    // Get window type before borrowing gpui_app mutably
    let window_type = window_state.window_type.clone();

    // Open GPUI window using external window API with appropriate view
    let gpui_window = gpui_app.open_window_external(external_handle.clone(), |window, cx| {
        window_content::create_window_content(&window_type, captured_window_id, &engine_state_for_events, window, cx)
    }).expect("Failed to open GPUI window");

    // Store the GPUI window handle
    let window_state = app.windows.get_mut(window_id).expect("Window state must exist");
    window_state.gpui_window = Some(gpui_window);

    tracing::debug!("✨ GPUI window opened successfully!\n");
}
