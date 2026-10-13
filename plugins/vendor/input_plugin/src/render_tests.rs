//! Renders the real editors headlessly and drives them with the shared
//! commands, drawing after every step. That runs every panel's `render`
//! against real state, which is where re-entrant entity borrows and bad
//! indices would show up.

use std::path::{Path, PathBuf};

use gpui::{TestAppContext, VisualTestContext};
use ui::dock::Panel;

use crate::editor::action::ActionEditor;
use crate::editor::chooser::{ChoiceItem, Chooser};
use crate::editor::mapping::MappingEditor;
use crate::model::behavior::{Modifier, Tolerant, Trigger, ValueType};
use crate::model::presets::presets_for;
use crate::model::{ActionAsset, ActionRef, Asset, MappingAsset};

/// A project folder (with a `Pulsar.toml`, so it is the root) for one test.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("input_plugin_render_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("input")).unwrap();
    std::fs::write(dir.join("Pulsar.toml"), "").unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: String) -> PathBuf {
    let path = dir.join(rel);
    std::fs::write(&path, text).unwrap();
    path
}

/// A few frames: docks size themselves over the first couple.
fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }
    // Let animations (a switch flipping) finish, so nothing is left running
    // when the test ends.
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

/// A project with a move action (2D), a jump (bool) and a mapping binding both,
/// plus the kinds of broken entry the editor must still show.
fn sample_project(name: &str) -> (PathBuf, PathBuf, ActionAsset, ActionAsset) {
    let dir = project(name);
    let mut mv = ActionAsset::new();
    mv.value_type = ValueType::Axis2D;
    mv.modifiers = vec![Tolerant::Known(Modifier::DeadZone { lower: 0.2, upper: 1.0, radial: true })];
    let mut jump = ActionAsset::new();
    jump.triggers = vec![Tolerant::Known(Trigger::Pressed)];
    write(&dir, "input/Move.inputaction", mv.to_text());
    write(&dir, "input/Jump.inputaction", jump.to_text());

    let mut map = MappingAsset::new();
    let cat = map.categories[0].id;
    let mv_ref = ActionRef::new(mv.id.clone(), "input/Move.inputaction");
    let jump_ref = ActionRef::new(jump.id.clone(), "input/Jump.inputaction");
    map.add_preset(cat, &mv_ref, &presets_for(ValueType::Axis2D)[0]);
    map.add_entry(cat, "Keyboard.Space", Some(jump_ref.clone()));
    let combat = map.add_category("Combat");
    map.add_entry(combat, "Gamepad.FaceBottom", Some(jump_ref));
    map.add_entry(combat, "Keyboard.NotAKey", None);
    map.add_entry(combat, "Mouse.Left", Some(ActionRef::new(crate::model::AssetId::new(), "gone.inputaction")));
    let disabled = map.add_entry(combat, "Keyboard.KeyQ", None).unwrap();
    map.entry_mut(disabled).unwrap().enabled = false;
    let path = write(&dir, "input/Game.inputmap", map.to_text());
    (dir, path, mv, jump)
}

#[gpui::test]
fn the_action_editor_renders_and_edits_are_undoable(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (_dir, _, _, _) = sample_project("action_basic");
    let path = _dir.join("input/Jump.inputaction");
    let (editor, cx) = cx.add_window_view(|window, cx| ActionEditor::new(path.clone(), None, window, cx));
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.tab_name(cx)).as_deref(), Some("Jump"));

    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    assert!(!ctx.doc.read_with(cx, |d, _| d.is_dirty()));
    ctx.doc.update(cx, |d, cx| d.edit("Change value type", None, cx, |a| a.value_type = ValueType::Axis1D));
    draw(cx);
    assert!(ctx.doc.read_with(cx, |d, _| d.is_dirty()));
    ctx.doc.update(cx, |d, cx| d.undo(cx));
    draw(cx);
    assert_eq!(ctx.doc.read_with(cx, |d, _| d.asset().value_type), ValueType::Bool);
    assert!(!ctx.doc.read_with(cx, |d, _| d.is_dirty()), "undoing back to the saved state is clean");
}

