use gpui::{prelude::*, *};
use ui::input::InputState;

pub trait DocSource {
    fn placeholder() -> &'static str;
    fn initial_content() -> String;
}

pub fn make_search_input<T: DocSource>(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        let mut state = InputState::new(window, cx);
        state.set_placeholder(T::placeholder(), window, cx);
        state
    })
}
