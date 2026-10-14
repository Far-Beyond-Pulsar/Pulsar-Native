//! The pinned ledger: requirements, constraints, decisions, open questions and
//! files touched.
//!
//! Pins are ordinary `Pinned` ledger entries. This module is the only code
//! that adds, edits or removes them, renders them as the cache-stable block
//! the projector places at the end of the system prompt, and checks proposed
//! changes (from the steward) against provenance before they land.
//!
//! Provenance rule: a pin written by the user, or accepted by the user, is
//! `Source::User`. Text that came from tools, the web or sub-agents never
//! becomes a pin by itself; a change derived from such content is held until
//! the user accepts it ([`Acceptance::User`]).

use crate::ledger::{EntryKind, Ledger, LedgerEntry, PinKind, Source};
use crate::profile::ModelProfile;
use crate::taint::{path_args, ToolClass, ToolClassifier};
use agent_chat_core::ToolCall;

/// Longest text a single pin may carry.
pub const MAX_PIN_CHARS: usize = 400;
/// Files-touched pins kept (oldest are removed first).
pub const MAX_FILES_TOUCHED: usize = 40;

/// Who is adding a pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinAuthor {
    /// The user pinned or accepted it.
    User,
    /// The engine maintains it (files touched).
    Engine,
}

/// How the ledger is allowed to be changed by a [`PinDiff`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acceptance {
    /// The user accepted the change in the panel: everything applies.
    User,
    /// No user in the loop: only changes derived from trusted content apply.
    AutoTrusted,
}

/// Where a proposed change came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Derived only from what the user said.
    Trusted,
    /// Derived (even partly) from tool, web or sub-agent content, or the
    /// steward could not show otherwise.
    Untrusted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinOp {
    Add { text: String },
    Remove { text: String },
    Replace { old: String, new: String },
}

/// One proposed edit to the pinned ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinChange {
    pub kind: PinKind,
    pub op: PinOp,
    pub provenance: Provenance,
}

impl PinChange {
    pub fn summary(&self) -> String {
        let tag = match self.provenance {
            Provenance::Trusted => "",
            Provenance::Untrusted => " (from untrusted content)",
        };
        match &self.op {
            PinOp::Add { text } => format!("+ {}: {text}{tag}", self.kind.label()),
            PinOp::Remove { text } => format!("- {}: {text}{tag}", self.kind.label()),
            PinOp::Replace { old, new } => {
                format!("~ {}: {old} -> {new}{tag}", self.kind.label())
            }
        }
    }
}

/// A set of proposed changes, shown in the Pinned strip for accept / edit /
/// reject.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PinDiff {
    pub changes: Vec<PinChange>,
}

impl PinDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Apply the changes `acceptance` allows. Returns the changes that were
    /// held back, which stay pending for the user.
    pub fn apply(&self, ledger: &mut Ledger, acceptance: Acceptance) -> (usize, Vec<PinChange>) {
        let mut applied = 0usize;
        let mut held = Vec::new();
        for change in &self.changes {
            if apply_change(ledger, change, acceptance) {
                applied += 1;
            } else {
                held.push(change.clone());
            }
        }
        (applied, held)
    }
}

