//! What the window's controls do. Every edit goes through [`commit`]: change the
//! overrides, write them to the project, bind them, then rebuild the list.

use gpui::*;

use crate::screen::{KeybindingsWindow, Notice, Recorder};
use crate::utils::overrides::{self, Binding, Override};
use crate::utils::registry;
use crate::utils::store::KeybindingStore;
use crate::utils::{Entry, Filter, file};

/// The longest chord the recorder accepts.
const MAX_STROKES: usize = 4;

type Cx<'a> = Context<'a, KeybindingsWindow>;

fn commit(
    this: &mut KeybindingsWindow,
    cx: &mut Cx,
    success: Option<String>,
    edit: impl FnOnce(&[Entry], &mut Vec<Override>),
) -> bool {
    let project = KeybindingsWindow::project();
    let defaults = registry::defaults(cx);
    // Make sure the project's file is what the store holds before editing it.
    KeybindingStore::for_project(project.as_deref(), cx);
    let mut previous = Vec::new();
    let saved = cx.update_global(|store: &mut KeybindingStore, cx| {
        previous = store.overrides.clone();
        let mut next = previous.clone();
        edit(&defaults, &mut next);
        store.overrides = next;
        store.save_and_apply(cx)
    });
    let ok = match saved {
        Ok(()) => {
            this.notice = success.map(Notice::Info);
            true
        }
        Err(error) => {
            // Nothing was written: put the list back as it was.
            cx.update_global(|store: &mut KeybindingStore, _| store.overrides = previous);
            this.notice = Some(Notice::Error(error));
            false
        }
    };
    this.confirm_reset_all = false;
    this.refresh(cx);
    cx.notify();
    ok
}

pub(crate) fn set_filter(this: &mut KeybindingsWindow, filter: Filter, cx: &mut Cx) {
    this.filter = filter;
    this.confirm_reset_all = false;
    this.update_visible();
    cx.notify();
}

pub(crate) fn remove_binding(this: &mut KeybindingsWindow, binding: Binding, cx: &mut Cx) {
    let message = format!("Removed {}", binding.keystrokes);
    commit(this, cx, Some(message), |defaults, overrides| {
        overrides::remove_binding(defaults, overrides, &binding)
    });
}

pub(crate) fn reset_action(this: &mut KeybindingsWindow, action: String, cx: &mut Cx) {
    commit(
        this,
        cx,
        Some("Restored the default shortcuts".to_owned()),
        |defaults, overrides| overrides::reset_action(defaults, overrides, &action),
    );
}

/// The first click arms it, the second does it.
pub(crate) fn reset_all(this: &mut KeybindingsWindow, cx: &mut Cx) {
    if !this.confirm_reset_all {
        this.confirm_reset_all = true;
        cx.notify();
        return;
    }
    commit(
        this,
        cx,
        Some("Every shortcut is back to its default".to_owned()),
        |_, overrides| overrides.clear(),
    );
}

pub(crate) fn reveal_file(this: &mut KeybindingsWindow, cx: &mut Cx) {
    let Some(project) = KeybindingsWindow::project() else {
        this.notice = Some(Notice::Error("Open a project first".to_owned()));
        cx.notify();
        return;
    };
    let path = file::keybindings_path(&project);
    if path.is_file() {
        cx.reveal_path(&path);
    } else if let Some(dir) = path.parent() {
        // Nothing has been changed yet; show where it will be written.
        let _ = std::fs::create_dir_all(dir);
        cx.reveal_path(dir);
    }
}

// ── Recording ────────────────────────────────────────────────────────────────

pub(crate) fn start_recording(
    this: &mut KeybindingsWindow,
    action_name: String,
    action_title: String,
    replacing: Option<Binding>,
    window: &mut Window,
    cx: &mut Cx,
) {
    let handle = window.window_handle();
    let weak = cx.entity().downgrade();
    // Fires before action dispatch, so the keys being recorded do not also run
    // whatever they are bound to.
    let intercept = cx.intercept_keystrokes(move |event, window, cx| {
        if window.window_handle() != handle {
            return;
        }
        let keystroke = event.keystroke.clone();
        let handled = weak
            .update(cx, |this, cx| record_key(this, &keystroke, window, cx))
            .unwrap_or(false);
        if handled {
            cx.stop_propagation();
        }
    });
    let context = replacing
        .as_ref()
        .and_then(|binding| binding.context.clone())
        .unwrap_or_default();
    this.context_input
        .update(cx, |input, cx| input.set_value(context, window, cx));
    this.recorder = Some(Recorder {
        action_name,
        action_title,
        replacing,
        keys: Vec::new(),
        error: None,
        _intercept: intercept,
    });
    this.confirm_reset_all = false;
    window.focus(&this.focus_handle, cx);
    cx.notify();
}

