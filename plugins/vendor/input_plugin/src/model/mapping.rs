//! The input action mapping asset: which keys drive which actions.
//!
//! A mapping is a list of *entries*, each binding one key to one action, sorted
//! into named *categories* ("Movement", "Combat", "Menus") purely for the
//! person editing it. Categories mean nothing at runtime. A mapping also has a
//! priority: when several are active, a higher one sees input first and its
//! actions can consume it.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::behavior::{Modifier, Swizzle, Tolerant, Trigger};
use super::id::{ActionRef, AssetId};
use super::{parse_object, Asset};

/// File extension of mapping assets.
pub const MAPPING_EXTENSION: &str = "inputmap";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// Unique within the mapping and never reused, so a selection or an
    /// issue can point at it while the list changes.
    pub id: u32,
    /// A key id from the catalog (`Keyboard.KeyW`). Empty: not chosen yet.
    pub key: String,
    /// The action it drives. `None`: not chosen yet.
    pub action: Option<ActionRef>,
    pub enabled: bool,
    /// Triggers for this binding only; they run before the action's own.
    pub triggers: Vec<Tolerant<Trigger>>,
    /// Modifiers for this binding only; they run before the action's own.
    pub modifiers: Vec<Tolerant<Modifier>>,
    pub note: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            id: 0,
            key: String::new(),
            action: None,
            enabled: true,
            triggers: Vec::new(),
            modifiers: Vec::new(),
            note: String::new(),
            extra: Map::new(),
        }
    }
}

