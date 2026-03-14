/// Creates a window wrapper type that delegates rendering to an inner drawer/panel entity
/// via [`ui::drawer_window_entity`].
///
/// This eliminates boilerplate for the common pattern:
/// ```ignore
/// pub struct FooWindow { foo: Entity<FooDrawer> }
/// impl FooWindow { pub fn new(foo: Entity<FooDrawer>, _cx: &mut Context<Self>) -> Self { … } }
/// impl Render for FooWindow { fn render(…) { drawer_window_entity("Key", self.foo.clone(), cx) } }
/// ```
///
/// # Usage
///
/// ```ignore
/// // Generates: struct, new(), field accessor, and Render impl.
/// // EventEmitter (if needed) must be added separately.
/// drawer_window!(ProblemsWindow, ProblemsDrawer, problems_drawer, "Window.Title.Problems");
/// impl gpui::EventEmitter<NavigateToDiagnostic> for ProblemsWindow {}
/// ```
///
/// Parameters: `(TypeName, InnerType, field_name, "i18n-title-key")`
#[macro_export]
macro_rules! drawer_window {
    ($window_type:ident, $inner_type:ty, $inner_field:ident, $title:expr) => {
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
    };
}
