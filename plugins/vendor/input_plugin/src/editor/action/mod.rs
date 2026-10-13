//! The action editor: what an action is, how its value is shaped, when it
//! fires, and a timeline that shows all of that working.
//!
//! The settings are in the middle. Triggers and modifiers are on the right;
//! below are the preview, the mappings that use the action, and problems.

mod behaviors;
mod issues;
mod preview;
mod settings;
mod usage;

use std::path::PathBuf;

use gpui::*;
use plugin_editor_api::{Redo, Undo, UndoService};
use ui::button::{Button, ButtonVariants};
use ui::dock::PanelEvent;
use ui::tag::Tag;
use ui::workspace::Workspace;
use ui::{h_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::library::{self, Library};
use crate::model::{ActionAsset, Severity};

use super::common::chrome::{banner, toolbar_divider};
use super::common::document::Document;
use super::common::panel::{build_workspace, Dock, Layout};
use super::common::toolbar::{document_controls, file_banners, status_tag};
use super::state::{Lib, LibraryModel};
use super::as_panel;

/// Dock channel: panels only move between docks of the same editor kind.
const DOCK_CHANNEL: u32 = 21;

pub use super::ACTION_EDITOR_ID as EDITOR_ID;

/// What every panel of the action editor works on.
#[derive(Clone)]
pub struct ActionCtx {
    pub doc: Entity<Document<ActionAsset>>,
    pub lib: Lib,
}

impl ActionCtx {
    pub fn asset(&self, cx: &App) -> ActionAsset {
        self.doc.read(cx).asset().clone()
    }

    pub fn edit(&self, label: &'static str, merge_key: Option<&'static str>, cx: &mut App, change: impl FnOnce(&mut ActionAsset)) {
        self.doc.update(cx, |d, cx| d.edit(label, merge_key, cx, change));
    }

    /// The file's name without its extension.
    pub fn stem(&self, cx: &App) -> String {
        self.doc.read(cx).file_name()
    }

    /// The project's library with this action as it is now, unsaved edits and all.
    pub fn library_now(&self, cx: &App) -> Library {
        let library = self.lib.read(cx).library();
        let doc = self.doc.read(cx);
        match doc.path() {
            Some(path) => library.with_current_action(path, doc.asset()),
            None => (*library).clone(),
        }
    }
}

pub struct ActionEditor {
    pub(crate) ctx: ActionCtx,
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ActionEditor {
    pub fn new(path: PathBuf, undo: Option<&UndoService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_root = library::project_root(&path);
        let doc = cx.new(|_| {
            let mut doc = Document::<ActionAsset>::open(Some(path));
            if let Some(undo) = undo {
                doc.adopt_undo(undo);
            }
            doc
        });
        let lib = cx.new(|cx| LibraryModel::new(project_root, cx));
        let ctx = ActionCtx { doc: doc.clone(), lib: lib.clone() };

        let settings = cx.new(|cx| settings::SettingsPanel::new(ctx.clone(), window, cx));
        let triggers = cx.new(|cx| behaviors::TriggersPanel::new(ctx.clone(), cx));
        let modifiers = cx.new(|cx| behaviors::ModifiersPanel::new(ctx.clone(), cx));
        let preview = cx.new(|cx| preview::PreviewPanel::new(ctx.clone(), cx));
        let usage = cx.new(|cx| usage::UsagePanel::new(ctx.clone(), cx));
        let issues = cx.new(|cx| issues::IssuesPanel::new(ctx.clone(), cx));

        let layout = Layout {
            center: vec![as_panel(settings)],
            left: None,
            right: Some(Dock::stacked(400.0, vec![vec![as_panel(triggers)], vec![as_panel(modifiers)]])),
            bottom: Some(Dock::grouped(270.0, vec![vec![as_panel(preview)], vec![as_panel(usage), as_panel(issues)]])),
        };
        let workspace = build_workspace("input-action-workspace", DOCK_CHANNEL, layout, window, cx);

        let subscriptions = vec![cx.observe(&doc, |_, _, cx| cx.notify()), cx.observe(&lib, |_, _, cx| cx.notify())];
        Self { ctx, workspace, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library_now(cx);
        let doc = self.ctx.doc.read(cx);
        let asset = doc.asset();
        let name = doc.file_name();
        let id = asset.id.clone();
        let issues = asset.validate(&name, &library);
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
            .child(document_controls("action", &self.ctx.doc, cx))
            .child(toolbar_divider(cx))
            .child(Icon::new(IconName::Flash).size(px(15.0)).text_color(theme.info))
            .child(div().text_sm().font_semibold().text_color(theme.foreground).child(name.clone()))
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.0))
                    .bg(theme.background)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(asset.value_type.label()),
            )
            .child(
                Button::new("action-id")
                    .label(format!("ID {}", id.short()))
                    .ghost()
                    .xsmall()
                    .tooltip(format!("{id}\nThe stable id mappings refer to this action by. Click to copy it."))
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

    /// A banner when another file claims this action's id (a copied file).
    fn id_conflict_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let library = self.ctx.lib.read(cx).library();
        let doc = self.ctx.doc.read(cx);
        let id = &doc.asset().id;
        let path = doc.path()?.to_path_buf();
        let others: Vec<String> = library
            .action_claimants(id)
            .iter()
            .filter(|e| e.path != path && !same_path(&e.path, &path))
            .map(|e| e.relative.clone())
            .collect();
        if others.is_empty() {
            return None;
        }
        let ctx = self.ctx.clone();
        Some(
            banner(
                Severity::Warning,
                format!("{} has the same id as this action (a copy?). Mappings that use either can't tell them apart until one gets a new id.", others.join(", ")),
                vec![Button::new("new-action-id").label("Give this action a new id").small().on_click(move |_, _, cx| {
                    ctx.doc.update(cx, |d, cx| {
                        d.edit("New action id", None, cx, |a| a.id = crate::model::AssetId::new());
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

impl EventEmitter<PanelEvent> for ActionEditor {}

impl Focusable for ActionEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ui::dock::Panel for ActionEditor {
    fn panel_name(&self) -> &'static str {
        EDITOR_ID
    }

    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(self.ctx.doc.read(cx).file_name().into())
    }

    fn tab_icon(&self, _cx: &App) -> Option<IconName> {
        Some(IconName::Flash)
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

impl Render for ActionEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let banners = file_banners("action", &self.ctx.doc, cx);
        let conflict = self.id_conflict_banner(cx);

        div()
            .id("input-action-editor")
            .key_context("InputActionEditor")
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
