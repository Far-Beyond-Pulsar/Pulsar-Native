#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::{WindowRequest};
    use gpui::{App, WindowOptions, Render, IntoElement};

    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    #[test]
    fn test_create_window() {
        let app = App::new();
        let wm = WindowManager::new();
        let options = WindowOptions::default();
        let result = wm.create_window(
            WindowRequest::Entry,
            options,
            |_, _| TestView,
            &mut app,
        );
        assert!(result.is_ok());
    }
}
