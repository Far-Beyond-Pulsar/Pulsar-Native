//! Dock panels over the editor entity: the canvas and the side panels.
//!
//! Each is a thin wrapper that renders a part of the editor (the editor holds the
//! state), so every panel sees the same document.

use gpui::*;
use ui::{
    dock::{Panel, PanelEvent},
    ActiveTheme,
};

use crate::editor::panel::ShaderEditorPanel;
use crate::features::preview::MaterialPreviewPanel;
use crate::ui_components::properties::PropertiesRenderer;

macro_rules! dock_panel_boilerplate {
    ($panel:ident, $name:literal, $title:literal) => {
        impl EventEmitter<PanelEvent> for $panel {}

        impl Focusable for $panel {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl Panel for $panel {
            fn panel_name(&self) -> &'static str {
                $name
            }

            fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
                $title.into_any_element()
            }
        }
    };
}

/// Observes the editor so the panel repaints when the document, the selection
/// or the compile state changes.
fn follow<T: 'static + Render>(editor: &WeakEntity<ShaderEditorPanel>, cx: &mut Context<T>) {
    if let Some(entity) = editor.upgrade() {
        cx.observe(&entity, |_, _, cx| cx.notify()).detach();
    }
}

/// Compiler Panel
pub struct CompilerPanel {
    editor: WeakEntity<ShaderEditorPanel>,
    focus_handle: FocusHandle,
}

impl CompilerPanel {
    pub fn new(editor: WeakEntity<ShaderEditorPanel>, cx: &mut Context<Self>) -> Self {
        follow(&editor, cx);
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for CompilerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(editor) = self.editor.upgrade() {
            div()
                .size_full()
                .child(editor.update(cx, |editor, cx| editor.render_compiler_results(cx)))
        } else {
            div().child("Editor not available")
        }
    }
}

dock_panel_boilerplate!(CompilerPanel, "compiler", "Compiler");

/// Find Panel: the index of the graph's nodes.
pub struct FindPanel {
    editor: WeakEntity<ShaderEditorPanel>,
    focus_handle: FocusHandle,
}

impl FindPanel {
    pub fn new(editor: WeakEntity<ShaderEditorPanel>, cx: &mut Context<Self>) -> Self {
        follow(&editor, cx);
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for FindPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(editor) = self.editor.upgrade() {
            div()
                .size_full()
                .child(editor.update(cx, |editor, cx| editor.render_find_panel(cx)))
        } else {
            div().child("Editor not available")
        }
    }
}

dock_panel_boilerplate!(FindPanel, "find", "Find");

/// Properties Panel
pub struct PropertiesPanel {
    editor: WeakEntity<ShaderEditorPanel>,
    focus_handle: FocusHandle,
}

impl PropertiesPanel {
    pub fn new(editor: WeakEntity<ShaderEditorPanel>, cx: &mut Context<Self>) -> Self {
        follow(&editor, cx);
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for PropertiesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(editor) = self.editor.upgrade() {
            div()
                .size_full()
                .bg(cx.theme().sidebar)
                .child(editor.update(cx, |editor, cx| PropertiesRenderer::render(editor, window, cx)))
        } else {
            div().child("Editor not available")
        }
    }
}

dock_panel_boilerplate!(PropertiesPanel, "properties", "Shader Details");

/// Material Preview Panel: thin dock wrapper around MaterialPreviewPanel.
pub struct PreviewPanel {
    editor: WeakEntity<ShaderEditorPanel>,
    focus_handle: FocusHandle,
    preview: Entity<MaterialPreviewPanel>,
}

impl PreviewPanel {
    pub fn new(editor: WeakEntity<ShaderEditorPanel>, cx: &mut Context<Self>) -> Self {
        follow(&editor, cx);
        let preview = cx.new(|cx| MaterialPreviewPanel::new(editor.clone(), cx));
        Self {
            editor,
            focus_handle: cx.focus_handle(),
            preview,
        }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(editor) = self.editor.upgrade() {
            let wgsl = editor.read(cx).last_compiled_wgsl.clone();
            if let Some(wgsl) = wgsl {
                self.preview.update(cx, |preview, cx| {
                    preview.update_shader(&wgsl, cx);
                });
            }
        }
        div().size_full().child(self.preview.clone())
    }
}

dock_panel_boilerplate!(PreviewPanel, "material-preview", "Material Preview");

/// The dock panel holding the canvas.
pub struct CanvasPanel {
    editor: WeakEntity<ShaderEditorPanel>,
    focus_handle: FocusHandle,
}

impl CanvasPanel {
    pub fn new(editor: WeakEntity<ShaderEditorPanel>, cx: &mut Context<Self>) -> Self {
        follow(&editor, cx);
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for CanvasPanel {}

impl Focusable for CanvasPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CanvasPanel {
    fn panel_name(&self) -> &'static str {
        "graph-canvas"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn title(&self, _window: &Window, cx: &App) -> AnyElement {
        let title = self
            .editor
            .upgrade()
            .map(|editor| {
                let editor = editor.read(cx);
                let name = editor.get_display_name();
                if editor.is_dirty() {
                    format!("{name} *")
                } else {
                    name
                }
            })
            .unwrap_or_else(|| "Shader Graph".to_string());
        title.into_any_element()
    }
}

impl Render for CanvasPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.editor.upgrade() {
            Some(editor) => editor.update(cx, |editor, cx| editor.render_center(window, cx)),
            None => div().into_any_element(),
        }
    }
}
