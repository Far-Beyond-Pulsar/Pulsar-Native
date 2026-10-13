//! Renders the real editor headlessly and drives it with the mouse and the
//! shared commands, drawing after every step. That runs every panel's
//! `render` against real state, which is where re-entrant entity borrows and
//! bad indices would show up.

use std::path::PathBuf;

use gpui::{point, px, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use ui::dock::Panel;

use crate::editor::WidgetEditor;
use crate::model::{Asset, Kind, NodeId, SlotKind, Vec2, WidgetAsset};

fn temp_file(name: &str, content: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ui_editor_render_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// A few frames: docks size themselves over the first couple.
fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }
}

/// A widget with a panel holding a title, and a loose button on the canvas.
fn sample_file(name: &str) -> (PathBuf, NodeId, NodeId) {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let mut panel = w.new_node(Kind::Border);
    panel.name = "Panel".into();
    panel.set_prop("background", serde_json::json!("#334455"));
    let panel_id = panel.id;
    w.add(root, None, panel).unwrap();
    w.node_mut(panel_id).unwrap().set_slot(SlotKind::Canvas, "offsets", serde_json::json!([200.0, 150.0, 400.0, 240.0]));
    let mut button = w.new_node(Kind::Button);
    button.name = "Play".into();
    let button_id = button.id;
    w.add(root, None, button).unwrap();
    w.node_mut(button_id).unwrap().set_slot(SlotKind::Canvas, "offsets", serde_json::json!([800.0, 600.0, 160.0, 48.0]));
    (temp_file(name, &w.to_text()), panel_id, button_id)
}

/// The window position of `id`'s centre, from the designer's own view of the world.
fn center_of(editor: &gpui::Entity<WidgetEditor>, id: NodeId, cx: &mut VisualTestContext) -> gpui::Point<gpui::Pixels> {
    let designer = editor.read_with(cx, |e, _| e.designer.clone());
    let (bounds, frame) = designer.read_with(cx, |d, _| (d.bounds.get(), d.frame.clone()));
    let frame = frame.expect("the designer has drawn");
    let rect = frame.screen[&id];
    let canvas = editor.read_with(cx, |e, cx| e.ctx.view.read(cx).to_canvas(Vec2::new(rect.center().x, rect.center().y)));
    point(bounds.origin.x + px(canvas.x), bounds.origin.y + px(canvas.y))
}

#[gpui::test]
fn the_editor_renders_and_a_click_selects_the_widget_under_it(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (path, panel, button) = sample_file("select.widget");
    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(path.clone(), None, window, cx));
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.tab_name(cx)).as_deref(), Some("select"));

    let at = center_of(&editor, panel, cx);
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.ctx.selected(cx)), vec![panel]);

    let at = center_of(&editor, button, cx);
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.ctx.selected(cx)), vec![button]);

    // Shift-click adds to the selection.
    let at = center_of(&editor, panel, cx);
    cx.simulate_click(at, Modifiers::shift());
    draw(cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.ctx.selected(cx)), vec![button, panel]);
}

#[gpui::test]
fn dragging_a_widget_moves_it_as_one_undo_step(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (path, _panel, button) = sample_file("drag.widget");
    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(path.clone(), None, window, cx));
    draw(cx);

    let offsets = |editor: &gpui::Entity<WidgetEditor>, cx: &mut VisualTestContext| {
        editor.read_with(cx, |e, cx| e.ctx.widget(cx).node(button).unwrap().slot_view(SlotKind::Canvas).quad("offsets"))
    };
    let before = offsets(&editor, cx);
    let scale = editor.read_with(cx, |e, cx| e.ctx.view.read(cx).scale());

    let start = center_of(&editor, button, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    // Move in several steps, as a real drag does, with no snapping to confuse the arithmetic.
    editor.update(cx, |e, cx| e.ctx.view.update(cx, |v, _| v.snap = false));
    for step in 1..=5 {
        cx.simulate_mouse_move(point(start.x + px(20.0 * step as f32), start.y + px(10.0 * step as f32)), MouseButton::Left, Modifiers::none());
        draw(cx);
    }
    cx.simulate_mouse_up(point(start.x + px(100.0), start.y + px(50.0)), MouseButton::Left, Modifiers::none());
    draw(cx);

    let after = offsets(&editor, cx);
    assert!((after[0] - before[0] - 100.0 / scale).abs() < 0.6, "{before:?} -> {after:?} at scale {scale}");
    assert!((after[1] - before[1] - 50.0 / scale).abs() < 0.6);
    assert_eq!((after[2], after[3]), (before[2], before[3]), "moving doesn't resize");

    // The whole drag is one step.
    editor.update(cx, |e, cx| e.ctx.doc.update(cx, |d, cx| d.undo(cx)));
    draw(cx);
    assert_eq!(offsets(&editor, cx), before, "one undo puts it back");
    editor.update(cx, |e, cx| e.ctx.doc.update(cx, |d, cx| d.redo(cx)));
    draw(cx);
    assert_eq!(offsets(&editor, cx), after);
}

