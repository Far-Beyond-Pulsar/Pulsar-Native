//! The mapping editor: which keys drive which actions.
//!
//! The bindings, sorted into categories, are in the middle. The project's
//! actions are on the left, ready to drag onto a category or to bind with a
//! ready-made set of keys. The selected binding or category is on the right,
//! and problems are below.

mod actions;
mod bindings;
mod details;
mod issues;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use plugin_editor_api::{Redo, Undo, UndoService};
use ui::button::{Button, ButtonVariants};
use ui::dock::PanelEvent;
use ui::tag::Tag;
use ui::workspace::Workspace;
use ui::{h_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::library::{self, Library};
use crate::model::behavior::ValueType;
use crate::model::presets::Preset;
use crate::model::{ActionAsset, ActionRef, Asset, AssetId, MappingAsset, Severity};

use super::common::chrome::toolbar_divider;
use super::common::document::Document;
use super::common::panel::{build_workspace, Dock, Layout};
use super::common::toolbar::{document_controls, file_banners, status_tag};
use super::state::{Lib, LibraryModel, Sel, Selection};
use super::as_panel;

/// Dock channel: panels only move between docks of the same editor kind.
const DOCK_CHANNEL: u32 = 22;

pub use super::MAPPING_EDITOR_ID as EDITOR_ID;

/// What every panel of the mapping editor works on, and the commands they
/// share, so a menu, a button and a key all do an edit the same way.
#[derive(Clone)]
pub struct MapCtx {
    pub doc: Entity<Document<MappingAsset>>,
    pub sel: Sel,
    pub lib: Lib,
}

impl MapCtx {
    pub fn asset(&self, cx: &App) -> MappingAsset {
        self.doc.read(cx).asset().clone()
    }

    pub fn library(&self, cx: &App) -> Arc<Library> {
        self.lib.read(cx).library()
    }

    pub fn edit(&self, label: &'static str, merge_key: Option<&'static str>, cx: &mut App, change: impl FnOnce(&mut MappingAsset)) {
        self.doc.update(cx, |d, cx| d.edit(label, merge_key, cx, change));
    }

    pub fn selected_entry(&self, cx: &App) -> Option<u32> {
        self.sel.read(cx).entry()
    }

    pub fn selected_category(&self, cx: &App) -> Option<u32> {
        self.sel.read(cx).category()
    }

    pub fn select_entry(&self, id: u32, cx: &mut App) {
        let Some(category) = self.doc.read(cx).asset().category_of(id).map(|c| c.id) else { return };
        self.sel.update(cx, |s, cx| s.select_entry(id, category, cx));
    }

    pub fn select_category(&self, id: u32, cx: &mut App) {
        self.sel.update(cx, |s, cx| s.select_category(id, cx));
    }

    /// The category new bindings go into: the selected one, the selected
    /// binding's, or the first.
    pub fn target_category(&self, cx: &App) -> u32 {
        let asset = self.doc.read(cx).asset();
        self.selected_category(cx)
            .filter(|c| asset.category(*c).is_some())
            .or_else(|| asset.categories.first().map(|c| c.id))
            .unwrap_or(1)
    }

    // ── Bindings ───────────────────────────────────────────────────────────

    /// Add a binding to `category` (at the end, or at `index`) and select it.
    pub fn add_binding(&self, category: u32, index: Option<usize>, key: &str, action: Option<ActionRef>, cx: &mut App) -> Option<u32> {
        let mut created = None;
        self.edit("Add binding", None, cx, |m| {
            if let Some(id) = m.add_entry(category, key, action) {
                if let Some(index) = index {
                    m.move_entry(id, category, index);
                }
                created = Some(id);
            }
        });
        if let Some(id) = created {
            self.select_entry(id, cx);
        }
        created
    }

    pub fn remove_binding(&self, id: u32, cx: &mut App) {
        self.edit("Remove binding", None, cx, |m| m.remove_entry(id));
    }

    pub fn duplicate_binding(&self, id: u32, cx: &mut App) {
        let mut copy = None;
        self.edit("Duplicate binding", None, cx, |m| copy = m.duplicate_entry(id));
        if let Some(copy) = copy {
            self.select_entry(copy, cx);
        }
    }

    pub fn move_binding(&self, id: u32, category: u32, index: usize, cx: &mut App) {
        self.edit("Move binding", None, cx, |m| m.move_entry(id, category, index));
        self.select_entry(id, cx);
    }

    pub fn set_key(&self, id: u32, key: &str, cx: &mut App) {
        let key = key.to_string();
        self.edit("Change key", None, cx, |m| m.set_key(id, &key));
    }

    pub fn set_action(&self, id: u32, action: Option<ActionRef>, cx: &mut App) {
        self.edit("Change action", None, cx, |m| m.set_action(id, action));
    }

    pub fn set_enabled(&self, id: u32, enabled: bool, cx: &mut App) {
        self.edit("Enable or disable binding", None, cx, |m| {
            if let Some(e) = m.entry_mut(id) {
                e.enabled = enabled;
            }
        });
    }

    /// Bind `action` with a preset's keys into the target category.
    pub fn bind_preset(&self, action: &ActionRef, preset: &Preset, cx: &mut App) {
        let category = self.target_category(cx);
        let mut first = None;
        self.edit("Add bindings", None, cx, |m| first = m.add_preset(category, action, preset).first().copied());
        if let Some(id) = first {
            self.select_entry(id, cx);
        }
    }

    // ── Categories ─────────────────────────────────────────────────────────

    pub fn add_category(&self, cx: &mut App) -> u32 {
        let mut id = 0;
        self.edit("Add category", None, cx, |m| id = m.add_category("Category"));
        self.select_category(id, cx);
        id
    }

    pub fn rename_category(&self, id: u32, name: &str, cx: &mut App) {
        let name = name.to_string();
        self.edit("Rename category", Some("category-name"), cx, |m| m.rename_category(id, &name));
    }

    pub fn remove_category(&self, id: u32, cx: &mut App) {
        self.edit("Remove category", None, cx, |m| m.remove_category(id));
    }

    pub fn move_category(&self, id: u32, to: usize, cx: &mut App) {
        self.edit("Move category", None, cx, |m| m.move_category(id, to));
    }

    // ── Actions ────────────────────────────────────────────────────────────

    /// Point a binding at the action a chooser reported: an id, or `""` for
    /// none, or `"+new"` to make a new action first.
    pub fn assign_action(&self, entry: u32, chosen: &str, cx: &mut App) {
        match chosen {
            "" => self.set_action(entry, None, cx),
            "+new" => match self.create_action("NewAction", ValueType::Bool, cx) {
                Ok(reference) => self.set_action(entry, Some(reference), cx),
                Err(error) => tracing::warn!("input: couldn't make an action: {error}"),
            },
            id => {
                let library = self.library(cx);
                if let Some(reference) = action_ref(&library, id) {
                    self.set_action(entry, Some(reference), cx);
                }
            }
        }
    }

    /// Make a new action file beside this mapping and return a reference to
    /// it. The file gets its id now, so the reference is good immediately.
    pub fn create_action(&self, name: &str, value_type: ValueType, cx: &mut App) -> Result<ActionRef, String> {
        let doc = self.doc.read(cx);
        let mapping_path = doc.path().ok_or("Save the mapping first so the action has somewhere to go")?.to_path_buf();
        let dir = mapping_path.parent().ok_or("The mapping isn't in a folder")?.to_path_buf();
        let stem = crate::model::action::to_identifier(name);
        let mut path = dir.join(format!("{stem}.{}", crate::model::action::ACTION_EXTENSION));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{stem}{n}.{}", crate::model::action::ACTION_EXTENSION));
            n += 1;
        }
        let mut action = ActionAsset::new();
        action.value_type = value_type;
        std::fs::write(&path, action.to_text()).map_err(|e| e.to_string())?;
        let root = self.lib.read(cx).root().to_path_buf();
        let hint = library::relative_path(&root, &path);
        self.lib.update(cx, |l, cx| l.refresh(cx));
        Ok(ActionRef::new(action.id, hint))
    }
}