#[gpui::test]
fn every_trigger_and_modifier_has_controls_that_render(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (dir, _, _, _) = sample_project("action_all");
    let path = dir.join("input/Move.inputaction");
    let (editor, cx) = cx.add_window_view(|window, cx| ActionEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());

    use crate::editor::behavior_ui::Behavior;
    for (_, _, trigger) in Trigger::catalog() {
        ctx.doc.update(cx, |d, cx| d.edit("Add trigger", None, cx, |a| a.triggers.push(Tolerant::Known(trigger))));
        draw(cx);
    }
    for (_, _, modifier) in Modifier::catalog() {
        ctx.doc.update(cx, |d, cx| d.edit("Add modifier", None, cx, |a| a.modifiers.push(Tolerant::Known(modifier))));
        draw(cx);
    }
    // One written by a newer version shows too, and is kept.
    ctx.doc.update(cx, |d, cx| {
        d.edit("Add unknown", None, cx, |a| {
            a.triggers.push(Tolerant::Unknown(serde_json::json!({"type": "from_the_future"})));
            a.modifiers.push(Tolerant::Unknown(serde_json::json!({"type": "from_the_future"})));
        })
    });
    draw(cx);
    let saved = ctx.doc.update(cx, |d, cx| {
        d.save(cx);
        d.asset().to_text()
    });
    assert!(saved.contains("from_the_future"));
    for value_type in ValueType::ALL {
        ctx.doc.update(cx, |d, cx| d.edit("Change value type", None, cx, |a| a.value_type = value_type));
        draw(cx);
    }
}

#[gpui::test]
fn the_action_editor_shows_which_mappings_use_the_action(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (dir, _, _, _) = sample_project("action_usage");
    let path = dir.join("input/Jump.inputaction");
    let (editor, cx) = cx.add_window_view(|window, cx| ActionEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let (id, users) = ctx.lib.read_with(cx, |l, _| {
        let lib = l.library();
        let id = lib.actions().into_iter().find(|a| a.stem == "Jump").unwrap().id.clone();
        let users = lib.mappings_using(&id).len();
        (id, users)
    });
    assert_eq!(users, 1);
    assert_eq!(ctx.doc.read_with(cx, |d, _| d.asset().id.clone()), id);
}

#[gpui::test]
fn the_mapping_editor_renders_a_mapping_with_every_kind_of_entry(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (_dir, path, _, _) = sample_project("map_render");
    let (editor, cx) = cx.add_window_view(|window, cx| MappingEditor::new(path.clone(), None, window, cx));
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.tab_name(cx)).as_deref(), Some("Game"));
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());

    let entries = ctx.doc.read_with(cx, |d, _| d.asset().entry_count());
    assert_eq!(entries, 9);

    // Select every entry and category in turn: the details panel renders for each.
    let (entry_ids, category_ids) = ctx.doc.read_with(cx, |d, _| {
        let m = d.asset();
        (m.entries().map(|(_, e)| e.id).collect::<Vec<_>>(), m.categories.iter().map(|c| c.id).collect::<Vec<_>>())
    });
    for id in entry_ids {
        cx.update(|_, app| ctx.select_entry(id, app));
        draw(cx);
    }
    for id in category_ids {
        cx.update(|_, app| ctx.select_category(id, app));
        draw(cx);
    }
}

