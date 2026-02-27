//! Component-based UI architecture
//!
//! Provides a trait-based system for composable UI components similar to React

use gpui::*;
use ui_types_common::window_types::{WindowRequest, WindowId};
use window_manager;

/// Configuration for spawning a component in a window
#[derive(Clone, Debug)]
pub struct ComponentWindowConfig {
    pub title: String,
    pub bounds: Option<Bounds<Pixels>>,
    pub kind: WindowKind,
    pub is_movable: bool,
    pub display_id: Option<DisplayId>,
}

impl Default for ComponentWindowConfig {
    fn default() -> Self {
        Self {
            title: "Pulsar".to_string(),
            bounds: None,
            kind: WindowKind::Normal,
            is_movable: true,
            display_id: None,
        }
    }
}

/// A composable UI component that can be rendered and spawned in windows
pub trait Component: Render + Sized + 'static {
    /// Configuration type for this component
    type Config: Clone + Send + 'static;

    /// Create a new instance with the given configuration
    fn new(config: Self::Config, window: &mut Window, cx: &mut Context<Self>) -> Self;

    /// Spawn this component in a new window
    fn spawn_window(
        config: Self::Config,
        window_config: ComponentWindowConfig,
        cx: &mut App,
    ) -> Result<()> {
        let options = WindowOptions {
            window_bounds: window_config.bounds.map(WindowBounds::Windowed),
            titlebar: None,
            window_background: WindowBackgroundAppearance::Opaque,
            focus: true,
            show: true,
            kind: window_config.kind.clone(),
            is_movable: window_config.kind != WindowKind::PopUp,
            is_minimizable: true,
            is_resizable: true,
            display_id: window_config.display_id,
            app_id: None,
            window_min_size: None,
            window_decorations: None,
            tabbing_identifier: None,
        };

        // Replace direct cx.open_window with window_manager::WindowManager::global().create_window
        window_manager::WindowManager::update_global(cx, |wm: &mut window_manager::WindowManager, cx| {
            wm.create_window(
                WindowRequest::Component,
                options,
                move |window: &mut gpui::Window, cx: &mut gpui::App| {
                    cx.new(|cx| Self::new(config.clone(), window, cx)).into()
                },
                cx,
            )
        })
        .map(|_| ())?;

        Ok(())
    }
}

/// Marker trait for components that should be rendered as the root of a window
pub trait RootComponent: Component {}

/// Marker trait for components that can be embedded within other components
pub trait EmbeddableComponent: Component {}