pub struct MappingEditor {
    pub(crate) ctx: MapCtx,
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl MappingEditor {
    pub fn new(path: PathBuf, undo: Option<&UndoService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_root = library::project_root(&path);
        let doc = cx.new(|_| {
            let mut doc = Document::<MappingAsset>::open(Some(path));
            if let Some(undo) = undo {
                doc.adopt_undo(undo);
            }
            doc
        });
        let sel = cx.new(|_| Selection::default());
        let lib = cx.new(|cx| LibraryModel::new(project_root, cx));
        let ctx = MapCtx { doc: doc.clone(), sel: sel.clone(), lib: lib.clone() };

        // Undo and revert can remove what is selected.
        let subscriptions = vec![
            cx.observe(&doc, {
                let sel = sel.clone();
                move |_, doc, cx| {
                    let mapping = doc.read(cx).asset().clone();
                    sel.update(cx, |s, cx| s.retain_existing(&mapping, cx));
                    cx.notify();
                }
            }),
            cx.observe(&lib, |_, _, cx| cx.notify()),
        ];

        let bindings = cx.new(|cx| bindings::BindingsPanel::new(ctx.clone(), window, cx));
        let actions_panel = cx.new(|cx| actions::ActionsPanel::new(ctx.clone(), window, cx));
        let details = cx.new(|cx| details::DetailsPanel::new(ctx.clone(), window, cx));
        let issues = cx.new(|cx| issues::IssuesPanel::new(ctx.clone(), cx));

        let layout = Layout {
            center: vec![as_panel(bindings)],
            left: Some(Dock::new(320.0, vec![as_panel(actions_panel)])),
            right: Some(Dock::new(380.0, vec![as_panel(details)])),
            bottom: Some(Dock::new(200.0, vec![as_panel(issues)])),
        };
        let workspace = build_workspace("input-mapping-workspace", DOCK_CHANNEL, layout, window, cx);
        Self { ctx, workspace, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library(cx);
        let doc = self.ctx.doc.read(cx);
        let asset = doc.asset();
        let name = doc.file_name();
        let id = asset.id.clone();
        let issues = asset.validate(library.as_ref());
        let worst = issues.iter().map(|i| i.severity).max();
        let copy_id = id.to_string();
        let count = asset.entry_count();

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
            .child(document_controls("mapping", &self.ctx.doc, cx))
            .child(toolbar_divider(cx))
            .child(Icon::new(IconName::Key).size(px(15.0)).text_color(theme.warning))
            .child(div().text_sm().font_semibold().text_color(theme.foreground).child(name))
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.0))
                    .bg(theme.background)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{count} bindings · priority {}", asset.priority)),
            )
            .child(
                Button::new("mapping-id")
                    .label(format!("ID {}", id.short()))
                    .ghost()
                    .xsmall()
                    .tooltip(format!("{id}\nThe stable id a component or script can refer to this mapping by. Click to copy it."))
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
}

impl EventEmitter<PanelEvent> for MappingEditor {}

impl Focusable for MappingEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ui::dock::Panel for MappingEditor {
    fn panel_name(&self) -> &'static str {
        EDITOR_ID
    }

    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(self.ctx.doc.read(cx).file_name().into())
    }

    fn tab_icon(&self, _cx: &App) -> Option<IconName> {
        Some(IconName::Key)
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

impl Render for MappingEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let banners = file_banners("mapping", &self.ctx.doc, cx);

        div()
            .id("input-mapping-editor")
            .key_context("InputMappingEditor")
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
            .child(div().flex_1().min_h_0().child(self.workspace.clone()))
    }
}

/// An action's id from the text a chooser reports.
pub(crate) fn action_ref(library: &Library, id: &str) -> Option<ActionRef> {
    let id = AssetId::parse(id)?;
    let entry = library.action_entry(&id)?;
    Some(ActionRef::new(id, entry.relative.clone()))
}
