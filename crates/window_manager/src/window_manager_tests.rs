#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::{WindowRequest};
    use gpui::{App, WindowOptions};

    #[test]
    fn test_create_window() {
        let app = App::new();
        let wm = WindowManager::new(engine_state::EngineContext::global().unwrap());
        let options = WindowOptions::default();
        let result = wm.create_window(
            WindowRequest::Entry,
            options,
            |_, _, _| gpui::div().into_any_element(),
            &mut app,
        );
        assert!(result.is_ok());
    }
}
