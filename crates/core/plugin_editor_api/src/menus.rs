//! Editor menu strips (Pulsar-Native#1012).
//!
//! The title bar's menus hold only engine-wide commands. Each editor puts its
//! own menus (Edit, Selection, Go, ...) in its own top bar: it describes them
//! as [`EditorMenu`]s of its actions and renders them with
//! [`EditorMenuStrip`]. Built-in and plugin editors use the same strip, so a
//! plugin editor's menus look and behave like the level editor's.
//!
//! A menu item dispatches its action like a key binding would: to the
//! strip's action context when one is given (the editor's focus handle, so a
//! text editor's Undo reaches the focused text), otherwise from the strip's
//! place in the editor's element tree. The editor handles the actions it
//! lists, as it already does for its shortcuts, and the menu shows each
//! action's key binding.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, Action, App, ElementId, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    popup_menu::{PopupMenu, PopupMenuExt as _},
    Sizable as _,
};

/// One menu of an editor's strip: a title and its items.
pub struct EditorMenu {
    pub label: SharedString,
    pub items: Vec<EditorMenuItem>,
}

/// An entry in an [`EditorMenu`].
pub enum EditorMenuItem {
    /// Dispatches `action`; shown greyed out when not `enabled`.
    Action {
        label: SharedString,
        action: Box<dyn Action>,
        enabled: bool,
    },
    Separator,
    Submenu(EditorMenu),
    /// Undo and Redo of `history`, as [`EditorMenu::undo_redo`] describes.
    /// Read when the menu opens, so they are current even when the editor
    /// did not re-render after its last edit.
    UndoRedo {
        undo: SharedString,
        redo: SharedString,
        history: Option<Arc<dyn crate::undo::HistoryView>>,
    },
}

impl EditorMenu {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            items: Vec::new(),
        }
    }

    /// An item that dispatches `action`.
    pub fn action(self, label: impl Into<SharedString>, action: impl Action) -> Self {
        self.action_if(label, action, true)
    }

    /// An item that dispatches `action`, greyed out unless `enabled`.
    pub fn action_if(
        mut self,
        label: impl Into<SharedString>,
        action: impl Action,
        enabled: bool,
    ) -> Self {
        self.items.push(EditorMenuItem::Action {
            label: label.into(),
            action: Box::new(action),
            enabled,
        });
        self
    }

    /// The Edit menu's Undo and Redo: "`undo` <what>" and "`redo` <what>"
    /// (the caller's translated words), greyed out when `history` has
    /// nothing to undo or redo. They dispatch the shared [`crate::Undo`] and
    /// [`crate::Redo`], which the editor handles for its document.
    pub fn undo_redo(
        mut self,
        undo: impl Into<SharedString>,
        redo: impl Into<SharedString>,
        history: Option<Arc<dyn crate::undo::HistoryView>>,
    ) -> Self {
        self.items.push(EditorMenuItem::UndoRedo {
            undo: undo.into(),
            redo: redo.into(),
            history,
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(EditorMenuItem::Separator);
        self
    }

    pub fn submenu(mut self, menu: EditorMenu) -> Self {
        self.items.push(EditorMenuItem::Submenu(menu));
        self
    }

    /// The items as a popup menu. Builds anew each time the menu opens.
    fn build(
        &self,
        mut popup: PopupMenu,
        window: &mut Window,
        cx: &mut gpui::Context<PopupMenu>,
    ) -> PopupMenu {
        for item in &self.items {
            popup = match item {
                EditorMenuItem::Action {
                    label,
                    action,
                    enabled,
                } => popup.menu_with_disabled(label.clone(), action.boxed_clone(), !enabled),
                EditorMenuItem::Separator => popup.separator(),
                EditorMenuItem::UndoRedo {
                    undo,
                    redo,
                    history,
                } => {
                    let history = history.as_deref();
                    let label = |word: &SharedString, what: Option<String>| -> SharedString {
                        match what {
                            Some(what) if !what.is_empty() => format!("{word} {what}").into(),
                            _ => word.clone(),
                        }
                    };
                    popup
                        .menu_with_disabled(
                            label(undo, history.and_then(|h| h.undo_label())),
                            Box::new(crate::undo::Undo),
                            !history.is_some_and(|h| h.can_undo()),
                        )
                        .menu_with_disabled(
                            label(redo, history.and_then(|h| h.redo_label())),
                            Box::new(crate::undo::Redo),
                            !history.is_some_and(|h| h.can_redo()),
                        )
                }
                EditorMenuItem::Submenu(menu) => {
                    let menu = Rc::new(EditorMenu {
                        label: menu.label.clone(),
                        items: menu.items.iter().map(EditorMenuItem::duplicate).collect(),
                    });
                    let label = menu.label.clone();
                    popup.submenu(label, window, cx, move |popup, window, cx| {
                        menu.build(popup, window, cx)
                    })
                }
            };
        }
        popup
    }
}

impl EditorMenuItem {
    fn duplicate(&self) -> Self {
        match self {
            Self::Action {
                label,
                action,
                enabled,
            } => Self::Action {
                label: label.clone(),
                action: action.boxed_clone(),
                enabled: *enabled,
            },
            Self::Separator => Self::Separator,
            Self::UndoRedo {
                undo,
                redo,
                history,
            } => Self::UndoRedo {
                undo: undo.clone(),
                redo: redo.clone(),
                history: history.clone(),
            },
            Self::Submenu(menu) => Self::Submenu(EditorMenu {
                label: menu.label.clone(),
                items: menu.items.iter().map(Self::duplicate).collect(),
            }),
        }
    }
}

/// An editor's menus as a row of small buttons, each opening its menu.
///
/// Each button has the debug selector `editor-menu-{label}`.
#[derive(IntoElement)]
pub struct EditorMenuStrip {
    id: ElementId,
    menus: Vec<EditorMenu>,
    action_context: Option<FocusHandle>,
}

impl EditorMenuStrip {
    /// `id` tells this strip apart from others in the window (two open
    /// editors of one kind give their own).
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            menus: Vec::new(),
            action_context: None,
        }
    }

    pub fn menu(mut self, menu: EditorMenu) -> Self {
        self.menus.push(menu);
        self
    }

    /// Dispatch the items' actions from `focus` (and return focus there when
    /// a menu closes), instead of from the strip itself.
    pub fn action_context(mut self, focus: FocusHandle) -> Self {
        self.action_context = Some(focus);
        self
    }
}