#[gpui::test]
fn commands_edit_the_widget_and_every_state_renders(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let (path, panel, button) = sample_file("commands.widget");
    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let count = |cx: &mut VisualTestContext| editor.read_with(cx, |e, cx| e.ctx.widget(cx).root.count());
    let start = count(cx);

    // Add every kind of widget, then take them all away again with undo.
    for kind in Kind::ALL {
        if kind == Kind::Instance {
            continue;
        }
        cx.update(|_, cx| {
            ctx.add_node(kind, None, cx);
        });
        draw(cx);
    }
    assert!(count(cx) > start + Kind::ALL.len() - 2, "every kind was added");

    // Select things and exercise structure commands.
    cx.update(|_, cx| {
        ctx.select(vec![panel], cx);
        ctx.wrap_selected(Kind::Overlay, cx);
    });
    draw(cx);
    cx.update(|_, cx| ctx.unwrap_selected(cx));
    draw(cx);
    cx.update(|_, cx| {
        ctx.select(vec![button], cx);
        ctx.duplicate_selected(cx);
    });
    draw(cx);
    cx.update(|_, cx| {
        ctx.copy_selected(cx);
        ctx.paste(cx);
        ctx.delete_selected(cx);
    });
    draw(cx);
    cx.update(|_, cx| ctx.select_all(cx));
    draw(cx);
    cx.update(|_, cx| ctx.nudge_selected(Vec2::new(5.0, 5.0), cx));
    draw(cx);

    // Undo everything: each intermediate state must render, and the file is clean again.
    for _ in 0..200 {
        let can = editor.read_with(cx, |e, cx| e.ctx.doc.read(cx).can_undo());
        if !can {
            break;
        }
        editor.update(cx, |e, cx| e.ctx.doc.update(cx, |d, cx| d.undo(cx)));
        draw(cx);
    }
    assert_eq!(count(cx), start);
    assert!(!editor.read_with(cx, |e, cx| e.tab_unsaved(cx)));
}

