//! The canvas area: the canvas and the comment title editor over it. The canvas draws
//! and handles input, and owns the context menu and node picker; the title editor is
//! editor chrome Graphy leaves to the host.

use gpui::*;
use ui::input::TextInput;
use super::panel::ShaderEditorPanel;

impl ShaderEditorPanel {
    /// The canvas of the document and the overlays over it.
    pub(crate) fn render_center(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = crate::theme::graph_theme(cx);
        if self.canvas.read(cx).theme() != &theme {
            self.canvas.update(cx, |canvas, cx| canvas.set_theme(theme, cx));
        }
        let canvas = self.canvas.clone();
        div()
            .relative()
            .size_full()
            .on_action(cx.listener(|editor, _: &plugin_editor_api::Undo, _, cx| editor.undo(cx)))
            .on_action(cx.listener(|editor, _: &plugin_editor_api::Redo, _, cx| editor.redo(cx)))
            .child(div().size_full().child(canvas))
            .child(self.render_comment_editor(cx))
            .into_any_element()
    }

    /// Title editor over the comment being renamed.
    fn render_comment_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(index) = self.editing_comment else {
            return div().into_any_element();
        };
        let canvas = self.canvas.read(cx);
        let Some(comment) = canvas.graph().comments.get(index) else {
            return div().into_any_element();
        };
        let (_, zoom) = canvas.view();
        let title_h = (30.0 * zoom).clamp(18.0, 44.0);
        let title_w = (comment.size.0 as f32 * zoom - 24.0 * zoom).max(80.0);
        let origin = canvas.graph_to_window(graphy_render::Vec2::new(
            comment.position.x as f32,
            comment.position.y as f32,
        ));
        let position = Point::new(origin.x + px(12.0 * zoom), origin.y + px(6.0 * zoom));
        deferred(
            anchored()
                .position(position)
                .snap_to_window_with_margin(px(4.0))
                .anchor(Corner::TopLeft)
                .child(
                    div().occlude().w(px(title_w)).h(px(title_h * 0.68)).child(
                        TextInput::new(&self.comment_input)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false),
                    ),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

