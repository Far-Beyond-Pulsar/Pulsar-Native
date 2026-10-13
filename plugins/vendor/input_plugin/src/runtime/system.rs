//! Evaluating mappings against the devices, frame by frame.
//!
//! An [`InputSystem`] holds the *active mappings* (contexts). Each frame it
//! reads the keys from [`RawInput`], runs them through every entry's
//! modifiers and triggers and every action's, and reports what happened to
//! each action as [`ActionEvent`]s.
//!
//! # Per action, per frame
//!
//! 1. Each enabled entry reads its key (zero if a higher-priority action
//!    consumed it this frame) and runs it through its own modifiers.
//! 2. The entries' values are combined, the largest magnitude per component
//!    winning, so holding W and the stick together never doubles.
//! 3. The action's modifiers shape the result, and the action's value type
//!    cuts it to size.
//! 4. Triggers decide the action's state. With no triggers anywhere it is
//!    *Triggered* while actuated. Otherwise it is the strongest state any
//!    trigger (the action's, or an entry's own) reports. A chord gates the
//!    result: unless the chorded action is Triggered, nothing fires.
//! 5. The state change is reported: *Started* when it leaves None, *Ongoing*
//!    and *Triggered* each frame they hold, *Completed* when a Triggered
//!    action stops and *Canceled* when an Ongoing one stops before firing.
//! 6. If the action consumes input and is active, its keys are hidden from
//!    lower-priority mappings for the rest of the frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::keys::catalog;
use crate::model::behavior::{magnitude, Modifier, Trigger, Value3, ValueType};
use crate::model::{ActionAsset, AssetId, MappingAsset};

use super::modify::Chain;
use super::raw::RawInput;
use super::trigger::{TriggerRt, TriggerState};

/// What happened to an action this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    /// It left idle: the first frame of an activation.
    Started,
    /// Building toward firing.
    Ongoing,
    /// Firing.
    Triggered,
    /// A firing action stopped.
    Completed,
    /// An action that was building stopped before it fired.
    Canceled,
}

impl Phase {
    pub const ALL: [Phase; 5] = [Phase::Started, Phase::Ongoing, Phase::Triggered, Phase::Completed, Phase::Canceled];

    /// The word used in event names (`Jump.triggered`).
    pub fn word(self) -> &'static str {
        match self {
            Phase::Started => "started",
            Phase::Ongoing => "ongoing",
            Phase::Triggered => "triggered",
            Phase::Completed => "completed",
            Phase::Canceled => "canceled",
        }
    }
}

/// One thing that happened to one action.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionEvent {
    /// Which active mapping reported it.
    pub context: ContextHandle,
    pub mapping: AssetId,
    pub action: AssetId,
    /// The event name (`Jump`).
    pub name: Arc<str>,
    /// The object the mapping is attached to, if any.
    pub owner: Option<u64>,
    pub phase: Phase,
    pub value_type: ValueType,
    pub value: Value3,
    /// Seconds since the action left idle.
    pub elapsed: f32,
    /// Seconds it has been firing in this activation.
    pub triggered_for: f32,
}

/// Names an active mapping, to remove it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextHandle(pub u64);

/// Where the system gets the actions a mapping refers to.
pub trait ActionLookup {
    /// The action and the name its events go by.
    fn action(&self, id: &AssetId) -> Option<(Arc<ActionAsset>, String)>;
}

impl ActionLookup for crate::library::Library {
    fn action(&self, id: &AssetId) -> Option<(Arc<ActionAsset>, String)> {
        self.action_entry(id).map(|e| (e.asset.clone(), e.event_name()))
    }
}

impl ActionLookup for HashMap<AssetId, (Arc<ActionAsset>, String)> {
    fn action(&self, id: &AssetId) -> Option<(Arc<ActionAsset>, String)> {
        self.get(id).cloned()
    }
}

// ── Compiled form ───────────────────────────────────────────────────────────

struct EntryRt {
    key: usize,
    mods: Chain,
    triggers: Vec<TriggerRt>,
    /// Chorded actions (indices into the context's actions) that must fire
    /// for this entry to count; `None` is one that can't exist here.
    gates: Vec<Option<usize>>,
}

