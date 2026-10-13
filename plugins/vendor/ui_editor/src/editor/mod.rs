//! The widget editor: a docked workspace of panels over one shared document.
//!
//! The designer (centre) draws the widget and is where it is edited by
//! direct manipulation. Around it: the palette and library of things to add,
//! the hierarchy, the details of whatever is selected, and below, the
//! widget's parameters and problems. All of them share one
//! [`common::document::Document`], so every edit is undoable and every panel
//! stays in step.

pub mod canvas;
pub mod commands;
pub mod common;
pub mod designer;
pub mod details;
pub mod fields;
pub mod frame;
pub mod hierarchy;
pub mod icons;
pub mod issues;
pub mod keys;
pub mod menus;
pub mod palette;
pub mod parameters;
pub mod value_editor;
pub mod state;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use plugin_editor_api::{Redo, Undo, UndoService};
use ui::button::{Button, ButtonVariants};
use ui::dock::{PanelEvent, PanelView};
use ui::tag::Tag;
use ui::workspace::Workspace;
use ui::{h_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::library;
use crate::model::validate::WidgetSource;
use crate::model::{Severity, WidgetAsset};

use commands::Ctx;
use common::chrome::{banner, toolbar_divider};
use common::document::Document;
use common::panel::{build_workspace, Dock, Layout};
use common::toolbar::{document_controls, file_banners, status_tag};
use designer::DesignerPanel;
use details::DetailsPanel;
use hierarchy::HierarchyPanel;
use issues::IssuesPanel;
use palette::{LibraryPanel, PalettePanel};
use parameters::ParametersPanel;
use state::{LibraryModel, Selection, View};

pub const EDITOR_ID: &str = "widget-editor";

/// Dock channel: panels only move between docks of the same editor kind.
const DOCK_CHANNEL: u32 = 9;

fn as_panel<P: PanelView + 'static>(panel: P) -> Arc<dyn PanelView> {
    Arc::new(panel)
}

