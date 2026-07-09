use gpui::{prelude::*, *};
use ui::input::InputState;

/// Trait capturing the per-doc-source configuration needed to build a search
/// input and populate the initial markdown view.
pub trait DocSource {
    fn placeholder() -> &'static str;
    fn initial_content() -> String;
}

/// Builds a search `InputState` pre-populated with the placeholder defined by
/// the `DocSource` implementor `T`.
pub fn make_search_input<T: DocSource>(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        let mut state = InputState::new(window, cx);
        state.set_placeholder(T::placeholder(), window, cx);
        state
    })
}