struct ActionRt {
    id: AssetId,
    name: Arc<str>,
    value_type: ValueType,
    threshold: f32,
    consume: bool,
    when_paused: bool,
    mods: Chain,
    triggers: Vec<TriggerRt>,
    gates: Vec<Option<usize>>,
    entries: Vec<EntryRt>,
    // State between frames.
    state: TriggerState,
    value: Value3,
    elapsed: f32,
    triggered_for: f32,
}

struct Active {
    handle: ContextHandle,
    mapping: AssetId,
    priority: i32,
    owner: Option<u64>,
    enabled: bool,
    actions: Vec<ActionRt>,
    /// Evaluation order: chorded actions after the ones they depend on.
    order: Vec<usize>,
}

fn compile(mapping: &MappingAsset, lookup: &dyn ActionLookup, handle: ContextHandle, priority: i32, owner: Option<u64>) -> (Active, Vec<String>) {
    let keys = catalog();
    let mut problems = Vec::new();
    let mut index_of: HashMap<AssetId, usize> = HashMap::new();
    let mut actions: Vec<ActionRt> = Vec::new();
    let mut gate_ids: Vec<Vec<AssetId>> = Vec::new();

    let mut ensure = |id: &AssetId, actions: &mut Vec<ActionRt>, problems: &mut Vec<String>| -> Option<usize> {
        if let Some(&i) = index_of.get(id) {
            return Some(i);
        }
        let Some((asset, name)) = lookup.action(id) else {
            problems.push(format!("action {id} can't be found"));
            return None;
        };
        let mut gates = Vec::new();
        let mut triggers = Vec::new();
        for t in asset.known_triggers() {
            match t {
                Trigger::Chorded { action } => gates.push(action.id.clone()),
                other => triggers.extend(TriggerRt::new(other.clone())),
            }
        }
        let i = actions.len();
        index_of.insert(id.clone(), i);
        actions.push(ActionRt {
            id: id.clone(),
            name: name.into(),
            value_type: asset.value_type,
            threshold: asset.actuation_threshold,
            consume: asset.consume_input,
            when_paused: asset.trigger_when_paused,
            mods: Chain::new(asset.known_modifiers().cloned().collect::<Vec<Modifier>>()),
            triggers,
            gates: Vec::new(),
            entries: Vec::new(),
            state: TriggerState::None,
            value: [0.0; 3],
            elapsed: 0.0,
            triggered_for: 0.0,
        });
        gate_ids.push(gates);
        Some(i)
    };

    // First pass: every action the mapping binds.
    let mut entry_gates: Vec<(usize, usize, Vec<AssetId>)> = Vec::new();
    for (_, entry) in mapping.entries() {
        if !entry.enabled {
            continue;
        }
        let Some(reference) = &entry.action else { continue };
        let Some(key) = keys.index_of(&entry.key) else {
            problems.push(format!("`{}` isn't a known key", entry.key));
            continue;
        };
        let Some(action) = ensure(&reference.id, &mut actions, &mut problems) else { continue };
        let mut gates = Vec::new();
        let mut triggers = Vec::new();
        for t in entry.triggers.iter().filter_map(|t| t.known()) {
            match t {
                Trigger::Chorded { action } => gates.push(action.id.clone()),
                other => triggers.extend(TriggerRt::new(other.clone())),
            }
        }
        let slot = actions[action].entries.len();
        actions[action].entries.push(EntryRt {
            key,
            mods: Chain::new(entry.modifiers.iter().filter_map(|m| m.known()).cloned().collect()),
            triggers,
            gates: Vec::new(),
        });
        entry_gates.push((action, slot, gates));
    }

    // Second pass: resolve chords against the actions this mapping has.
    let resolve = |ids: &[AssetId], problems: &mut Vec<String>| -> Vec<Option<usize>> {
        ids.iter()
            .map(|id| {
                let found = index_of.get(id).copied();
                if found.is_none() {
                    problems.push(format!("a chord needs action {id}, which this mapping doesn't bind"));
                }
                found
            })
            .collect()
    };
    for (i, gates) in gate_ids.iter().enumerate() {
        actions[i].gates = resolve(gates, &mut problems);
    }
    for (action, slot, gates) in entry_gates {
        actions[action].entries[slot].gates = resolve(&gates, &mut problems);
    }

    let order = evaluation_order(&actions);
    (Active { handle, mapping: mapping.id.clone(), priority, owner, enabled: true, actions, order }, problems)
}