/// Apply one change. `false` when it was held back or had no effect.
pub fn apply_change(ledger: &mut Ledger, change: &PinChange, acceptance: Acceptance) -> bool {
    if change.provenance == Provenance::Untrusted && acceptance != Acceptance::User {
        return false;
    }
    let author = PinAuthor::User;
    match &change.op {
        PinOp::Add { text } => matches!(
            ledger.add_pin(change.kind, text, author),
            PinAdd::Added(_)
        ),
        PinOp::Remove { text } => match ledger.find_pin(change.kind, text) {
            Some(ix) => ledger.remove_pin(ix),
            None => false,
        },
        PinOp::Replace { old, new } => match ledger.find_pin(change.kind, old) {
            Some(ix) => ledger.edit_pin(ix, new),
            None => matches!(ledger.add_pin(change.kind, new, author), PinAdd::Added(_)),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinAdd {
    Added(usize),
    /// An identical pin already exists at this index.
    Duplicate(usize),
    /// Empty text.
    Rejected,
}

/// A pin as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinView {
    pub index: usize,
    pub kind: PinKind,
    pub text: String,
    /// The user wrote or accepted it.
    pub user: bool,
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    let text = normalize(text);
    if text.chars().count() <= max {
        text
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

impl Ledger {
    /// Every live pin except the per-request live state, in ledger order.
    pub fn pins(&self) -> Vec<PinView> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match &entry.kind {
                EntryKind::Pinned { kind, text }
                    if *kind != PinKind::LiveState && !text.trim().is_empty() =>
                {
                    Some(PinView {
                        index,
                        kind: *kind,
                        text: text.clone(),
                        user: entry.source == Source::User,
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn pins_of(&self, kind: PinKind) -> Vec<PinView> {
        self.pins().into_iter().filter(|pin| pin.kind == kind).collect()
    }

    /// Index of the pin of `kind` with this (whitespace-normalised) text.
    pub fn find_pin(&self, kind: PinKind, text: &str) -> Option<usize> {
        let wanted = normalize(text).to_lowercase();
        self.pins()
            .into_iter()
            .find(|pin| pin.kind == kind && normalize(&pin.text).to_lowercase() == wanted)
            .map(|pin| pin.index)
    }

    /// Append a pin. Pins are never inserted in the middle, so ledger indexes
    /// the panel holds stay valid.
    pub fn add_pin(&mut self, kind: PinKind, text: &str, author: PinAuthor) -> PinAdd {
        let text = clip(text, MAX_PIN_CHARS);
        if text.is_empty() || kind == PinKind::LiveState {
            return PinAdd::Rejected;
        }
        if let Some(ix) = self.find_pin(kind, &text) {
            return PinAdd::Duplicate(ix);
        }
        let mut entry = LedgerEntry::pinned(kind, text);
        if author == PinAuthor::User {
            entry.source = Source::User;
        }
        PinAdd::Added(self.push(entry))
    }

    /// Change a pin's text in place. Editing counts as the user's own word.
    pub fn edit_pin(&mut self, index: usize, new_text: &str) -> bool {
        let new_text = clip(new_text, MAX_PIN_CHARS);
        if new_text.is_empty() {
            return false;
        }
        match self.entries.get_mut(index) {
            Some(entry) => match &mut entry.kind {
                EntryKind::Pinned { kind, text } if *kind != PinKind::LiveState => {
                    *text = new_text;
                    entry.source = Source::User;
                    true
                }
                _ => false,
            },
            None => false,
        }
    }

    /// Remove a pin. The entry becomes an empty tombstone so indexes after it
    /// do not shift; tombstones are ignored everywhere and never saved.
    pub fn remove_pin(&mut self, index: usize) -> bool {
        match self.entries.get_mut(index).map(|entry| &mut entry.kind) {
            Some(EntryKind::Pinned { kind, text })
                if *kind != PinKind::LiveState && !text.trim().is_empty() =>
            {
                text.clear();
                true
            }
            _ => false,
        }
    }

    /// Cut the ledger back to `len` entries but keep the user's pins, which
    /// are appended wherever they were made and must survive a rollback.
    pub fn truncate_keep_pins(&mut self, len: usize) {
        if len >= self.entries.len() {
            return;
        }
        let kept: Vec<LedgerEntry> = self.entries[len..]
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.kind,
                    EntryKind::Pinned { kind, text }
                        if *kind != PinKind::LiveState && !text.trim().is_empty()
                )
            })
            .cloned()
            .collect();
        self.entries.truncate(len);
        self.entries.extend(kept);
    }

    /// Record the files a batch of tool calls changed.
    pub fn note_files_touched(&mut self, calls: &[(ToolCall, bool)], classifier: &ToolClassifier) {
        for (call, is_error) in calls {
            if *is_error || classifier.classify(&call.name) != ToolClass::Write {
                continue;
            }
            if !looks_like_file_write(&call.name) {
                continue;
            }
            for path in path_args(&call.arguments_json) {
                self.add_pin(PinKind::FileTouched, &path, PinAuthor::Engine);
            }
        }
        // Keep the newest MAX_FILES_TOUCHED.
        let files = self.pins_of(PinKind::FileTouched);
        if files.len() > MAX_FILES_TOUCHED {
            for pin in &files[..files.len() - MAX_FILES_TOUCHED] {
                self.remove_pin(pin.index);
            }
        }
    }
}

fn looks_like_file_write(tool: &str) -> bool {
    ["write", "edit", "create", "delete", "remove", "move", "rename", "replace", "patch", "save"]
        .iter()
        .any(|word| tool.contains(word))
}

/// Character budget for the pinned block, from the window. Binding pins
/// (requirements, constraints) are never elided even when over it.
pub fn pin_budget_chars(profile: &ModelProfile) -> usize {
    let window = profile.effective_context() as usize;
    (window * 4 * 15 / 100).clamp(600, 8_000)
}

/// The rendered pinned block and the ledger entries it came from. `None`
/// when there are no pins.
pub fn pinned_block(ledger: &Ledger, max_chars: usize) -> Option<(String, Vec<usize>)> {
    let pins = ledger.pins();
    if pins.is_empty() {
        return None;
    }
    let indexes: Vec<usize> = (0..ledger.entries.len())
        .filter(|ix| {
            matches!(
                &ledger.entries[*ix].kind,
                EntryKind::Pinned { kind, .. } if *kind != PinKind::LiveState
            )
        })
        .collect();

    // (kind, text) per item, oldest first inside a kind.
    let mut items: Vec<(PinKind, String)> = Vec::new();
    for kind in PinKind::DURABLE {
        for pin in pins.iter().filter(|pin| pin.kind == kind) {
            items.push((kind, normalize(&pin.text)));
        }
    }
    let mut omitted = [0usize; 5];
    let kind_slot = |kind: PinKind| PinKind::DURABLE.iter().position(|k| *k == kind).unwrap_or(0);

    let render = |items: &[(PinKind, String)], omitted: &[usize; 5]| -> String {
        let mut out = String::from(
            "## Pinned context\nThe user set or approved these. Honor them in every step.",
        );
        for (slot, kind) in PinKind::DURABLE.iter().enumerate() {
            let own: Vec<&(PinKind, String)> = items.iter().filter(|(k, _)| k == kind).collect();
            if own.is_empty() && omitted[slot] == 0 {
                continue;
            }
            out.push_str(&format!("\n\n### {}", kind.title()));
            if omitted[slot] > 0 {
                out.push_str(&format!("\n({} older omitted)", omitted[slot]));
            }
            for (_, text) in own {
                out.push_str(&format!("\n- {text}"));
            }
        }
        out
    };

    let mut text = render(&items, &omitted);
    // Over budget: drop the oldest files, then questions, then decisions.
    for victim in [PinKind::FileTouched, PinKind::OpenQuestion, PinKind::Decision] {
        while text.chars().count() > max_chars {
            let Some(pos) = items.iter().position(|(k, _)| *k == victim) else {
                break;
            };
            items.remove(pos);
            omitted[kind_slot(victim)] += 1;
            text = render(&items, &omitted);
        }
    }
    Some((text, indexes))
}

// ---------------------------------------------------------------------------
// Candidate detection

/// A sentence that reads like a requirement or constraint, offered to the user
/// as a one-click pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinCandidate {
    pub kind: PinKind,
    pub text: String,
}

const CONSTRAINT_WORDS: &[&str] = &[
    "never", "don't", "dont", "do not", "must not", "mustn't", "shouldn't", "should not",
    "avoid", "without",
];
const REQUIREMENT_WORDS: &[&str] = &[
    "must", "always", "only", "make sure", "ensure", "need to", "has to", "have to",
];

/// Whole-word / whole-phrase test, case-insensitive.
fn has_phrase(haystack: &str, phrase: &str) -> bool {
    let hay = haystack.to_lowercase().replace('\u{2019}', "'");
    let mut from = 0usize;
    while let Some(pos) = hay[from..].find(phrase) {
        let start = from + pos;
        let end = start + phrase.len();
        let before_ok = hay[..start]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_alphanumeric());
        let after_ok = hay[end..]
            .chars()
            .next()
            .map_or(true, |c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = start + phrase.chars().next().map_or(1, char::len_utf8);
        if from >= hay.len() {
            break;
        }
    }
    false
}

/// Sentences of `text` containing a trigger word. At most `limit`.
pub fn detect_pin_candidates(text: &str, limit: usize) -> Vec<PinCandidate> {
    let mut out: Vec<PinCandidate> = Vec::new();
    let mut sentence = String::new();
    let mut sentences: Vec<String> = Vec::new();
    for c in text.chars() {
        if matches!(c, '.' | '!' | '?' | '\n' | ';') {
            if !sentence.trim().is_empty() {
                sentences.push(std::mem::take(&mut sentence));
            }
            sentence.clear();
        } else {
            sentence.push(c);
        }
    }
    if !sentence.trim().is_empty() {
        sentences.push(sentence);
    }
    for sentence in sentences {
        let clipped = clip(&sentence, MAX_PIN_CHARS);
        if clipped.chars().count() < 8 {
            continue;
        }
        let kind = if CONSTRAINT_WORDS.iter().any(|w| has_phrase(&clipped, w)) {
            PinKind::Constraint
        } else if REQUIREMENT_WORDS.iter().any(|w| has_phrase(&clipped, w)) {
            PinKind::Requirement
        } else {
            continue;
        };
        if out.iter().any(|c| c.text == clipped) {
            continue;
        }
        out.push(PinCandidate {
            kind,
            text: clipped,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Re-anchoring

/// Task lines of the live state that are not finished.
fn open_task_lines(live_state: &str) -> Vec<String> {
    live_state
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('[')?;
            let (status, title) = rest.split_once(']')?;
            let status = status.trim().to_lowercase();
            if matches!(
                status.as_str(),
                "done" | "completed" | "complete" | "cancelled" | "canceled" | "skipped"
            ) {
                return None;
            }
            Some(format!("{} ({status})", title.trim()))
        })
        .collect()
}

/// A short reminder of the current goal, the binding pins and the open tasks.
/// `None` when there is nothing to remind about.
pub fn build_reminder(ledger: &Ledger) -> Option<String> {
    let goal = ledger.entries.iter().rev().find_map(|entry| match &entry.kind {
        EntryKind::UserTurn { text } if !text.trim().is_empty() => Some(clip(text, 240)),
        _ => None,
    })?;
    let mut out = format!("Reminder: current goal: {goal}");
    let binding: Vec<String> = ledger
        .pins()
        .into_iter()
        .filter(|pin| pin.kind.is_binding())
        .map(|pin| clip(&pin.text, 120))
        .take(6)
        .collect();
    if !binding.is_empty() {
        out.push_str(&format!("\nPinned: {}", binding.join(" | ")));
    }
    let live: Vec<&str> = ledger
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Pinned { kind: PinKind::LiveState, text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let tasks: Vec<String> = live.iter().flat_map(|text| open_task_lines(text)).take(6).collect();
    if !tasks.is_empty() {
        out.push_str(&format!("\nOpen tasks: {}", tasks.join("; ")));
    }
    Some(out)
}