pub struct WidgetEditor {
    pub(crate) ctx: Ctx,
    pub(crate) designer: Entity<DesignerPanel>,
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl WidgetEditor {
    pub fn new(path: PathBuf, undo: Option<&UndoService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_root = library::project_root(&path);
        let doc = cx.new(|_| {
            let mut doc = Document::<WidgetAsset>::open(Some(path));
            if let Some(undo) = undo {
                doc.adopt_undo(undo);
            }
            doc
        });
        let sel = cx.new(|_| Selection::default());
        let view = cx.new(|_| View::default());
        let lib = cx.new(|cx| LibraryModel::new(project_root.clone(), cx));
        let ctx = Ctx { doc: doc.clone(), sel: sel.clone(), view, lib, frame: Rc::new(RefCell::new(None)) };

        // Undo and revert can remove the nodes that are selected.
        let subscriptions = vec![cx.observe(&doc, {
            let sel = sel.clone();
            move |_, doc, cx| {
                let widget = doc.read(cx).asset().clone();
                sel.update(cx, |s, cx| s.retain_existing(&widget, cx));
                cx.notify();
            }
        })];

        let designer = cx.new(|cx| DesignerPanel::new(ctx.clone(), project_root, cx));
        let hierarchy = cx.new(|cx| HierarchyPanel::new(ctx.clone(), window, cx));
        let palette = cx.new(|cx| PalettePanel::new(ctx.clone(), window, cx));
        let library_panel = cx.new(|cx| LibraryPanel::new(ctx.clone(), window, cx));
        let details = cx.new(|cx| DetailsPanel::new(ctx.clone(), window, cx));
        let parameters = cx.new(|cx| ParametersPanel::new(ctx.clone(), window, cx));
        let issues = cx.new(|cx| IssuesPanel::new(ctx.clone(), cx));

        let layout = Layout {
            center: vec![as_panel(designer.clone())],
            left: Some(Dock::stacked(300.0, vec![vec![as_panel(palette), as_panel(library_panel)], vec![as_panel(hierarchy)]])),
            right: Some(Dock::new(350.0, vec![as_panel(details)])),
            // Side by side rather than tabbed: both are glanceable while designing.
            bottom: Some(Dock::grouped(250.0, vec![vec![as_panel(parameters)], vec![as_panel(issues)]])),
        };
        let workspace = build_workspace("widget-workspace", DOCK_CHANNEL, layout, window, cx);

        Self { ctx, designer, workspace, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let doc = self.ctx.doc.read(cx);
        let widget = doc.asset();
        let name = doc.file_name();
        let id = widget.id.clone();
        let library = self.ctx.library_now(cx);
        let issues = widget.validate(&library);
        let worst = issues.iter().map(|i| i.severity).max();
        let copy_id = id.to_string();

        h_flex()
            .w_full()
            .flex_shrink_0()
            .h(px(40.0))
            .px_2()
            .gap_2()
            .items_center()
            .bg(theme.secondary)
            .border_b_1()
            .border_color(theme.border)
            .child(document_controls("widget", &self.ctx.doc, cx))
            .child(toolbar_divider(cx))
            .child(Icon::new(IconName::Component).size(px(15.0)).text_color(theme.info))
            .child(div().text_sm().font_semibold().text_color(theme.foreground).child(name))
            .child(
                Button::new("widget-id")
                    .label(format!("ID {}", id.short()))
                    .ghost()
                    .xsmall()
                    .tooltip(format!("{id}\nThe stable id other widgets refer to this one by. Click to copy it."))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()))),
            )
            .child(div().flex_1())
            .children(worst.map(|w| match w {
                Severity::Error => Tag::danger().small().child(format!("{} issues", issues.len())).into_any_element(),
                Severity::Warning => Tag::warning().small().child(format!("{} issues", issues.len())).into_any_element(),
                Severity::Info => Tag::info().small().child(format!("{} notes", issues.len())).into_any_element(),
            }))
            .child(status_tag(&self.ctx.doc, cx))
    }

    /// A banner when another file claims this widget's id (a copied file).
    fn id_conflict_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let library = self.ctx.lib.read(cx).library();
        let doc = self.ctx.doc.read(cx);
        let id = &doc.asset().id;
        let path = doc.path()?.to_path_buf();
        let others: Vec<String> = library
            .claimants(id)
            .iter()
            .filter(|e| e.path != path && !same_path(&e.path, &path))
            .map(|e| e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .collect();
        if others.is_empty() || !library.is_ambiguous(id) {
            return None;
        }
        let ctx = self.ctx.clone();
        Some(
            banner(
                Severity::Warning,
                format!("{} has the same id as this widget (a copy?). Widgets that use either can't tell them apart until one gets a new id.", others.join(", ")),
                vec![Button::new("new-widget-id").label("Give this widget a new id").small().on_click(move |_, _, cx| {
                    ctx.doc.update(cx, |d, cx| {
                        d.edit("New widget id", None, cx, |w| w.regenerate_id());
                        d.save(cx);
                    })
                })],
                cx,
            )
            .into_any_element(),
        )
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

impl EventEmitter<PanelEvent> for WidgetEditor {}

impl Focusable for WidgetEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ui::dock::Panel for WidgetEditor {
    fn panel_name(&self) -> &'static str {
        EDITOR_ID
    }

    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(self.ctx.doc.read(cx).file_name().into())
    }

    fn tab_icon(&self, _cx: &App) -> Option<IconName> {
        Some(IconName::Component)
    }

    fn tab_unsaved(&self, cx: &App) -> bool {
        self.ctx.doc.read(cx).is_dirty()
    }

    fn panel_file_path(&self, cx: &App) -> Option<PathBuf> {
        self.ctx.doc.read(cx).path().map(PathBuf::from)
    }

    fn title(&self, _window: &Window, cx: &App) -> AnyElement {
        SharedString::from(self.ctx.doc.read(cx).file_name()).into_any_element()
    }
}

impl Render for WidgetEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let banners = file_banners("widget", &self.ctx.doc, cx);
        let conflict = self.id_conflict_banner(cx);

        div()
            .id("widget-editor")
            .key_context("WidgetEditor")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            // The host's shared Undo/Redo act on this file's history.
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.ctx.doc.update(cx, |d, cx| d.undo(cx))))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.ctx.doc.update(cx, |d, cx| d.redo(cx))))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.secondary() && event.keystroke.key == "s" {
                    this.ctx.doc.update(cx, |d, cx| d.save(cx));
                    cx.stop_propagation();
                }
            }))
            .child(toolbar)
            .children(banners)
            .children(conflict)
            .child(div().flex_1().min_h_0().child(self.workspace.clone()))
    }
}