/// Actions in an order where each comes after the ones it is chorded with.
/// A cycle (which validation reports) is broken wherever it is met.
fn evaluation_order(actions: &[ActionRt]) -> Vec<usize> {
    fn visit(i: usize, actions: &[ActionRt], seen: &mut HashSet<usize>, out: &mut Vec<usize>) {
        if !seen.insert(i) {
            return;
        }
        let deps = actions[i].gates.iter().flatten().chain(actions[i].entries.iter().flat_map(|e| e.gates.iter().flatten()));
        for &dep in deps {
            visit(dep, actions, seen, out);
        }
        out.push(i);
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in 0..actions.len() {
        visit(i, actions, &mut seen, &mut out);
    }
    out
}

// ── The system ──────────────────────────────────────────────────────────────

/// The active mappings and what they are doing.
#[derive(Default)]
pub struct InputSystem {
    contexts: Vec<Active>,
    next_handle: u64,
    /// Events from removing a mapping, delivered with the next update.
    pending: Vec<ActionEvent>,
}

impl InputSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start using `mapping`. A higher `priority` sees input first. Returns
    /// the handle to remove it with, and anything about the mapping that
    /// can't work here (an action that can't be found, a chord that can't be
    /// satisfied).
    pub fn add(&mut self, mapping: &MappingAsset, lookup: &dyn ActionLookup, priority: i32, owner: Option<u64>) -> (ContextHandle, Vec<String>) {
        self.next_handle += 1;
        let handle = ContextHandle(self.next_handle);
        let (active, problems) = compile(mapping, lookup, handle, priority, owner);
        self.contexts.push(active);
        // Stable: equal priorities keep the order they were added in.
        self.contexts.sort_by_key(|c| std::cmp::Reverse(c.priority));
        (handle, problems)
    }

    /// Stop using a mapping. Its actions that were active end: a firing one
    /// completes, a building one is canceled, and those events come with the
    /// next update.
    pub fn remove(&mut self, handle: ContextHandle) {
        if let Some(at) = self.contexts.iter().position(|c| c.handle == handle) {
            let mut context = self.contexts.remove(at);
            end_all(&mut context, &mut self.pending);
        }
    }

    /// Remove every mapping attached to `owner`.
    pub fn remove_owned_by(&mut self, owner: u64) {
        let handles: Vec<_> = self.contexts.iter().filter(|c| c.owner == Some(owner)).map(|c| c.handle).collect();
        for handle in handles {
            self.remove(handle);
        }
    }

    pub fn remove_all(&mut self) {
        for mut context in self.contexts.drain(..) {
            end_all(&mut context, &mut self.pending);
        }
    }

    /// Turn a mapping on or off without losing its place. Turning it off
    /// ends what its actions were doing.
    pub fn set_enabled(&mut self, handle: ContextHandle, enabled: bool) {
        if let Some(context) = self.contexts.iter_mut().find(|c| c.handle == handle) {
            if context.enabled && !enabled {
                end_all(context, &mut self.pending);
            }
            context.enabled = enabled;
        }
    }

    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    pub fn contains(&self, handle: ContextHandle) -> bool {
        self.contexts.iter().any(|c| c.handle == handle)
    }

    /// The mapping ids now active, highest priority first.
    pub fn active_mappings(&self) -> Vec<AssetId> {
        self.contexts.iter().map(|c| c.mapping.clone()).collect()
    }

    /// What `action` reads in the highest-priority mapping that has it.
    pub fn value_of(&self, action: &AssetId) -> Option<(TriggerState, Value3)> {
        self.contexts.iter().flat_map(|c| &c.actions).find(|a| &a.id == action).map(|a| (a.state, a.value))
    }

    /// Evaluate one frame. `raw` must have had [`RawInput::derive`] called;
    /// the caller ends the frame on it afterwards. `paused` silences actions
    /// that don't work while the game is paused.
    pub fn update(&mut self, raw: &RawInput, dt: f32, paused: bool) -> Vec<ActionEvent> {
        let mut events = std::mem::take(&mut self.pending);
        let mut consumed: HashSet<usize> = HashSet::new();
        for context in &mut self.contexts {
            if !context.enabled {
                continue;
            }
            // The state each action had last frame, for chords on actions
            // that come later in the order.
            let before: Vec<TriggerState> = context.actions.iter().map(|a| a.state).collect();
            let mut now: Vec<Option<TriggerState>> = vec![None; context.actions.len()];
            for &i in &context.order {
                let gate_state = |gate: &Option<usize>, now: &[Option<TriggerState>]| match gate {
                    Some(g) => now[*g].unwrap_or(before[*g]) == TriggerState::Triggered,
                    None => false,
                };
                let open: bool = context.actions[i].gates.iter().all(|g| gate_state(g, &now));
                let entry_open: Vec<bool> = context.actions[i].entries.iter().map(|e| e.gates.iter().all(|g| gate_state(g, &now))).collect();
                let action = &mut context.actions[i];
                let silenced = paused && !action.when_paused;
                let (state, value, used) = evaluate(action, raw, dt, &consumed, silenced, open, &entry_open);
                now[i] = Some(state);
                if action.consume && state != TriggerState::None {
                    consumed.extend(used);
                }
                transition(action, context.handle, &context.mapping, context.owner, state, value, dt, &mut events);
            }
        }
        events
    }
}

