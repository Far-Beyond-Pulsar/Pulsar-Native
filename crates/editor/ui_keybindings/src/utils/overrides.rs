//! The user's keybinding changes, as data, and the rules for editing them.
//!
//! Nothing here touches GPUI. The keymap is described by plain [`Entry`]
//! values in the order the keymap holds them, and the user's changes are a list
//! of [`Override`]s applied after the defaults. Later entries win, the same
//! rule the keymap uses at dispatch time, so what [`effective`] computes is
//! what the keymap will do once the overrides are bound.
//!
//! An override either binds an action to keystrokes or, with `action: None`,
//! *unbinds* them: it removes every earlier binding of the same keystrokes in
//! the same context (no context means the bindings that have none). It does not
//! reach bindings in other contexts, and it is not a `NoAction` in the keymap:
//! at dispatch a context-less entry outranks contextual ones, so a mask would
//! also silence them. [`remove_binding`] puts back anything else that shared the
//! slot with the binding being removed.
//!
//! Every edit ends in [`compact`], which drops each override that changes
//! nothing, so removing a default and adding it back leaves the file as it was.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line of the user's keybinding file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Override {
    /// Space separated keystrokes (a chord), e.g. `ctrl-k ctrl-s`.
    pub keystrokes: String,
    /// The action to bind, or `null` to unbind these keystrokes.
    #[serde(default)]
    pub action: Option<String>,
    /// Where the binding applies; no context means everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Parameters of an action that takes some.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// An action bound to keystrokes in a context.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub action: String,
    pub keystrokes: String,
    pub context: Option<String>,
    pub params: Option<Value>,
}

impl Binding {
    pub fn new(action: &str, keystrokes: &str, context: Option<&str>) -> Self {
        Self {
            action: action.to_owned(),
            keystrokes: keystrokes.to_owned(),
            context: context.map(str::to_owned),
            params: None,
        }
    }

    /// The same binding, whatever its parameters.
    pub fn same_slot(&self, other: &Binding) -> bool {
        self.action == other.action
            && self.keystrokes == other.keystrokes
            && self.context == other.context
    }

    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.action,
            self.keystrokes,
            self.context.as_deref().unwrap_or(""),
            self.params
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default()
        )
    }
}

/// One binding of the keymap, in keymap order.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Bind(Binding),
    /// An unbinding (`NoAction`).
    Mask {
        keystrokes: String,
        context: Option<String>,
    },
}

impl Override {
    pub fn to_entry(&self) -> Entry {
        match &self.action {
            Some(action) => Entry::Bind(Binding {
                action: action.clone(),
                keystrokes: self.keystrokes.clone(),
                context: self.context.clone(),
                params: self.params.clone(),
            }),
            None => Entry::Mask {
                keystrokes: self.keystrokes.clone(),
                context: self.context.clone(),
            },
        }
    }

    fn bind(binding: &Binding) -> Self {
        Self {
            keystrokes: binding.keystrokes.clone(),
            action: Some(binding.action.clone()),
            context: binding.context.clone(),
            params: binding.params.clone(),
        }
    }

    fn mask(keystrokes: &str, context: &Option<String>) -> Self {
        Self {
            keystrokes: keystrokes.to_owned(),
            action: None,
            context: context.clone(),
            params: None,
        }
    }
}

/// An unbinding removes a binding of the same keystrokes in exactly its context.
fn covers(mask_context: &Option<String>, binding_context: &Option<String>) -> bool {
    mask_context == binding_context
}

fn entries(defaults: &[Entry], overrides: &[Override]) -> Vec<Entry> {
    defaults
        .iter()
        .cloned()
        .chain(overrides.iter().map(Override::to_entry))
        .collect()
}

/// The bindings that are live: every binding no later mask covers, in order.
pub fn effective(entries: &[Entry]) -> Vec<Binding> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(ix, entry)| {
            let Entry::Bind(binding) = entry else {
                return None;
            };
            let hidden = entries[ix + 1..].iter().any(|later| {
                matches!(later, Entry::Mask { keystrokes, context }
                    if *keystrokes == binding.keystrokes && covers(context, &binding.context))
            });
            (!hidden).then(|| binding.clone())
        })
        .collect()
}

