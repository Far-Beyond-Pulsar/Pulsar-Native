use gpui::App;
use linkme::distributed_slice;

/// Trait for types that can initialize UI components into the GPUI app context.
pub trait UiComponentInit: Send + Sync {
    fn init(&self, cx: &mut App);
}

/// Distributed slice for zero-overhead compile-time component registration.
/// Components register themselves by adding to this slice via the `register_ui_component!` macro.
#[distributed_slice]
pub static UI_COMPONENT_INITIALIZERS: [&'static dyn UiComponentInit] = [..];

/// Register a component initializer into the global UI component registry.
///
/// The initializer type must implement [`UiComponentInit`]. This macro is safe to use from
/// any crate that depends on `ui`, since it uses `$crate` to reference the correct slice.
///
/// Usage:
/// ```rust,ignore
/// struct MyComponentInit;
/// impl ui::registry::UiComponentInit for MyComponentInit {
///     fn init(&self, cx: &mut gpui::App) { /* bind keys, register globals, etc. */ }
/// }
/// ui::register_ui_component!(MyComponentInit);
/// ```
#[macro_export]
macro_rules! register_ui_component {
    ($initializer:expr) => {
        #[linkme::distributed_slice($crate::registry::UI_COMPONENT_INITIALIZERS)]
        static _INIT: &'static dyn $crate::registry::UiComponentInit = &$initializer;
    };
}

/// Initialize all components registered via `register_ui_component!`.
pub fn init_all_components(cx: &mut App) {
    for initializer in UI_COMPONENT_INITIALIZERS {
        initializer.init(cx);
    }
}