/// An action's state and value this frame, and the keys it used that are
/// actuated (for consumption).
fn evaluate(
    action: &mut ActionRt,
    raw: &RawInput,
    dt: f32,
    consumed: &HashSet<usize>,
    silenced: bool,
    open: bool,
    entry_open: &[bool],
) -> (TriggerState, Value3, Vec<usize>) {
    let threshold = action.threshold;
    let actuated = |v: Value3| magnitude(v) >= threshold && magnitude(v) > 0.0;

    let mut combined = [0.0f32; 3];
    let mut used = Vec::new();
    let mut entry_states = Vec::new();
    let mut any_explicit = !action.triggers.is_empty();
    for (slot, entry) in action.entries.iter_mut().enumerate() {
        any_explicit |= !entry.triggers.is_empty();
        let blocked = silenced || consumed.contains(&entry.key) || !entry_open.get(slot).copied().unwrap_or(true);
        let raw_value = if blocked { [0.0; 3] } else { raw.value(entry.key) };
        let value = entry.mods.apply(raw_value, dt);
        if actuated(value) {
            used.push(entry.key);
        }
        for t in &mut entry.triggers {
            entry_states.push(t.update(actuated(value), dt));
        }
        for c in 0..3 {
            if value[c].abs() > combined[c].abs() {
                combined[c] = value[c];
            }
        }
    }
    let shaped = action.mods.apply(combined, dt);
    let value = action.value_type.fit(shaped, threshold);
    let is_actuated = actuated(value);

    let mut state = if any_explicit {
        let mut strongest = TriggerState::None;
        for t in &mut action.triggers {
            strongest = strongest.max(t.update(is_actuated, dt));
        }
        for s in entry_states {
            strongest = strongest.max(s);
        }
        strongest
    } else if is_actuated {
        TriggerState::Triggered
    } else {
        TriggerState::None
    };
    if !open {
        state = TriggerState::None;
    }
    (state, value, used)
}