#[gpui::test]
fn a_file_without_an_id_gets_one_on_open(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let path = temp_file("fresh.widget", "");
    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let id = editor.read_with(cx, |e, cx| e.ctx.widget(cx).id.clone());
    let on_disk = WidgetAsset::peek_id(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(on_disk, Some(id));
    assert!(!editor.read_with(cx, |e, cx| e.tab_unsaved(cx)));
}

#[gpui::test]
fn nested_widgets_parameters_and_details_render_for_every_kind(cx: &mut TestAppContext) {
    use crate::model::{Arg, ValueClass};
    cx.update(ui::init);
    let dir = std::env::temp_dir().join(format!("ui_editor_nested_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Pulsar.toml"), "").unwrap();

    // A card with a title parameter and a body slot.
    let mut card = WidgetAsset::new();
    let root = card.root.id;
    let mut title = card.new_node(Kind::Text);
    title.name = "Title".into();
    let title_id = title.id;
    card.add(root, None, title).unwrap();
    let mut slot = card.new_node(Kind::NamedSlot);
    slot.name = "Body".into();
    let sid = slot.id;
    card.add(root, None, slot).unwrap();
    card.node_mut(sid).unwrap().set_slot(SlotKind::Canvas, "offsets", serde_json::json!([0.0, 40.0, 200.0, 100.0]));
    let param = card.add_parameter("Heading", ValueClass::Text);
    card.link_parameter(param, title_id, "text").unwrap();
    std::fs::write(dir.join("Card.widget"), card.to_text()).unwrap();

    // A menu that uses it twice.
    let mut menu = WidgetAsset::new();
    let root = menu.root.id;
    let mut first = menu.new_instance(card.id.clone(), "Card");
    first.instance.as_mut().unwrap().overrides.insert(param, Arg::Value(serde_json::json!("Sword")));
    let first_id = first.id;
    menu.add(root, None, first).unwrap();
    menu.node_mut(first_id).unwrap().set_slot(SlotKind::Canvas, "offsets", serde_json::json!([50.0, 50.0, 250.0, 150.0]));
    let mut filler = menu.new_node(Kind::Button);
    filler.for_slot = Some("Body".into());
    let filler_id = filler.id;
    menu.add(first_id, None, filler).unwrap();
    let menu_path = dir.join("Menu.widget");
    std::fs::write(&menu_path, menu.to_text()).unwrap();

    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(menu_path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    assert_eq!(ctx.lib.read_with(cx, |l, _| l.library().len()), 2, "both widgets are found by id");

    // Select the instance and its slot content, then one of every kind.
    for id in [first_id, filler_id] {
        cx.update(|_, cx| ctx.select(vec![id], cx));
        draw(cx);
    }
    for kind in Kind::ALL {
        if kind == Kind::Instance {
            continue;
        }
        let id = cx.update(|_, cx| ctx.add_node(kind, None, cx)).expect("added");
        cx.update(|_, cx| ctx.select(vec![id], cx));
        draw(cx);
    }
    // Make a parameter from a property, then drive it.
    let text = cx.update(|_, cx| ctx.add_node(Kind::Text, None, cx)).unwrap();
    let made = cx.update(|_, cx| ctx.parameterize(text, "text", cx));
    draw(cx);
    assert!(made.is_some());
    cx.update(|_, cx| ctx.select(Vec::new(), cx));
    draw(cx);

    // Moving the file keeps the reference good.
    std::fs::create_dir_all(dir.join("moved")).unwrap();
    std::fs::rename(dir.join("Card.widget"), dir.join("moved/Renamed.widget")).unwrap();
    ctx.lib.update(cx, |l, cx| l.refresh(cx));
    draw(cx);
    let library = ctx.lib.read_with(cx, |l, _| l.library());
    assert!(library.entry(&card.id).is_some());
    assert_eq!(library.entry(&card.id).unwrap().name, "Renamed");
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui::test]
fn several_widgets_can_be_added_to_a_horizontal_box(cx: &mut TestAppContext) {
    cx.update(ui::init);
    let path = temp_file("hbox.widget", "");
    let (editor, cx) = cx.add_window_view(|window, cx| WidgetEditor::new(path.clone(), None, window, cx));
    draw(cx);
    let ctx = editor.read_with(cx, |e, _| e.ctx.clone());
    let hbox = cx.update(|_, cx| ctx.add_node(Kind::HorizontalBox, None, cx)).unwrap();
    draw(cx);
    // Keep adding with no explicit target, as the double-click on the palette does.
    for kind in [Kind::Text, Kind::Button, Kind::Border, Kind::Image, Kind::Slider] {
        cx.update(|_, cx| ctx.select(vec![hbox], cx));
        cx.update(|_, cx| ctx.add_node(kind, None, cx));
        draw(cx);
    }
    // And with the previous addition still selected.
    for kind in [Kind::Text, Kind::Border, Kind::Text] {
        cx.update(|_, cx| ctx.add_node(kind, None, cx));
        draw(cx);
    }
    let n = editor.read_with(cx, |e, cx| e.ctx.widget(cx).node(hbox).unwrap().children.len());
    assert_eq!(n, 8, "every addition lands in the box");
}