#[gpui::test]
fn commands_edit_the_mapping_and_every_state_renders(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (_dir, path, mv, _jump) = sample_project("map_commands");
    let (editor, cx) = cx.add_window_view(|window, cx| MappingEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let count = |cx: &mut VisualTestContext| ctx.doc.read_with(cx, |d, _| d.asset().entry_count());
    let start = count(cx);
    let category = ctx.doc.read_with(cx, |d, _| d.asset().categories[0].id);
    let reference = ActionRef::new(mv.id.clone(), "input/Move.inputaction");

    let added = cx.update(|_, app| ctx.add_binding(category, None, "Keyboard.KeyE", Some(reference.clone()), app)).unwrap();
    draw(cx);
    assert_eq!(count(cx), start + 1);
    assert_eq!(ctx.sel.read_with(cx, |s, _| s.entry()), Some(added), "the new binding is selected");

    cx.update(|_, app| ctx.set_key(added, "Gamepad.LeftStick", app));
    cx.update(|_, app| ctx.set_enabled(added, false, app));
    draw(cx);
    assert_eq!(ctx.doc.read_with(cx, |d, _| d.asset().entry(added).map(|e| (e.key.clone(), e.enabled))), Some(("Gamepad.LeftStick".to_string(), false)));

    cx.update(|_, app| ctx.duplicate_binding(added, app));
    draw(cx);
    assert_eq!(count(cx), start + 2);

    let second = ctx.doc.read_with(cx, |d, _| d.asset().categories[1].id);
    cx.update(|_, app| ctx.move_binding(added, second, 0, app));
    draw(cx);
    assert_eq!(ctx.doc.read_with(cx, |d, _| d.asset().category_of(added).map(|c| c.id)), Some(second));

    let preset = presets_for(ValueType::Axis2D).remove(1);
    cx.update(|_, app| ctx.bind_preset(&reference, &preset, app));
    draw(cx);
    assert_eq!(count(cx), start + 2 + preset.keys.len());

    let new_category = cx.update(|_, app| ctx.add_category(app));
    cx.update(|_, app| ctx.rename_category(new_category, "Menus", app));
    draw(cx);
    cx.update(|_, app| ctx.remove_category(new_category, app));
    draw(cx);

    cx.update(|_, app| ctx.remove_binding(added, app));
    draw(cx);

    // All of it undoes, one step at a time, and still renders.
    let mut undone = 0;
    while ctx.doc.read_with(cx, |d, _| d.can_undo()) && undone < 50 {
        ctx.doc.update(cx, |d, cx| d.undo(cx));
        draw(cx);
        undone += 1;
    }
    assert_eq!(count(cx), start);
    assert!(!ctx.doc.read_with(cx, |d, _| d.is_dirty()));
}

#[gpui::test]
fn every_trigger_and_modifier_renders_on_a_binding(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (_dir, path, _, jump) = sample_project("map_behaviors");
    let (editor, cx) = cx.add_window_view(|window, cx| MappingEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let entry = ctx.doc.read_with(cx, |d, _| d.asset().entries().find(|(_, e)| e.key == "Keyboard.Space").map(|(_, e)| e.id)).unwrap();
    cx.update(|_, app| ctx.select_entry(entry, app));
    draw(cx);

    use crate::editor::behavior_ui::Behavior;
    let chord = Trigger::Chorded { action: ActionRef::new(jump.id.clone(), "input/Jump.inputaction") };
    ctx.doc.update(cx, |d, cx| {
        d.edit("Add", None, cx, |m| {
            let e = m.entry_mut(entry).unwrap();
            for (_, _, t) in Trigger::catalog() {
                e.triggers.push(Tolerant::Known(t));
            }
            e.triggers.push(Tolerant::Known(chord));
            for (_, _, md) in Modifier::catalog() {
                e.modifiers.push(Tolerant::Known(md));
            }
        })
    });
    draw(cx);
}

#[gpui::test]
fn a_new_action_made_from_the_mapping_editor_is_found_by_id(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (dir, path, _, _) = sample_project("map_new_action");
    let (editor, cx) = cx.add_window_view(|window, cx| MappingEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let reference = cx.update(|_, app| ctx.create_action("look around", ValueType::Axis2D, app)).unwrap();
    assert_eq!(reference.hint, "input/LookAround.inputaction");
    assert!(dir.join("input/LookAround.inputaction").is_file());
    draw(cx);
    let found = ctx.lib.read_with(cx, |l, _| l.library().action_entry(&reference.id).map(|a| a.asset.value_type));
    assert_eq!(found, Some(ValueType::Axis2D), "the library picks the new file up");

    // A second one with the same name doesn't overwrite the first.
    let again = cx.update(|_, app| ctx.create_action("look around", ValueType::Bool, app)).unwrap();
    assert_ne!(again.id, reference.id);
    assert_eq!(again.hint, "input/LookAround2.inputaction");
}

#[gpui::test]
fn the_chooser_filters_thousands_of_items_without_building_them_all(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let items: Vec<ChoiceItem> = (0..5000).map(|i| ChoiceItem::new(format!("id{i}"), format!("Action {i}")).detail(format!("input/Action{i}.inputaction")).tag("Bool")).collect();
    let (chooser, cx) = cx.add_window_view(|window, cx| Chooser::new(items.clone(), "", window, cx));
    draw(cx);
    assert_eq!(chooser.read_with(cx, |c, _| c.row_count()), 5000);
    chooser.update_in(cx, |c, window, cx| c.set_query("action 4999", window, cx));
    draw(cx);
    assert_eq!(chooser.read_with(cx, |c, _| c.row_count()), 1);
    chooser.update_in(cx, |c, window, cx| c.set_query("action 42", window, cx));
    draw(cx);
    let filtered = chooser.read_with(cx, |c, _| c.row_count());
    assert!(filtered > 1 && filtered < 5000, "{filtered} rows match `action 42`");
    chooser.update_in(cx, |c, window, cx| c.set_query("nothing like this", window, cx));
    draw(cx);
    assert_eq!(chooser.read_with(cx, |c, _| c.row_count()), 0);
}

#[gpui::test]
fn a_file_without_an_id_gets_one_when_opened(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let dir = project("stamp");
    let path = write(&dir, "input/Fresh.inputaction", String::new());
    let (editor, cx) = cx.add_window_view(|window, cx| ActionEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let id = editor.read_with(cx, |e, cx| e.ctx.asset(cx).id);
    assert!(id.is_assigned());
    assert_eq!(crate::model::peek_id(&std::fs::read_to_string(&path).unwrap()), Some(id), "and it is on disk");

    let map = write(&dir, "input/Fresh.inputmap", String::new());
    let (editor, cx) = cx.add_window_view(|window, cx| MappingEditor::new(map.clone(), None, window, cx));
    draw(cx);
    let id = editor.read_with(cx, |e, cx| e.ctx.asset(cx).id);
    assert_eq!(crate::model::peek_id(&std::fs::read_to_string(&map).unwrap()), Some(id));
}