#[allow(clippy::too_many_arguments)]
fn transition(
    action: &mut ActionRt,
    context: ContextHandle,
    mapping: &AssetId,
    owner: Option<u64>,
    state: TriggerState,
    value: Value3,
    dt: f32,
    out: &mut Vec<ActionEvent>,
) {
    use TriggerState::*;
    let before = action.state;
    action.state = state;
    action.value = value;
    if state != None {
        action.elapsed += if before == None { 0.0 } else { dt };
    } else {
        action.elapsed = 0.0;
    }
    if state == Triggered {
        action.triggered_for += if before == Triggered { dt } else { 0.0 };
    } else if state == None {
        action.triggered_for = 0.0;
    }
    let phases: &[Phase] = match (before, state) {
        (None, None) => &[],
        (None, Ongoing) => &[Phase::Started, Phase::Ongoing],
        (None, Triggered) => &[Phase::Started, Phase::Triggered],
        (Ongoing, Ongoing) | (Triggered, Ongoing) => &[Phase::Ongoing],
        (Ongoing, Triggered) | (Triggered, Triggered) => &[Phase::Triggered],
        (Ongoing, None) => &[Phase::Canceled],
        (Triggered, None) => &[Phase::Completed],
    };
    // A completion that follows a firing frame leaves the triggered time
    // visible for the handler.
    for &phase in phases {
        out.push(ActionEvent {
            context,
            mapping: mapping.clone(),
            action: action.id.clone(),
            name: action.name.clone(),
            owner,
            phase,
            value_type: action.value_type,
            value,
            elapsed: action.elapsed,
            triggered_for: action.triggered_for,
        });
    }
}

