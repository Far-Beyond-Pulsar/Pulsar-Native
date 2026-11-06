//! The window itself, headless: it lists actions, edits save to the project and
//! rebind live, and the recorder captures real keystrokes.

use gpui::{
    AppContext as _, Bounds, KeyBinding, Keystroke, TestAppContext, VisualTestContext,
    WindowBounds, WindowOptions, actions, point, px, size,
};

use crate::handlers;
use crate::screen::KeybindingsWindow;
use crate::utils::file;
use crate::utils::view_model::Filter;

actions!(kbwin, [Alpha, Beta]);

/// The project path lives in a process-wide context, so these tests take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn take_turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn press(cx: &gpui::App, keys: &str) -> Vec<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let (matched, _) = keymap.bindings_for_input(&[Keystroke::parse(keys).unwrap()], &[]);
    matched
        .iter()
        .map(|binding| binding.action().name().to_owned())
        .collect()
}

fn open(
    cx: &mut TestAppContext,
    project: &std::path::Path,
) -> (gpui::Entity<KeybindingsWindow>, VisualTestContext) {
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.display().to_string());
    cx.update(|cx| {
        ui::init(cx);
        ui::themes::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-b", Beta, None),
        ]);
    });
    let mut view = None;
    let window = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1100.), px(760.)),
            ))),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let entity = cx.new(|cx| KeybindingsWindow::new(window, cx));
            view = Some(entity.clone());
            entity
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (view.unwrap(), cx)
}

fn action_index(view: &gpui::Entity<KeybindingsWindow>, cx: &VisualTestContext, name: &str) -> usize {
    view.read_with(cx, |window, _| {
        window
            .entries
            .iter()
            .position(|entry| entry.info.name == name)
            .unwrap_or_else(|| panic!("{name} is not listed"))
    })
}

#[gpui::test]
fn the_window_lists_actions_with_their_shortcuts(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let project = tempfile::tempdir().unwrap();
    let (view, mut cx) = open(cx, project.path());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();

    let alpha = action_index(&view, &cx, "kbwin::Alpha");
    view.read_with(&cx, |window, _| {
        let entry = &window.entries[alpha];
        assert_eq!(entry.info.title, "Alpha");
        assert_eq!(entry.bindings.len(), 1);
        assert_eq!(entry.bindings[0].binding.keystrokes, "ctrl-a");
        assert!(!entry.customized);
        assert!(window.visible.contains(&alpha));
        assert!(window.count(Filter::Customized) == 0);
    });
}

#[gpui::test]
fn recording_a_shortcut_saves_to_the_project_and_rebinds_live(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let project = tempfile::tempdir().unwrap();
    let (view, mut cx) = open(cx, project.path());

    // Change Alpha from ctrl-a to ctrl-j, with real keystrokes.
    let old = view.read_with(&cx, |window, _| {
        window.entries[action_index(&view, &cx_ref(&cx), "kbwin::Alpha")].bindings[0]
            .binding
            .clone()
    });
    view.update_in(&mut cx, |window, win, cx| {
        handlers::start_recording(
            window,
            "kbwin::Alpha".into(),
            "Alpha".into(),
            Some(old),
            win,
            cx,
        );
    });
    cx.simulate_keystrokes("ctrl-j");
    view.read_with(&cx, |window, _| {
        assert_eq!(window.recorder.as_ref().unwrap().keys, vec!["ctrl-j"]);
    });
    // Plain Backspace takes the last key back; Escape closes without saving.
    cx.simulate_keystrokes("backspace");
    view.read_with(&cx, |window, _| assert!(window.recorder.as_ref().unwrap().keys.is_empty()));
    cx.simulate_keystrokes("ctrl-j enter");

    view.read_with(&cx, |window, _| {
        assert!(window.recorder.is_none(), "enter saved and closed the dialog");
    });
    cx.update(|_, cx| {
        assert!(press(cx, "ctrl-a").is_empty(), "the old shortcut is gone");
        assert_eq!(press(cx, "ctrl-j"), vec!["kbwin::Alpha"]);
    });
    let saved = file::load(project.path());
    assert_eq!(saved.overrides.len(), 2, "an unbind and a bind");

    // The list now shows the change.
    let alpha = action_index(&view, &cx, "kbwin::Alpha");
    view.read_with(&cx, |window, _| {
        let entry = &window.entries[alpha];
        assert!(entry.customized);
        assert_eq!(entry.bindings.len(), 1);
        assert_eq!(entry.bindings[0].binding.keystrokes, "ctrl-j");
        assert!(entry.bindings[0].user);
    });

    // Reset puts everything back and leaves an empty file.
    view.update_in(&mut cx, |window, _, cx| {
        handlers::reset_action(window, "kbwin::Alpha".into(), cx)
    });
    cx.update(|_, cx| assert_eq!(press(cx, "ctrl-a"), vec!["kbwin::Alpha"]));
    assert!(file::load(project.path()).overrides.is_empty());
}