/// [`effective`] of the defaults followed by the overrides.
pub fn effective_with(defaults: &[Entry], overrides: &[Override]) -> Vec<Binding> {
    effective(&entries(defaults, overrides))
}

/// For each binding, the later one that takes its keystrokes in the same
/// context (so it never fires), if there is one.
pub fn shadowed_by(bindings: &[Binding]) -> Vec<Option<usize>> {
    bindings
        .iter()
        .enumerate()
        .map(|(ix, binding)| {
            bindings
                .iter()
                .enumerate()
                .skip(ix + 1)
                .rev()
                .find(|(_, later)| {
                    later.keystrokes == binding.keystrokes
                        && later.context == binding.context
                        && !later.same_slot(binding)
                })
                .map(|(later_ix, _)| later_ix)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClashKind {
    /// The very same keystrokes in the same context.
    Same,
    /// The new keystrokes are the start of this chord.
    StartsChord,
    /// This binding is the start of the new chord.
    ExtendsBinding,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clash {
    pub binding: Binding,
    pub kind: ClashKind,
}

fn strokes(keystrokes: &str) -> Vec<&str> {
    keystrokes.split_whitespace().collect()
}

/// What binding `keystrokes` in `context` would run into. `except` is the
/// action being edited, whose own bindings are not clashes.
pub fn find_clashes(
    bindings: &[Binding],
    keystrokes: &str,
    context: &Option<String>,
    except: Option<&str>,
) -> Vec<Clash> {
    let new = strokes(keystrokes);
    bindings
        .iter()
        .filter(|binding| Some(binding.action.as_str()) != except)
        .filter(|binding| binding.context == *context)
        .filter_map(|binding| {
            let old = strokes(&binding.keystrokes);
            let kind = if old == new {
                ClashKind::Same
            } else if old.len() > new.len() && old[..new.len()] == new[..] {
                ClashKind::StartsChord
            } else if new.len() > old.len() && new[..old.len()] == old[..] {
                ClashKind::ExtendsBinding
            } else {
                return None;
            };
            Some(Clash {
                binding: binding.clone(),
                kind,
            })
        })
        .collect()
}

fn sorted_keys(bindings: &[Binding]) -> Vec<String> {
    let mut keys: Vec<String> = bindings.iter().map(Binding::key).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Drop every override that changes nothing, last to first.
pub fn compact(defaults: &[Entry], overrides: &mut Vec<Override>) {
    loop {
        let with = sorted_keys(&effective_with(defaults, overrides));
        let redundant = (0..overrides.len()).rev().find(|&ix| {
            let mut without = overrides.clone();
            without.remove(ix);
            sorted_keys(&effective_with(defaults, &without)) == with
        });
        match redundant {
            Some(ix) => {
                overrides.remove(ix);
            }
            None => break,
        }
    }
}

/// Bind `binding`. With `replace_clashes`, every other action using exactly
/// these keystrokes in this context loses them first.
pub fn add_binding(
    defaults: &[Entry],
    overrides: &mut Vec<Override>,
    binding: Binding,
    replace_clashes: bool,
) {
    if replace_clashes {
        let taken: Vec<Binding> = effective_with(defaults, overrides)
            .into_iter()
            .filter(|live| {
                live.keystrokes == binding.keystrokes
                    && live.context == binding.context
                    && live.action != binding.action
            })
            .collect();
        for other in taken {
            remove_binding(defaults, overrides, &other);
        }
    }
    let already = effective_with(defaults, overrides)
        .iter()
        .any(|live| live.same_slot(&binding));
    if !already {
        overrides.push(Override::bind(&binding));
    }
    compact(defaults, overrides);
}

/// Unbind `target`, leaving every other live binding as it was.
pub fn remove_binding(defaults: &[Entry], overrides: &mut Vec<Override>, target: &Binding) {
    let before = effective_with(defaults, overrides);
    overrides.retain(|o| {
        !(o.action.as_deref() == Some(target.action.as_str())
            && o.keystrokes == target.keystrokes
            && o.context == target.context)
    });
    let still_live = effective_with(defaults, overrides)
        .iter()
        .any(|live| live.same_slot(target));
    if still_live {
        // A default provides it: mask it, then put back what the mask also hides.
        overrides.push(Override::mask(&target.keystrokes, &target.context));
        let after = effective_with(defaults, overrides);
        for lost in before {
            if !lost.same_slot(target) && !after.contains(&lost) {
                overrides.push(Override::bind(&lost));
            }
        }
    }
    compact(defaults, overrides);
}

/// Back to the defaults for one action.
pub fn reset_action(defaults: &[Entry], overrides: &mut Vec<Override>, action: &str) {
    overrides.retain(|o| o.action.as_deref() != Some(action));
    overrides.retain(|o| {
        o.action.is_some()
            || !defaults.iter().any(|entry| {
                matches!(entry, Entry::Bind(b)
                    if b.action == action
                        && b.keystrokes == o.keystrokes
                        && covers(&o.context, &b.context))
            })
    });
    compact(defaults, overrides);
}

/// Whether the user changed anything about `action`.
pub fn is_customized(defaults: &[Entry], overrides: &[Override], action: &str) -> bool {
    overrides.iter().any(|o| {
        o.action.as_deref() == Some(action)
            || (o.action.is_none()
                && defaults.iter().any(|entry| {
                    matches!(entry, Entry::Bind(b)
                        if b.action == action
                            && b.keystrokes == o.keystrokes
                            && covers(&o.context, &b.context))
                }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(action: &str, keys: &str, context: Option<&str>) -> Entry {
        Entry::Bind(Binding::new(action, keys, context))
    }

    fn live(defaults: &[Entry], overrides: &[Override]) -> Vec<(String, String)> {
        let mut out: Vec<_> = effective_with(defaults, overrides)
            .into_iter()
            .map(|b| (b.action, b.keystrokes))
            .collect();
        out.sort();
        out
    }

    fn pair(action: &str, keys: &str) -> (String, String) {
        (action.to_owned(), keys.to_owned())
    }

    #[test]
    fn removing_a_default_masks_it_and_reset_brings_it_back() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::Toggle", "ctrl-space", None),
        );
        assert_eq!(live(&defaults, &overrides), vec![]);
        assert_eq!(overrides.len(), 1);
        assert!(overrides[0].action.is_none());
        assert!(is_customized(&defaults, &overrides, "a::Toggle"));

        reset_action(&defaults, &mut overrides, "a::Toggle");
        assert!(overrides.is_empty());
        assert_eq!(
            live(&defaults, &overrides),
            vec![pair("a::Toggle", "ctrl-space")]
        );
        assert!(!is_customized(&defaults, &overrides, "a::Toggle"));
    }

    #[test]
    fn rebinding_removes_the_old_keys_and_adds_the_new() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::Toggle", "ctrl-space", None),
        );
        add_binding(
            &defaults,
            &mut overrides,
            Binding::new("a::Toggle", "ctrl-j", None),
            false,
        );
        assert_eq!(
            live(&defaults, &overrides),
            vec![pair("a::Toggle", "ctrl-j")]
        );
    }

    #[test]
    fn removing_a_binding_the_user_added_leaves_no_trace() {
        let defaults = [];
        let mut overrides = Vec::new();
        let added = Binding::new("a::Run", "f5", None);
        add_binding(&defaults, &mut overrides, added.clone(), false);
        assert_eq!(overrides.len(), 1);
        remove_binding(&defaults, &mut overrides, &added);
        assert!(overrides.is_empty());
    }

    #[test]
    fn adding_back_a_removed_default_restores_the_file_to_empty() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        let target = Binding::new("a::Toggle", "ctrl-space", None);
        remove_binding(&defaults, &mut overrides, &target);
        add_binding(&defaults, &mut overrides, target, false);
        assert!(overrides.is_empty());
    }

    #[test]
    fn adding_an_existing_binding_changes_nothing() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        add_binding(
            &defaults,
            &mut overrides,
            Binding::new("a::Toggle", "ctrl-space", None),
            false,
        );
        assert!(overrides.is_empty());
    }

    #[test]
    fn replacing_a_clash_takes_the_keys_from_the_other_action() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        add_binding(
            &defaults,
            &mut overrides,
            Binding::new("b::Other", "ctrl-space", None),
            true,
        );
        assert_eq!(
            live(&defaults, &overrides),
            vec![pair("b::Other", "ctrl-space")]
        );
        // The other action is shown as changed too.
        assert!(is_customized(&defaults, &overrides, "a::Toggle"));
    }

    #[test]
    fn without_replacing_both_actions_hold_the_keys_and_the_later_shadows() {
        let defaults = [bind("a::Toggle", "ctrl-space", None)];
        let mut overrides = Vec::new();
        add_binding(
            &defaults,
            &mut overrides,
            Binding::new("b::Other", "ctrl-space", None),
            false,
        );
        let bindings = effective_with(&defaults, &overrides);
        assert_eq!(bindings.len(), 2);
        assert_eq!(shadowed_by(&bindings), vec![Some(1), None]);
    }

    #[test]
    fn unbinding_a_global_binding_leaves_a_contextual_one_on_the_same_keys_alone() {
        let defaults = [
            bind("a::Global", "ctrl-c", None),
            bind("fm::Copy", "ctrl-c", Some("FileManager")),
        ];
        let mut overrides = Vec::new();
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::Global", "ctrl-c", None),
        );
        assert_eq!(
            live(&defaults, &overrides),
            vec![pair("fm::Copy", "ctrl-c")]
        );
        // One entry in the file: nothing had to be put back.
        assert_eq!(overrides.len(), 1);
    }

    #[test]
    fn unbinding_one_of_two_actions_sharing_a_slot_keeps_the_other() {
        let defaults = [bind("a::One", "f1", None), bind("a::Two", "f1", None)];
        let mut overrides = Vec::new();
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::One", "f1", None),
        );
        assert_eq!(live(&defaults, &overrides), vec![pair("a::Two", "f1")]);
    }

    #[test]
    fn contexts_keep_the_same_keys_apart() {
        let defaults = [
            bind("fm::Copy", "ctrl-c", Some("FileManager")),
            bind("ed::Copy", "ctrl-c", Some("Editor")),
        ];
        let bindings = effective_with(&defaults, &[]);
        assert_eq!(shadowed_by(&bindings), vec![None, None]);
        assert!(
            find_clashes(
                &bindings,
                "ctrl-c",
                &Some("Editor".into()),
                Some("fm::Copy")
            )
            .iter()
            .all(|c| c.binding.action == "ed::Copy")
        );
    }

    #[test]
    fn clashes_include_chords_that_start_with_or_extend_the_keys() {
        let bindings = vec![
            Binding {
                action: "a::Open".into(),
                keystrokes: "ctrl-k ctrl-s".into(),
                context: None,
                params: None,
            },
            Binding::new("a::Cut", "ctrl-x", None),
        ];
        let starts = find_clashes(&bindings, "ctrl-k", &None, None);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].kind, ClashKind::StartsChord);
        let extends = find_clashes(&bindings, "ctrl-x ctrl-y", &None, None);
        assert_eq!(extends[0].kind, ClashKind::ExtendsBinding);
        let same = find_clashes(&bindings, "ctrl-x", &None, Some("a::Open"));
        assert_eq!(same[0].kind, ClashKind::Same);
        assert!(find_clashes(&bindings, "ctrl-x", &None, Some("a::Cut")).is_empty());
    }

    #[test]
    fn reset_action_drops_only_that_actions_changes() {
        let defaults = [bind("a::One", "f1", None), bind("a::Two", "f2", None)];
        let mut overrides = Vec::new();
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::One", "f1", None),
        );
        add_binding(
            &defaults,
            &mut overrides,
            Binding::new("a::One", "f9", None),
            false,
        );
        remove_binding(
            &defaults,
            &mut overrides,
            &Binding::new("a::Two", "f2", None),
        );
        reset_action(&defaults, &mut overrides, "a::One");
        assert_eq!(live(&defaults, &overrides), vec![pair("a::One", "f1")]);
        assert!(is_customized(&defaults, &overrides, "a::Two"));
    }

    #[test]
    fn overrides_round_trip_through_json() {
        let overrides = vec![
            Override {
                keystrokes: "f5".into(),
                action: Some("a::Run".into()),
                context: None,
                params: None,
            },
            Override {
                keystrokes: "ctrl-space".into(),
                action: None,
                context: Some("Editor".into()),
                params: None,
            },
        ];
        let text = serde_json::to_string(&overrides).unwrap();
        assert!(text.contains("\"action\":null"));
        let back: Vec<Override> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, overrides);
    }
}