/// Finish whatever `context`'s actions were doing.
fn end_all(context: &mut Active, out: &mut Vec<ActionEvent>) {
    for action in &mut context.actions {
        let phase = match action.state {
            TriggerState::None => continue,
            TriggerState::Ongoing => Phase::Canceled,
            TriggerState::Triggered => Phase::Completed,
        };
        out.push(ActionEvent {
            context: context.handle,
            mapping: context.mapping.clone(),
            action: action.id.clone(),
            name: action.name.clone(),
            owner: context.owner,
            phase,
            value_type: action.value_type,
            value: [0.0; 3],
            elapsed: action.elapsed,
            triggered_for: action.triggered_for,
        });
        action.state = TriggerState::None;
        action.elapsed = 0.0;
        action.triggered_for = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::{Swizzle, Tolerant};
    use crate::model::ActionRef;

    type Lookup = HashMap<AssetId, (Arc<ActionAsset>, String)>;

    struct Rig {
        lookup: Lookup,
        mapping: MappingAsset,
        category: u32,
        system: InputSystem,
        raw: RawInput,
    }

    impl Rig {
        fn new() -> Self {
            let mapping = MappingAsset::new();
            let category = mapping.categories[0].id;
            Self { lookup: HashMap::new(), mapping, category, system: InputSystem::new(), raw: RawInput::new() }
        }

        fn action(&mut self, name: &str, build: impl FnOnce(&mut ActionAsset)) -> ActionRef {
            let mut asset = ActionAsset::new();
            build(&mut asset);
            let reference = ActionRef::new(asset.id.clone(), format!("{name}.inputaction"));
            self.lookup.insert(asset.id.clone(), (Arc::new(asset), name.to_string()));
            reference
        }

        fn bind(&mut self, key: &str, action: &ActionRef) -> u32 {
            self.mapping.add_entry(self.category, key, Some(action.clone())).unwrap()
        }

        fn start(&mut self) -> ContextHandle {
            let (handle, problems) = self.system.add(&self.mapping, &self.lookup, 0, None);
            assert!(problems.is_empty(), "{problems:?}");
            handle
        }

        /// One 0.1 second frame; returns the events as `name.phase`.
        fn frame(&mut self) -> Vec<String> {
            self.raw.derive();
            let events = self.system.update(&self.raw, 0.1, false);
            self.raw.end_frame();
            events.iter().map(|e| format!("{}.{}", e.name, e.phase.word())).collect()
        }

        fn frame_events(&mut self) -> Vec<ActionEvent> {
            self.raw.derive();
            let events = self.system.update(&self.raw, 0.1, false);
            self.raw.end_frame();
            events
        }

        fn press(&mut self, code: i64) {
            self.raw.key_code(code, true);
        }

        fn release(&mut self, code: i64) {
            self.raw.key_code(code, false);
        }
    }

    const SPACE: i64 = 62;
    const W: i64 = 41;
    const SHIFT: i64 = 60;

    #[test]
    fn an_action_with_no_triggers_fires_while_held_and_completes_on_release() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |_| {});
        rig.bind("Keyboard.Space", &jump);
        rig.start();
        assert_eq!(rig.frame(), Vec::<String>::new());
        rig.press(SPACE);
        assert_eq!(rig.frame(), ["Jump.started", "Jump.triggered"]);
        assert_eq!(rig.frame(), ["Jump.triggered"]);
        rig.release(SPACE);
        assert_eq!(rig.frame(), ["Jump.completed"]);
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    #[test]
    fn a_tap_between_two_frames_is_not_lost() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |a| a.triggers = vec![Tolerant::Known(Trigger::Pressed)]);
        rig.bind("Keyboard.Space", &jump);
        rig.start();
        rig.press(SPACE);
        rig.release(SPACE);
        assert_eq!(rig.frame(), ["Jump.started", "Jump.triggered"]);
        assert_eq!(rig.frame(), ["Jump.completed"]);
    }

    #[test]
    fn several_keys_drive_one_action_without_adding_up() {
        let mut rig = Rig::new();
        let fire = rig.action("Fire", |_| {});
        rig.bind("Keyboard.Space", &fire);
        rig.bind("Mouse.Left", &fire);
        rig.start();
        rig.press(SPACE);
        rig.raw.mouse_button(0, true);
        let events = rig.frame_events();
        let triggered: Vec<_> = events.iter().filter(|e| e.phase == Phase::Triggered).collect();
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].value[0], 1.0);
    }

    #[test]
    fn wasd_makes_a_two_dimensional_move() {
        let mut rig = Rig::new();
        let mv = rig.action("Move", |a| a.value_type = ValueType::Axis2D);
        rig.mapping.add_direction_keys(rig.category, &mv, "Keyboard.KeyW", "Keyboard.KeyS", "Keyboard.KeyA", "Keyboard.KeyD");
        rig.start();
        rig.press(W);
        let e = rig.frame_events();
        assert_eq!(e.iter().find(|e| e.phase == Phase::Triggered).unwrap().value[..2], [0.0, 1.0]);
        rig.press(22); // D
        let e = rig.frame_events();
        assert_eq!(e.iter().find(|e| e.phase == Phase::Triggered).unwrap().value[..2], [1.0, 1.0]);
        rig.release(W);
        rig.release(22);
        rig.press(37); // S
        let e = rig.frame_events();
        assert_eq!(e.iter().find(|e| e.phase == Phase::Triggered).unwrap().value[..2], [0.0, -1.0]);
    }

    #[test]
    fn a_stick_with_a_dead_zone_ignores_drift() {
        let mut rig = Rig::new();
        let look = rig.action("Look", |a| {
            a.value_type = ValueType::Axis2D;
            a.modifiers = vec![Tolerant::Known(Modifier::DeadZone { lower: 0.25, upper: 1.0, radial: true })];
            a.actuation_threshold = 0.05;
        });
        rig.bind("Gamepad.RightStick", &look);
        rig.start();
        rig.raw.pad(crate::keys::Pad::RightStickX, 0.1);
        assert_eq!(rig.frame(), Vec::<String>::new(), "drift does nothing");
        rig.raw.pad(crate::keys::Pad::RightStickX, 1.0);
        let e = rig.frame_events();
        assert_eq!(e.iter().find(|e| e.phase == Phase::Triggered).unwrap().value[0], 1.0);
    }

    #[test]
    fn hold_reports_ongoing_then_triggered_and_cancels_when_released_early() {
        let mut rig = Rig::new();
        let charge = rig.action("Charge", |a| a.triggers = vec![Tolerant::Known(Trigger::Hold { seconds: 0.25, once: false })]);
        rig.bind("Keyboard.Space", &charge);
        rig.start();
        rig.press(SPACE);
        assert_eq!(rig.frame(), ["Charge.started", "Charge.ongoing"]);
        assert_eq!(rig.frame(), ["Charge.ongoing"]);
        assert_eq!(rig.frame(), ["Charge.triggered"]);
        rig.release(SPACE);
        assert_eq!(rig.frame(), ["Charge.completed"]);

        rig.press(SPACE);
        rig.frame();
        rig.release(SPACE);
        assert_eq!(rig.frame(), ["Charge.canceled"]);
    }

    #[test]
    fn an_entrys_own_trigger_applies_to_that_key_only() {
        let mut rig = Rig::new();
        let dodge = rig.action("Dodge", |_| {});
        let tap = rig.bind("Keyboard.Space", &dodge);
        rig.mapping.entry_mut(tap).unwrap().triggers = vec![Tolerant::Known(Trigger::DoubleTap { window: 0.5 })];
        rig.start();
        rig.press(SPACE);
        assert_eq!(rig.frame(), ["Dodge.started", "Dodge.ongoing"]);
        rig.release(SPACE);
        rig.frame();
        rig.press(SPACE);
        assert!(rig.frame().contains(&"Dodge.triggered".to_string()));
    }

    #[test]
    fn a_chord_needs_the_other_action_to_be_firing() {
        let mut rig = Rig::new();
        let modifier = rig.action("Modifier", |_| {});
        let save = rig.action("Save", {
            let modifier = modifier.clone();
            move |a| {
                a.triggers = vec![Tolerant::Known(Trigger::Chorded { action: modifier }), Tolerant::Known(Trigger::Pressed)];
            }
        });
        rig.bind("Keyboard.ShiftLeft", &modifier);
        rig.bind("Keyboard.KeyS", &save);
        rig.start();
        rig.press(37);
        assert_eq!(rig.frame(), Vec::<String>::new(), "S alone does nothing");
        rig.release(37);
        rig.frame();
        rig.press(SHIFT);
        rig.press(37);
        let names = rig.frame();
        assert!(names.contains(&"Save.triggered".to_string()), "{names:?}");
    }

    #[test]
    fn a_chord_on_an_action_the_mapping_does_not_bind_never_fires() {
        let mut rig = Rig::new();
        let ghost = rig.action("Ghost", |_| {});
        let save = rig.action("Save", {
            let ghost = ghost.clone();
            move |a| a.triggers = vec![Tolerant::Known(Trigger::Chorded { action: ghost })]
        });
        rig.bind("Keyboard.KeyS", &save);
        let (_, problems) = rig.system.add(&rig.mapping, &rig.lookup, 0, None);
        assert_eq!(problems.len(), 1);
        rig.press(37);
        assert_eq!(rig.frame(), Vec::<String>::new());
    }

    #[test]
    fn a_higher_priority_mapping_consumes_the_key() {
        let mut rig = Rig::new();
        let walk = rig.action("Walk", |_| {});
        let menu = rig.action("MenuSelect", |_| {});
        rig.bind("Keyboard.Space", &walk);
        let mut menu_map = MappingAsset::new();
        let cat = menu_map.categories[0].id;
        menu_map.add_entry(cat, "Keyboard.Space", Some(menu));
        rig.system.add(&rig.mapping, &rig.lookup, 0, None);
        let (menu_handle, _) = rig.system.add(&menu_map, &rig.lookup, 10, None);
        rig.press(SPACE);
        assert_eq!(rig.frame(), ["MenuSelect.started", "MenuSelect.triggered"], "the menu sees it first and the game doesn't");
        rig.system.remove(menu_handle);
        let names = rig.frame();
        assert!(names.contains(&"MenuSelect.completed".to_string()), "removing a mapping ends its actions: {names:?}");
        assert!(names.contains(&"Walk.triggered".to_string()), "and the game gets the key back: {names:?}");
    }

    #[test]
    fn an_action_that_does_not_consume_lets_lower_mappings_see_the_key() {
        let mut rig = Rig::new();
        let walk = rig.action("Walk", |_| {});
        let peek = rig.action("Peek", |a| a.consume_input = false);
        rig.bind("Keyboard.Space", &walk);
        let mut top = MappingAsset::new();
        let cat = top.categories[0].id;
        top.add_entry(cat, "Keyboard.Space", Some(peek));
        rig.system.add(&rig.mapping, &rig.lookup, 0, None);
        rig.system.add(&top, &rig.lookup, 5, None);
        rig.press(SPACE);
        let names = rig.frame();
        assert!(names.contains(&"Peek.triggered".to_string()) && names.contains(&"Walk.triggered".to_string()), "{names:?}");
    }

    #[test]
    fn disabling_a_mapping_cancels_and_completes_what_it_was_doing() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |_| {});
        rig.bind("Keyboard.Space", &jump);
        let handle = rig.start();
        rig.press(SPACE);
        rig.frame();
        rig.system.set_enabled(handle, false);
        assert_eq!(rig.frame(), ["Jump.completed"]);
        assert_eq!(rig.frame(), Vec::<String>::new());
        rig.system.set_enabled(handle, true);
        assert_eq!(rig.frame(), ["Jump.started", "Jump.triggered"]);
    }

    #[test]
    fn a_paused_game_silences_actions_unless_they_opt_in() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |_| {});
        let pause = rig.action("Pause", |a| a.trigger_when_paused = true);
        rig.bind("Keyboard.Space", &jump);
        rig.bind("Keyboard.Escape", &pause);
        rig.start();
        rig.press(SPACE);
        rig.press(114);
        rig.raw.derive();
        let events = rig.system.update(&rig.raw, 0.1, true);
        let names: Vec<_> = events.iter().map(|e| e.name.to_string()).collect();
        assert!(names.iter().all(|n| n == "Pause"), "{names:?}");
        assert!(!names.is_empty());
    }

    #[test]
    fn timings_count_from_the_start_of_the_activation() {
        let mut rig = Rig::new();
        let run = rig.action("Run", |_| {});
        rig.bind("Keyboard.Space", &run);
        rig.start();
        rig.press(SPACE);
        rig.frame();
        rig.frame();
        let e = rig.frame_events();
        let last = e.last().unwrap();
        assert!((last.elapsed - 0.2).abs() < 1e-4, "{}", last.elapsed);
        assert!((last.triggered_for - 0.2).abs() < 1e-4);
    }

    #[test]
    fn missing_actions_and_unknown_keys_are_reported_when_adding() {
        let mut rig = Rig::new();
        rig.mapping.add_entry(rig.category, "Keyboard.Space", Some(ActionRef::new(AssetId::new(), "")));
        rig.mapping.add_entry(rig.category, "Keyboard.Nope", None);
        let jump = rig.action("Jump", |_| {});
        rig.bind("Keyboard.Nope", &jump);
        let (_, problems) = rig.system.add(&rig.mapping, &rig.lookup, 0, None);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn owned_mappings_can_be_removed_together() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |_| {});
        rig.bind("Keyboard.Space", &jump);
        rig.system.add(&rig.mapping, &rig.lookup, 0, Some(7));
        rig.system.add(&rig.mapping, &rig.lookup, 0, Some(8));
        rig.system.remove_owned_by(7);
        assert_eq!(rig.system.len(), 1);
        rig.system.remove_all();
        assert!(rig.system.is_empty());
    }

    #[test]
    fn events_carry_their_owner_and_value_type() {
        let mut rig = Rig::new();
        let jump = rig.action("Jump", |a| a.value_type = ValueType::Axis1D);
        rig.bind("Keyboard.Space", &jump);
        rig.system.add(&rig.mapping, &rig.lookup, 0, Some(42));
        rig.press(SPACE);
        let e = rig.frame_events();
        assert!(e.iter().all(|e| e.owner == Some(42) && e.value_type == ValueType::Axis1D));
    }

    #[test]
    fn a_swizzled_entry_for_a_one_dimensional_action_uses_the_right_component() {
        let mut rig = Rig::new();
        let zoom = rig.action("Zoom", |a| a.value_type = ValueType::Axis1D);
        let e = rig.bind("Keyboard.KeyE", &zoom);
        rig.mapping.entry_mut(e).unwrap().modifiers = vec![Tolerant::Known(Modifier::Scale { x: 3.0, y: 1.0, z: 1.0 }), Tolerant::Known(Modifier::Swizzle { order: Swizzle::Yxz })];
        rig.start();
        rig.press(23);
        assert!(rig.frame().is_empty(), "X was swizzled away, so there is nothing for a 1D action to read");
    }
}
