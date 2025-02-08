use gpui::{div, hsla, rgb, IntoElement, ParentElement, Render, Styled, ViewContext, VisualContext, WindowContext};


pub struct App{}

impl App {
    pub fn new(mut cx: &mut WindowContext) -> gpui::View<Self> {
        cx.new_view(|_| App {})
    }
}

impl Render for App {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        div().text_color(rgb(0xFFFFFF)).child("testing")
    }
}
