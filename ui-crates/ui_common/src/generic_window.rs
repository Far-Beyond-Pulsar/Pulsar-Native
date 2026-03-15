//! Macros for defining window types with minimum boilerplate.

/// Creates a drawer-wrapping window type that implements BOTH [`gpui::Render`] AND
/// [`window_manager::PulsarWindow`].
///
/// Use this for the common pattern where a window just wraps a single inner drawer/panel
/// entity with a fixed size and title.
///
/// # Parameters
/// - `$window_type` — name for the new window struct (e.g. `ProblemsWindow`)
/// - `$inner_type` — the inner drawer type (e.g. `ProblemsDrawer`)
/// - `$inner_field` — field name for the drawer (e.g. `problems_drawer`)
/// - `$title` — i18n key for the title bar (e.g. `"Window.Title.Problems"`)
/// - `$width`, `$height` — default window size in logical pixels
///
/// # Generated items
/// - `pub struct $window_type { $inner_field: Entity<$inner_type> }`
/// - `impl $window_type { pub fn new(…) -> Self }`
/// - `impl Render for $window_type`
/// - `impl PulsarWindow for $window_type` (Params = Entity<$inner_type>)
///
/// # Example
/// ```ignore
/// pulsar_drawer_window!(ProblemsWindow, ProblemsDrawer, problems_drawer, "Window.Title.Problems", 900.0, 600.0);
/// // Add custom event emitters separately:
/// impl gpui::EventEmitter<NavigateToDiagnostic> for ProblemsWindow {}
/// ```
#[macro_export]
macro_rules! pulsar_drawer_window {
    ($window_type:ident, $inner_type:ty, $inner_field:ident, $title:expr, $width:expr, $height:expr) => {
        pub struct $window_type {
            $inner_field: gpui::Entity<$inner_type>,
        }

        impl $window_type {
            pub fn new(
                $inner_field: gpui::Entity<$inner_type>,
                _cx: &mut gpui::Context<Self>,
            ) -> Self {
                Self { $inner_field }
            }

            pub fn $inner_field(&self) -> &gpui::Entity<$inner_type> {
                &self.$inner_field
            }
        }

        impl gpui::Render for $window_type {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                ui::drawer_window_entity($title, self.$inner_field.clone(), cx)
            }
        }

        impl window_manager::PulsarWindow for $window_type {
            type Params = gpui::Entity<$inner_type>;

            fn window_name() -> &'static str {
                stringify!($window_type)
            }

            fn window_options(_params: &gpui::Entity<$inner_type>) -> gpui::WindowOptions {
                window_manager::default_window_options($width, $height)
            }

            fn build(
                params: gpui::Entity<$inner_type>,
                _window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) -> gpui::Entity<Self> {
                #[allow(unused_imports)]
                use gpui::AppContext as _;
                cx.new(|cx| Self::new(params, cx))
            }
        }
    };
}

/// Keep `drawer_window!` as an alias for backwards compat during migration.
/// Prefer `pulsar_drawer_window!` for new code.
#[macro_export]
macro_rules! drawer_window {
    ($window_type:ident, $inner_type:ty, $inner_field:ident, $title:expr) => {
        $crate::pulsar_drawer_window!($window_type, $inner_type, $inner_field, $title, 900.0, 600.0);
    };
}