pub(crate) fn close_recorder(this: &mut KeybindingsWindow, cx: &mut Cx) {
    this.recorder = None;
    cx.notify();
}

pub(crate) fn clear_recording(this: &mut KeybindingsWindow, cx: &mut Cx) {
    if let Some(recorder) = &mut this.recorder {
        recorder.keys.clear();
        recorder.error = None;
    }
    cx.notify();
}

fn is_modifier(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "ctrl" | "alt" | "platform" | "function" | "cmd" | "super" | "win"
    )
}

/// One key went down while recording. Returns whether it was taken (so it does
/// not also trigger its action).
pub(crate) fn record_key(
    this: &mut KeybindingsWindow,
    keystroke: &Keystroke,
    window: &mut Window,
    cx: &mut Cx,
) -> bool {
    if this.recorder.is_none() {
        return false;
    }
    // Typing a context is typing, not recording.
    if this.context_input.focus_handle(cx).is_focused(window) {
        return false;
    }
    let key = keystroke.key.as_str();
    if is_modifier(key) {
        return true;
    }
    let modifiers = &keystroke.modifiers;
    let plain = !(modifiers.control
        || modifiers.alt
        || modifiers.shift
        || modifiers.platform
        || modifiers.function);
    let empty = this.recorder.as_ref().is_none_or(|r| r.keys.is_empty());
    match (plain, key) {
        (true, "escape") => close_recorder(this, cx),
        (true, "enter") if !empty => save_recording(this, window, cx),
        (true, "backspace") => {
            if let Some(recorder) = &mut this.recorder {
                recorder.keys.pop();
                recorder.error = None;
            }
            cx.notify();
        }
        _ => {
            if let Some(recorder) = &mut this.recorder {
                if recorder.keys.len() >= MAX_STROKES {
                    recorder.error =
                        Some(format!("A shortcut can have at most {MAX_STROKES} keys"));
                } else {
                    recorder.keys.push(keystroke.unparse());
                    recorder.error = None;
                }
            }
            cx.notify();
        }
    }
    true
}

/// Bind what was recorded, taking the keys from any action that has them in
/// the same context.
pub(crate) fn save_recording(this: &mut KeybindingsWindow, _window: &mut Window, cx: &mut Cx) {
    let Some(recorder) = &this.recorder else {
        return;
    };
    if recorder.keys.is_empty() {
        return;
    }
    let context_text = this.context_input.read(cx).value().to_string();
    let context = match registry::normalize_context(&context_text) {
        Ok(context) => context,
        Err(error) => {
            if let Some(recorder) = &mut this.recorder {
                recorder.error = Some(format!("The context is not valid: {error}"));
            }
            cx.notify();
            return;
        }
    };
    let keystrokes = match registry::normalize_keystrokes(&recorder.keys.join(" ")) {
        Ok(keystrokes) => keystrokes,
        Err(error) => {
            if let Some(recorder) = &mut this.recorder {
                recorder.error = Some(error);
            }
            cx.notify();
            return;
        }
    };
    let binding = Binding {
        action: recorder.action_name.clone(),
        keystrokes: keystrokes.clone(),
        context,
        params: None,
    };
    let replacing = recorder.replacing.clone();
    let message = format!("{} is now {}", recorder.action_title, keystrokes);
    let saved = commit(this, cx, Some(message), |defaults, overrides| {
        if let Some(old) = &replacing {
            overrides::remove_binding(defaults, overrides, old);
        }
        overrides::add_binding(defaults, overrides, binding, true);
    });
    if saved {
        this.recorder = None;
        cx.notify();
    }
}

// ── All the shortcuts of one action ──────────────────────────────────────────

pub(crate) fn show_details(this: &mut KeybindingsWindow, action: String, cx: &mut Cx) {
    this.details = Some(action);
    cx.notify();
}

pub(crate) fn close_details(this: &mut KeybindingsWindow, cx: &mut Cx) {
    this.details = None;
    cx.notify();
}