impl RenderOnce for EditorMenuStrip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let action_context = self.action_context;
        h_flex().id(self.id).gap_0p5().items_center().children(
            self.menus
                .into_iter()
                .enumerate()
                .map(move |(index, menu)| {
                    let menu = Rc::new(menu);
                    let action_context = action_context.clone();
                    let label = menu.label.clone();
                    div()
                        .debug_selector(move || format!("editor-menu-{label}"))
                        .child(
                            Button::new(("editor-menu", index))
                                .label(menu.label.clone())
                                .small()
                                .ghost()
                                .popup_menu(move |popup, window, cx| {
                                    let popup = match &action_context {
                                        Some(focus) => popup.action_context(focus.clone()),
                                        None => popup,
                                    };
                                    menu.build(popup, window, cx)
                                }),
                        )
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        actions, div, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
    };

    use super::{EditorMenu, EditorMenuStrip};

    actions!(menu_strip_test, [Undo, Redo, Disabled]);

    /// An editor that counts the actions its menus dispatch.
    struct Editor {
        focus: FocusHandle,
        undone: usize,
        redone: usize,
    }

    impl Focusable for Editor {
        fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for Editor {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus)
                .size_full()
                .on_action(cx.listener(|this, _: &Undo, _, _| this.undone += 1))
                .on_action(cx.listener(|this, _: &Redo, _, _| this.redone += 1))
                .child(
                    EditorMenuStrip::new("test-strip")
                        .action_context(self.focus.clone())
                        .menu(
                            EditorMenu::new("Edit")
                                .action("Undo", Undo)
                                .action("Redo", Redo)
                                .separator()
                                .action_if("Disabled", Disabled, false),
                        ),
                )
        }
    }

    /// Choosing a menu item dispatches its action to the editor; a disabled
    /// item is skipped.
    #[gpui::test]
    fn menu_items_dispatch_their_actions_to_the_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ui::init(cx);
            ui::themes::init(cx);
        });
        let (editor, cx) = cx.add_window_view(|_, cx| Editor {
            focus: cx.focus_handle(),
            undone: 0,
            redone: 0,
        });
        let cx: &mut VisualTestContext = cx;
        editor.update_in(cx, |editor, window, cx| window.focus(&editor.focus, cx));

        let open = |cx: &mut VisualTestContext| {
            let bounds = cx
                .debug_bounds("editor-menu-Edit")
                .expect("the Edit menu's button");
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
            cx.run_until_parked();
        };

        open(cx);
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| {
            assert_eq!((editor.undone, editor.redone), (1, 0))
        });

        open(cx);
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| {
            assert_eq!((editor.undone, editor.redone), (1, 1))
        });

        // Down skips the separator and the disabled item, wrapping to Undo.
        open(cx);
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| {
            assert_eq!((editor.undone, editor.redone), (2, 1))
        });
    }
}
