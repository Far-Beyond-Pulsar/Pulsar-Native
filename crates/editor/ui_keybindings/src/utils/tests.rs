//! The overrides against a real GPUI keymap, headless: what gets bound, what
//! stops firing, that a rebuild never stacks, and the startup path from a file.

use gpui::{KeyBinding, KeyContext, Keystroke, TestAppContext, actions};

use super::file;
use super::overrides::Override;
use super::registry::{self, USER_META};
use super::store::{KeybindingStore, apply_project_keybindings};

actions!(kbtest, [Alpha, Beta]);

fn press(cx: &gpui::App, keys: &str, context: &[KeyContext]) -> Vec<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let (matched, _) = keymap.bindings_for_input(&[Keystroke::parse(keys).unwrap()], context);
    matched
        .iter()
        .map(|binding| binding.action().name().to_owned())
        .collect()
}

fn entry(keys: &str, action: Option<&str>) -> Override {
    Override {
        keystrokes: keys.into(),
        action: action.map(str::to_owned),
        context: None,
        params: None,
    }
}

#[gpui::test]
fn overrides_unbind_and_rebind_over_the_defaults(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-b", Beta, None),
        ]);
        let warnings = registry::apply(
            &[
                entry("ctrl-a", None),
                entry("ctrl-j", Some("kbtest::Alpha")),
            ],
            cx,
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        assert!(press(cx, "ctrl-a", &[]).is_empty(), "ctrl-a was unbound");
        assert_eq!(press(cx, "ctrl-j", &[]), vec!["kbtest::Alpha"]);
        assert_eq!(press(cx, "ctrl-b", &[]), vec!["kbtest::Beta"], "untouched");

        // The defaults seen by the window never include the user's bindings.
        assert_eq!(registry::defaults(cx).len(), 2);
        let tagged = cx
            .key_bindings()
            .borrow()
            .bindings()
            .filter(|binding| binding.meta() == Some(USER_META))
            .count();
        // Only the added binding is in the keymap; the unbound default is stashed.
        assert_eq!(tagged, 1);
    });
}

#[gpui::test]
fn rebuilding_replaces_the_users_bindings_and_keeps_later_defaults(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        registry::apply(&[entry("ctrl-j", Some("kbtest::Alpha"))], cx);

        // A plugin binds something after the first apply.
        cx.bind_keys([KeyBinding::new("ctrl-b", Beta, None)]);
        registry::apply(&[entry("ctrl-k", Some("kbtest::Alpha"))], cx);

        assert!(
            press(cx, "ctrl-j", &[]).is_empty(),
            "the old override is gone"
        );
        assert_eq!(press(cx, "ctrl-k", &[]), vec!["kbtest::Alpha"]);
        assert_eq!(press(cx, "ctrl-a", &[]), vec!["kbtest::Alpha"]);
        assert_eq!(
            press(cx, "ctrl-b", &[]),
            vec!["kbtest::Beta"],
            "the later default survived"
        );

        registry::apply(&[], cx);
        assert!(press(cx, "ctrl-k", &[]).is_empty());
        assert_eq!(cx.key_bindings().borrow().bindings().count(), 2);
    });
}

#[gpui::test]
fn an_unknown_action_is_reported_and_skipped(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        let warnings = registry::apply(
            &[
                entry("ctrl-j", Some("nope::Missing")),
                entry("ctrl-k", Some("kbtest::Beta")),
            ],
            cx,
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(press(cx, "ctrl-k", &[]), vec!["kbtest::Beta"]);
    });
}

#[gpui::test]
fn startup_binds_the_projects_file_over_the_defaults(cx: &mut TestAppContext) {
    let project = tempfile::tempdir().unwrap();
    file::save(
        project.path(),
        &[
            entry("ctrl-a", None),
            entry("ctrl-J", Some("kbtest::Alpha")),
        ],
    )
    .unwrap();
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        apply_project_keybindings(project.path(), cx);

        assert!(press(cx, "ctrl-a", &[]).is_empty());
        assert_eq!(press(cx, "ctrl-shift-j", &[]), vec!["kbtest::Alpha"]);

        let store = cx.global::<KeybindingStore>();
        assert_eq!(store.project.as_deref(), Some(project.path()));
        // Spellings are normalized on load.
        assert_eq!(store.overrides[1].keystrokes, "ctrl-shift-j");
    });
}

#[gpui::test]
fn a_project_without_a_file_changes_nothing(cx: &mut TestAppContext) {
    let project = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        apply_project_keybindings(project.path(), cx);
        assert_eq!(press(cx, "ctrl-a", &[]), vec!["kbtest::Alpha"]);
        assert!(cx.global::<KeybindingStore>().overrides.is_empty());
    });
}

#[gpui::test]
fn switching_project_loads_that_projects_file(cx: &mut TestAppContext) {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    file::save(second.path(), &[entry("ctrl-a", None)]).unwrap();
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        apply_project_keybindings(first.path(), cx);
        assert_eq!(press(cx, "ctrl-a", &[]), vec!["kbtest::Alpha"]);

        let store = KeybindingStore::for_project(Some(second.path()), cx);
        assert_eq!(store.overrides.len(), 1);
        assert!(press(cx, "ctrl-a", &[]).is_empty());
    });
}

#[gpui::test]
fn unbinding_a_global_shortcut_keeps_contextual_bindings_on_the_same_keys(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-a", Alpha, None),
            KeyBinding::new("ctrl-a", Beta, Some("Input")),
        ]);
        let input = [KeyContext::parse("Input").unwrap()];

        registry::apply(&[entry("ctrl-a", None)], cx);
        assert_eq!(
            press(cx, "ctrl-a", &input),
            vec!["kbtest::Beta"],
            "the contextual binding still fires"
        );
        assert!(
            press(cx, "ctrl-a", &[]).is_empty(),
            "the global one is gone"
        );

        // Changing the file again brings the removed default back.
        registry::apply(&[], cx);
        assert_eq!(press(cx, "ctrl-a", &[]), vec!["kbtest::Alpha"]);
        assert_eq!(cx.key_bindings().borrow().bindings().count(), 2);
    });
}

#[gpui::test]
fn an_unbinding_only_removes_what_was_bound_before_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-a", Alpha, None)]);
        registry::apply(
            &[entry("ctrl-a", None), entry("ctrl-a", Some("kbtest::Beta"))],
            cx,
        );
        assert_eq!(press(cx, "ctrl-a", &[]), vec!["kbtest::Beta"]);
    });
}