#[gpui::test]
fn taking_a_shortcut_from_another_action_is_one_edit(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let project = tempfile::tempdir().unwrap();
    let (view, mut cx) = open(cx, project.path());

    view.update_in(&mut cx, |window, win, cx| {
        handlers::start_recording(window, "kbwin::Alpha".into(), "Alpha".into(), None, win, cx);
    });
    // ctrl-b belongs to Beta.
    cx.simulate_keystrokes("ctrl-b enter");
    cx.update(|_, cx| assert_eq!(press(cx, "ctrl-b"), vec!["kbwin::Alpha"]));
    let beta = action_index(&view, &cx, "kbwin::Beta");
    view.read_with(&cx, |window, _| {
        assert!(window.entries[beta].bindings.is_empty());
        assert!(window.entries[beta].customized);
    });

    view.update_in(&mut cx, |window, _, cx| handlers::reset_all(window, cx));
    // The first click only arms the button.
    cx.update(|_, cx| assert_eq!(press(cx, "ctrl-b"), vec!["kbwin::Alpha"]));
    view.update_in(&mut cx, |window, _, cx| handlers::reset_all(window, cx));
    cx.update(|_, cx| assert_eq!(press(cx, "ctrl-b"), vec!["kbwin::Beta"]));
}

#[gpui::test]
fn escape_closes_the_recorder_without_changing_anything(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let project = tempfile::tempdir().unwrap();
    let (view, mut cx) = open(cx, project.path());
    view.update_in(&mut cx, |window, win, cx| {
        handlers::start_recording(window, "kbwin::Alpha".into(), "Alpha".into(), None, win, cx);
    });
    cx.simulate_keystrokes("ctrl-j escape");
    view.read_with(&cx, |window, _| assert!(window.recorder.is_none()));
    cx.update(|_, cx| assert_eq!(press(cx, "ctrl-a"), vec!["kbwin::Alpha"]));
    assert!(!file::keybindings_path(project.path()).exists());
}

fn cx_ref(cx: &VisualTestContext) -> &VisualTestContext {
    cx
}

#[gpui::test]
fn an_action_with_many_shortcuts_opens_a_details_dialog(cx: &mut TestAppContext) {
    let _turn = take_turn();
    let project = tempfile::tempdir().unwrap();
    let (view, mut cx) = open(cx, project.path());
    cx.update(|_, cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-x", Alpha, Some("One")),
            KeyBinding::new("ctrl-y", Alpha, Some("Two")),
        ]);
    });
    view.update(&mut cx, |window, cx| window.refresh(cx));
    let alpha = action_index(&view, &cx, "kbwin::Alpha");
    view.read_with(&cx, |window, _| assert_eq!(window.entries[alpha].bindings.len(), 3));

    view.update(&mut cx, |window, cx| {
        handlers::show_details(window, "kbwin::Alpha".into(), cx)
    });
    cx.run_until_parked();
    view.read_with(&cx, |window, _| assert_eq!(window.details.as_deref(), Some("kbwin::Alpha")));
    view.update(&mut cx, |window, cx| handlers::close_details(window, cx));
    view.read_with(&cx, |window, _| assert!(window.details.is_none()));
}