impl Entry {
    pub fn action_id(&self) -> Option<&AssetId> {
        self.action.as_ref().map(|a| &a.id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Category {
    pub id: u32,
    pub name: String,
    pub entries: Vec<Entry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Category {
    fn default() -> Self {
        Self { id: 0, name: String::new(), entries: Vec::new(), extra: Map::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MappingAsset {
    #[serde(default = "AssetId::unassigned")]
    pub id: AssetId,
    pub description: String,
    /// Higher sees input first.
    pub priority: i32,
    pub categories: Vec<Category>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for MappingAsset {
    fn default() -> Self {
        Self {
            id: AssetId::new(),
            description: String::new(),
            priority: 0,
            categories: vec![Category { id: 1, name: "General".to_string(), entries: Vec::new(), extra: Map::new() }],
            extra: Map::new(),
        }
    }
}

/// Where an entry sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryPos {
    pub category: usize,
    pub index: usize,
}

impl MappingAsset {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Reading ────────────────────────────────────────────────────────────

    pub fn entries(&self) -> impl Iterator<Item = (&Category, &Entry)> {
        self.categories.iter().flat_map(|c| c.entries.iter().map(move |e| (c, e)))
    }

    pub fn entry_count(&self) -> usize {
        self.categories.iter().map(|c| c.entries.len()).sum()
    }

    pub fn entry(&self, id: u32) -> Option<&Entry> {
        self.entries().map(|(_, e)| e).find(|e| e.id == id)
    }

    pub fn entry_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.categories.iter_mut().flat_map(|c| c.entries.iter_mut()).find(|e| e.id == id)
    }

    pub fn find(&self, id: u32) -> Option<EntryPos> {
        self.categories.iter().enumerate().find_map(|(category, c)| {
            c.entries.iter().position(|e| e.id == id).map(|index| EntryPos { category, index })
        })
    }

    pub fn category(&self, id: u32) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    pub fn category_of(&self, entry: u32) -> Option<&Category> {
        self.categories.iter().find(|c| c.entries.iter().any(|e| e.id == entry))
    }

    /// Every action this mapping binds.
    pub fn used_actions(&self) -> HashSet<AssetId> {
        self.entries().filter_map(|(_, e)| e.action_id().cloned()).collect()
    }

    fn next_entry_id(&self) -> u32 {
        self.entries().map(|(_, e)| e.id).max().unwrap_or(0) + 1
    }

    fn next_category_id(&self) -> u32 {
        self.categories.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    /// `base`, or `base 2`, `base 3`... whichever no category has yet.
    pub fn unique_category_name(&self, base: &str) -> String {
        let taken = |name: &str| self.categories.iter().any(|c| c.name.eq_ignore_ascii_case(name));
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_default()
    }

    // ── Categories ─────────────────────────────────────────────────────────

    pub fn add_category(&mut self, name: &str) -> u32 {
        let id = self.next_category_id();
        let name = self.unique_category_name(if name.trim().is_empty() { "Category" } else { name.trim() });
        self.categories.push(Category { id, name, entries: Vec::new(), extra: Map::new() });
        id
    }

    pub fn rename_category(&mut self, id: u32, name: &str) {
        if let Some(c) = self.categories.iter_mut().find(|c| c.id == id) {
            c.name = name.trim().to_string();
        }
    }

    /// Remove a category and its entries. A mapping always keeps one
    /// category, so the last one is emptied instead.
    pub fn remove_category(&mut self, id: u32) {
        if self.categories.len() <= 1 {
            if let Some(c) = self.categories.iter_mut().find(|c| c.id == id) {
                c.entries.clear();
            }
            return;
        }
        self.categories.retain(|c| c.id != id);
    }

    /// Move a category so it ends up at `to` in the list.
    pub fn move_category(&mut self, id: u32, to: usize) {
        let Some(from) = self.categories.iter().position(|c| c.id == id) else { return };
        let category = self.categories.remove(from);
        self.categories.insert(to.min(self.categories.len()), category);
    }

    // ── Entries ────────────────────────────────────────────────────────────

    /// Add an entry at the end of `category`. `None` if there is no such
    /// category.
    pub fn add_entry(&mut self, category: u32, key: &str, action: Option<ActionRef>) -> Option<u32> {
        let id = self.next_entry_id();
        let c = self.categories.iter_mut().find(|c| c.id == category)?;
        c.entries.push(Entry { id, key: key.to_string(), action, ..Entry::default() });
        Some(id)
    }

    pub fn remove_entry(&mut self, id: u32) {
        for c in &mut self.categories {
            c.entries.retain(|e| e.id != id);
        }
    }

    /// Copy an entry to just after itself; returns the copy's id.
    pub fn duplicate_entry(&mut self, id: u32) -> Option<u32> {
        let pos = self.find(id)?;
        let new_id = self.next_entry_id();
        let mut copy = self.categories[pos.category].entries[pos.index].clone();
        copy.id = new_id;
        self.categories[pos.category].entries.insert(pos.index + 1, copy);
        Some(new_id)
    }

    /// Move an entry into `category` so it ends up at `index` there.
    pub fn move_entry(&mut self, id: u32, category: u32, index: usize) {
        let Some(to) = self.categories.iter().position(|c| c.id == category) else { return };
        let Some(from) = self.find(id) else { return };
        let entry = self.categories[from.category].entries.remove(from.index);
        let list = &mut self.categories[to].entries;
        list.insert(index.min(list.len()), entry);
    }

    pub fn set_key(&mut self, id: u32, key: &str) {
        if let Some(e) = self.entry_mut(id) {
            e.key = key.to_string();
        }
    }

    pub fn set_action(&mut self, id: u32, action: Option<ActionRef>) {
        if let Some(e) = self.entry_mut(id) {
            e.action = action;
        }
    }

    /// Point every entry that uses `action` at its current file, so the
    /// hints stay right after the action moved. Returns how many changed.
    pub fn refresh_hints(&mut self, action: &AssetId, hint: &str) -> usize {
        let mut changed = 0;
        for c in &mut self.categories {
            for e in &mut c.entries {
                if let Some(a) = e.action.as_mut().filter(|a| &a.id == action && a.hint != hint) {
                    a.hint = hint.to_string();
                    changed += 1;
                }
            }
        }
        changed
    }

    // ── Presets ────────────────────────────────────────────────────────────

    /// Bind four keys to a 2D action as a direction pad: `up` and `down` move
    /// along Y, `left` and `right` along X. Returns the new entries' ids.
    pub fn add_direction_keys(&mut self, category: u32, action: &ActionRef, up: &str, down: &str, left: &str, right: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        let specs: [(&str, Vec<Modifier>); 4] = [
            (up, vec![Modifier::Swizzle { order: Swizzle::Yxz }]),
            (down, vec![Modifier::Swizzle { order: Swizzle::Yxz }, Modifier::Negate { x: false, y: true, z: false }]),
            (left, vec![Modifier::Negate { x: true, y: false, z: false }]),
            (right, vec![]),
        ];
        for (key, modifiers) in specs {
            if let Some(id) = self.add_entry(category, key, Some(action.clone())) {
                if let Some(e) = self.entry_mut(id) {
                    e.modifiers = modifiers.into_iter().map(Tolerant::Known).collect();
                }
                ids.push(id);
            }
        }
        ids
    }

    /// Bind two keys to a 1D action: `positive` pushes it up, `negative` down.
    pub fn add_axis_keys(&mut self, category: u32, action: &ActionRef, positive: &str, negative: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        if let Some(id) = self.add_entry(category, positive, Some(action.clone())) {
            ids.push(id);
        }
        if let Some(id) = self.add_entry(category, negative, Some(action.clone())) {
            if let Some(e) = self.entry_mut(id) {
                e.modifiers = vec![Tolerant::Known(Modifier::Negate { x: true, y: false, z: false })];
            }
            ids.push(id);
        }
        ids
    }
}

impl Asset for MappingAsset {
    const KIND: &'static str = "input mapping";

    fn new_default() -> Self {
        Self::new()
    }

    fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        let value = parse_object(text)?;
        let mut notes = Vec::new();
        let mut mapping: MappingAsset = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if !mapping.id.is_assigned() {
            notes.push("This mapping had no id, so it was given one.".to_string());
        }
        if mapping.categories.is_empty() {
            mapping.categories.push(Category { id: 1, name: "General".to_string(), entries: Vec::new(), extra: Map::new() });
        }
        // Ids are what selections and issues point at; a hand-edited file
        // may repeat or omit them.
        let mut repaired = false;
        let mut seen_categories = HashSet::new();
        let mut next_category = mapping.categories.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        for c in &mut mapping.categories {
            if c.id == 0 || !seen_categories.insert(c.id) {
                c.id = next_category;
                next_category += 1;
                seen_categories.insert(c.id);
                repaired = true;
            }
        }
        let mut seen = HashSet::new();
        let mut next = mapping.entries().map(|(_, e)| e.id).max().unwrap_or(0) + 1;
        for c in &mut mapping.categories {
            for e in &mut c.entries {
                if e.id == 0 || !seen.insert(e.id) {
                    e.id = next;
                    next += 1;
                    seen.insert(e.id);
                    repaired = true;
                }
            }
        }
        if repaired {
            notes.push("Some entries or categories shared an id (or had none) and were renumbered.".to_string());
        }
        let unknown: usize = mapping
            .entries()
            .map(|(_, e)| e.triggers.iter().filter(|t| t.known().is_none()).count() + e.modifiers.iter().filter(|m| m.known().is_none()).count())
            .sum();
        if unknown > 0 {
            notes.push(format!("{unknown} trigger or modifier made by a newer version is kept but can't be edited here."));
        }
        Ok((mapping, notes))
    }

    fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    fn stamp(&mut self) -> bool {
        if self.id.is_assigned() {
            return false;
        }
        self.id = AssetId::new();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jump() -> ActionRef {
        ActionRef::new(AssetId::new(), "input/Jump.inputaction")
    }

    #[test]
    fn a_new_mapping_has_one_category_and_adds_entries_with_fresh_ids() {
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let a = m.add_entry(cat, "Keyboard.Space", Some(jump())).unwrap();
        let b = m.add_entry(cat, "Keyboard.KeyE", None).unwrap();
        assert_ne!(a, b);
        assert_eq!(m.entry_count(), 2);
        m.remove_entry(a);
        let c = m.add_entry(cat, "Keyboard.KeyQ", None).unwrap();
        assert!(c > b, "ids aren't reused after a removal");
        assert!(m.add_entry(999, "x", None).is_none());
    }

    #[test]
    fn entries_move_between_categories_and_keep_their_place() {
        let mut m = MappingAsset::new();
        let first = m.categories[0].id;
        let second = m.add_category("Combat");
        let a = m.add_entry(first, "Keyboard.KeyA", None).unwrap();
        let b = m.add_entry(first, "Keyboard.KeyB", None).unwrap();
        let c = m.add_entry(second, "Keyboard.KeyC", None).unwrap();
        m.move_entry(a, second, 0);
        assert_eq!(m.category(first).unwrap().entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b]);
        assert_eq!(m.category(second).unwrap().entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![a, c]);
        m.move_entry(a, second, 99);
        assert_eq!(m.category(second).unwrap().entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, a]);
    }

    #[test]
    fn categories_are_named_uniquely_and_the_last_one_cant_be_removed() {
        let mut m = MappingAsset::new();
        let a = m.categories[0].id;
        let b = m.add_category("General");
        assert_eq!(m.category(b).unwrap().name, "General 2");
        m.add_entry(a, "Keyboard.KeyA", None);
        m.remove_category(b);
        assert_eq!(m.categories.len(), 1);
        m.remove_category(a);
        assert_eq!(m.categories.len(), 1, "a mapping keeps a category");
        assert_eq!(m.entry_count(), 0, "but the last one is emptied");
        let c = m.add_category("Third");
        m.move_category(c, 0);
        assert_eq!(m.categories[0].id, c);
    }

    #[test]
    fn duplicating_an_entry_copies_its_settings_beside_it() {
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let a = m.add_entry(cat, "Keyboard.KeyA", Some(jump())).unwrap();
        m.entry_mut(a).unwrap().note = "strafe".into();
        let copy = m.duplicate_entry(a).unwrap();
        assert_eq!(m.find(copy).unwrap().index, m.find(a).unwrap().index + 1);
        assert_eq!(m.entry(copy).unwrap().note, "strafe");
        assert_eq!(m.entry(copy).unwrap().action, m.entry(a).unwrap().action);
    }

    #[test]
    fn direction_keys_make_four_entries_that_point_the_right_way() {
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let ids = m.add_direction_keys(cat, &jump(), "Keyboard.KeyW", "Keyboard.KeyS", "Keyboard.KeyA", "Keyboard.KeyD");
        assert_eq!(ids.len(), 4);
        let w = m.entry(ids[0]).unwrap();
        assert_eq!(w.modifiers, vec![Tolerant::Known(Modifier::Swizzle { order: Swizzle::Yxz })]);
        assert_eq!(m.entry(ids[3]).unwrap().modifiers.len(), 0, "D is plain +X");
        let axis = m.add_axis_keys(cat, &jump(), "Keyboard.KeyE", "Keyboard.KeyQ");
        assert_eq!(m.entry(axis[1]).unwrap().modifiers.len(), 1);
    }

    #[test]
    fn hints_are_refreshed_only_where_they_changed() {
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let a = jump();
        m.add_entry(cat, "Keyboard.Space", Some(a.clone()));
        m.add_entry(cat, "Gamepad.FaceBottom", Some(a.clone()));
        m.add_entry(cat, "Keyboard.KeyE", Some(jump()));
        assert_eq!(m.refresh_hints(&a.id, "moved/Jump.inputaction"), 2);
        assert_eq!(m.refresh_hints(&a.id, "moved/Jump.inputaction"), 0);
    }

    #[test]
    fn loading_repairs_repeated_ids_and_keeps_unknown_fields() {
        let id = AssetId::new();
        let text = format!(
            r#"{{"id":"{id}","future":true,"categories":[
                {{"id":1,"name":"A","entries":[{{"id":1,"key":"Keyboard.KeyA"}},{{"id":1,"key":"Keyboard.KeyB","mystery":5}}]}},
                {{"id":1,"name":"B","entries":[{{"key":"Keyboard.KeyC"}}]}}]}}"#
        );
        let (m, notes) = MappingAsset::parse(&text).unwrap();
        let ids: Vec<u32> = m.entries().map(|(_, e)| e.id).collect();
        let unique: HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 3);
        assert_ne!(m.categories[0].id, m.categories[1].id);
        assert!(notes.iter().any(|n| n.contains("renumbered")));
        let json: Value = serde_json::from_str(&m.to_text()).unwrap();
        assert_eq!(json["future"], Value::Bool(true));
        assert_eq!(json["categories"][0]["entries"][1]["mystery"], 5);
    }

    #[test]
    fn a_mapping_with_no_categories_gets_one() {
        let (m, _) = MappingAsset::parse(r#"{"categories": []}"#).unwrap();
        assert_eq!(m.categories.len(), 1);
    }

    #[test]
    fn an_entry_defaults_to_enabled_when_the_file_omits_it() {
        let (m, _) = MappingAsset::parse(r#"{"categories":[{"id":1,"name":"A","entries":[{"id":1,"key":"Keyboard.KeyA"}]}]}"#).unwrap();
        assert!(m.entry(1).unwrap().enabled);
    }
}
