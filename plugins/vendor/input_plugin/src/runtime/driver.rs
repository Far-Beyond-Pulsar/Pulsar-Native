//! One input session: devices in, active mappings, action events out.
//!
//! A [`Driver`] belongs to one world. Each frame, [`Driver::frame`]:
//!
//! 1. takes the raw input that arrived on the event hub since last frame
//!    (keys, mouse buttons, motion, wheel) and what the controllers did;
//! 2. brings the active mappings in line with what the world wants (one per
//!    enabled `PlayerInputComponent`) and with what scripts asked for;
//! 3. notices when a mapping or an action it uses was edited on disk, and
//!    recompiles it, so changes show up while the game is running;
//! 4. evaluates every mapping and publishes the actions' events.
//!
//! Nothing here touches the world, so it is tested without one; `tick` in the
//! parent module is the thin layer that reads components and calls it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pulsar_events::EventHub;

use super::assets::{Assets, Stamps};
use super::events::{Command, HubBinding, Inbox, Raw};
use super::gamepad::{self, Gamepads};
use super::raw::RawInput;
use super::system::{ContextHandle, InputSystem};

/// How often files are checked for edits.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// A mapping the world wants active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// Identifies the thing that wants it (a component instance), so a
    /// change to it replaces the old mapping.
    pub holder: u64,
    /// The object the mapping belongs to.
    pub owner: u64,
    /// Project-relative path or id of the mapping.
    pub mapping: String,
    /// Added to the mapping's own priority.
    pub priority: i32,
}

struct Attachment {
    mapping: String,
    priority: i32,
    owner: Option<u64>,
    handle: Option<ContextHandle>,
    stamps: Stamps,
}

pub struct Driver {
    assets: Assets,
    system: InputSystem,
    raw: RawInput,
    binding: Option<HubBinding>,
    pads: Option<Gamepads>,
    /// Mappings the world wants, by holder.
    attached: HashMap<u64, Attachment>,
    /// Mappings scripts added, by (mapping, owner).
    commanded: HashMap<(String, u64), Attachment>,
    last_watch: Instant,
    /// Problems already reported, so they're said once.
    reported: std::collections::HashSet<String>,
}

impl Driver {
    /// `root` is the project (found automatically if `None`); `pads` starts
    /// reading controllers.
    pub fn new(root: Option<PathBuf>, pads: bool) -> Self {
        Self {
            assets: Assets::new(root),
            system: InputSystem::new(),
            raw: RawInput::new(),
            binding: None,
            pads: pads.then(Gamepads::spawn),
            attached: HashMap::new(),
            commanded: HashMap::new(),
            last_watch: Instant::now(),
            reported: Default::default(),
        }
    }

    /// The mappings now active, highest priority first.
    pub fn active_mappings(&self) -> usize {
        self.system.len()
    }

    /// Run one frame.
    pub fn frame(&mut self, hub: &EventHub, dt: f32, wanted: &[Wanted]) {
        if !self.binding.as_ref().is_some_and(|b| b.hub().ptr_eq(hub)) {
            // A different hub is a different session: nothing carries over.
            self.system.remove_all();
            self.attached.clear();
            self.commanded.clear();
            self.raw.release_all();
            self.binding = Some(HubBinding::attach(hub));
        }

        let inbox = self.binding.as_ref().map(HubBinding::drain).unwrap_or_default();
        self.feed(&inbox);
        if let Some(pads) = &self.pads {
            gamepad::apply(&mut self.raw, pads.take());
        }

        self.sync_wanted(wanted);
        self.run_commands(inbox.commands);
        self.watch_files();

        self.raw.derive();
        let events = self.system.update(&self.raw, dt, false);
        self.raw.end_frame();
        if let Some(binding) = &self.binding {
            for event in &events {
                binding.publish(event);
            }
        }
    }

    fn feed(&mut self, inbox: &Inbox) {
        for raw in &inbox.raw {
            match *raw {
                Raw::Key { code, down } => self.raw.key_code(code, down),
                Raw::MouseButton { button, down } => self.raw.mouse_button(button, down),
                Raw::MouseMoved { dx, dy } => self.raw.mouse_moved(dx, dy),
                Raw::Wheel { x, y } => self.raw.wheel(x, y),
            }
        }
    }

    // ── Which mappings are active ──────────────────────────────────────────

    fn sync_wanted(&mut self, wanted: &[Wanted]) {
        let gone: Vec<u64> = self.attached.keys().copied().filter(|h| !wanted.iter().any(|w| w.holder == *h)).collect();
        for holder in gone {
            if let Some(a) = self.attached.remove(&holder) {
                self.detach(a);
            }
        }
        for w in wanted {
            let unchanged = self.attached.get(&w.holder).is_some_and(|a| a.mapping == w.mapping && a.priority == w.priority && a.owner == Some(w.owner));
            if unchanged {
                continue;
            }
            if let Some(old) = self.attached.remove(&w.holder) {
                self.detach(old);
            }
            let attachment = self.attach(&w.mapping, w.priority, Some(w.owner));
            self.attached.insert(w.holder, attachment);
        }
        // A mapping that wasn't found yet may exist now.
        let retry: Vec<u64> = self.attached.iter().filter(|(_, a)| a.handle.is_none()).map(|(h, _)| *h).collect();
        for holder in retry {
            if let Some(a) = self.attached.remove(&holder) {
                let again = self.attach(&a.mapping, a.priority, a.owner);
                self.attached.insert(holder, again);
            }
        }
    }

    fn run_commands(&mut self, commands: Vec<Command>) {
        for command in commands {
            match command {
                Command::Add { mapping, priority, entity } => {
                    let key = (mapping.clone(), entity);
                    if let Some(old) = self.commanded.remove(&key) {
                        self.detach(old);
                    }
                    let owner = (entity != 0).then_some(entity);
                    let attachment = self.attach(&mapping, priority.clamp(i32::MIN as i64, i32::MAX as i64) as i32, owner);
                    self.commanded.insert(key, attachment);
                }
                Command::Remove { mapping, entity } => {
                    if let Some(old) = self.commanded.remove(&(mapping, entity)) {
                        self.detach(old);
                    }
                }
                Command::SetEnabled { mapping, entity, enabled } => {
                    if let Some(handle) = self.commanded.get(&(mapping, entity)).and_then(|a| a.handle) {
                        self.system.set_enabled(handle, enabled);
                    }
                }
            }
        }
    }

    fn attach(&mut self, mapping: &str, priority: i32, owner: Option<u64>) -> Attachment {
        let none = |stamps: Stamps| Attachment { mapping: mapping.to_string(), priority, owner, handle: None, stamps };
        let Some(entry) = self.assets.mapping(mapping) else {
            if !mapping.trim().is_empty() && self.reported.insert(format!("missing:{mapping}")) {
                tracing::warn!("input: the mapping `{mapping}` can't be found in the project");
            }
            return none(Stamps::default());
        };
        let library = self.assets.library();
        let used = entry.asset.used_actions();
        let mut watched = vec![entry.path.clone()];
        for id in &used {
            if let Some(action) = library.action_entry(id) {
                watched.push(action.path.clone());
                if let Some(binding) = &mut self.binding {
                    binding.declare_action(&action.event_name().into());
                }
            }
        }
        let effective = entry.asset.priority.saturating_add(priority);
        let (handle, problems) = self.system.add(&entry.asset, library.as_ref(), effective, owner);
        for problem in problems {
            if self.reported.insert(format!("{}:{problem}", entry.id)) {
                tracing::warn!("input: mapping `{}`: {problem}", entry.relative);
            }
        }
        Attachment { mapping: mapping.to_string(), priority, owner, handle: Some(handle), stamps: Stamps::of(watched) }
    }

    fn detach(&mut self, attachment: Attachment) {
        if let Some(handle) = attachment.handle {
            self.system.remove(handle);
        }
    }

    // ── Hot reload ─────────────────────────────────────────────────────────

    fn watch_files(&mut self) {
        if self.last_watch.elapsed() < WATCH_EVERY {
            return;
        }
        self.last_watch = Instant::now();
        let stale = |a: &Attachment| a.handle.is_some() && a.stamps.changed();
        let holders: Vec<u64> = self.attached.iter().filter(|(_, a)| stale(a)).map(|(h, _)| *h).collect();
        let commanded: Vec<(String, u64)> = self.commanded.iter().filter(|(_, a)| stale(a)).map(|(k, _)| k.clone()).collect();
        if holders.is_empty() && commanded.is_empty() {
            return;
        }
        self.assets.rescan();
        self.reported.clear();
        for holder in holders {
            if let Some(a) = self.attached.remove(&holder) {
                let (mapping, priority, owner) = (a.mapping.clone(), a.priority, a.owner);
                self.detach(a);
                let again = self.attach(&mapping, priority, owner);
                self.attached.insert(holder, again);
            }
        }
        for key in commanded {
            if let Some(a) = self.commanded.remove(&key) {
                let (mapping, priority, owner) = (a.mapping.clone(), a.priority, a.owner);
                self.detach(a);
                let again = self.attach(&mapping, priority, owner);
                self.commanded.insert(key, again);
            }
        }
    }

    /// The project's library as the driver last scanned it.
    pub fn library(&mut self) -> Arc<crate::library::Library> {
        self.assets.library()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::Tolerant;
    use crate::model::{ActionAsset, ActionRef, Asset, MappingAsset, Trigger};
    use pulsar_events::gamma::{Channel, DynValue, SubscribeOptions};
    use pulsar_events::FlushPoint;
    use std::sync::Mutex;

    const SPACE: i64 = 62;

    struct Project {
        root: PathBuf,
        jump: ActionAsset,
    }

    impl Project {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("input_plugin_driver_{}_{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("input")).unwrap();
            let jump = ActionAsset::new();
            std::fs::write(root.join("input/Jump.inputaction"), jump.to_text()).unwrap();
            let mut map = MappingAsset::new();
            let cat = map.categories[0].id;
            map.add_entry(cat, "Keyboard.Space", Some(ActionRef::new(jump.id.clone(), "input/Jump.inputaction")));
            std::fs::write(root.join("input/Game.inputmap"), map.to_text()).unwrap();
            Self { root, jump }
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn wanted(owner: u64) -> Vec<Wanted> {
        vec![Wanted { holder: 1, owner, mapping: "input/Game.inputmap".into(), priority: 0 }]
    }

    /// One frame the way the tick loop runs it: input flush, the driver,
    /// then the flush after it.
    fn frame(driver: &mut Driver, hub: &EventHub, wanted: &[Wanted]) {
        hub.flush(FlushPoint::AfterInput);
        driver.frame(hub, 0.016, wanted);
        hub.flush(FlushPoint::AfterPhysics);
    }

    fn listen(hub: &EventHub, name: &str, channel: Channel) -> Arc<Mutex<Vec<Vec<DynValue>>>> {
        let id = hub.descriptor_by_name(name).unwrap_or_else(|| panic!("`{name}` isn't registered")).id;
        let seen: Arc<Mutex<Vec<Vec<DynValue>>>> = Arc::default();
        let sink = seen.clone();
        std::mem::forget(hub.bus().subscribe_dyn(id, SubscribeOptions::channel(channel), move |e| sink.lock().unwrap().push(e.fields.clone())));
        seen
    }

    #[test]
    fn pressing_a_key_publishes_the_action_to_a_script_on_the_owner() {
        let project = Project::new("press");
        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        frame(&mut driver, &hub, &wanted(77));
        assert_eq!(driver.active_mappings(), 1);

        let started = listen(&hub, "input::Jump.started", Channel::Entity(77));
        let triggered = listen(&hub, "input::Jump.triggered", Channel::Entity(77));
        let completed = listen(&hub, "input::Jump.completed", Channel::Entity(77));

        hub.publish(Channel::Global, pulsar_events::builtin::KeyDown { key: SPACE });
        frame(&mut driver, &hub, &wanted(77));
        frame(&mut driver, &hub, &wanted(77));
        hub.publish(Channel::Global, pulsar_events::builtin::KeyUp { key: SPACE });
        frame(&mut driver, &hub, &wanted(77));

        assert_eq!(started.lock().unwrap().len(), 1);
        assert_eq!(triggered.lock().unwrap().len(), 2);
        assert_eq!(completed.lock().unwrap().len(), 1);
        assert_eq!(triggered.lock().unwrap()[0][0], DynValue::U64(77));
        assert_eq!(triggered.lock().unwrap()[0][1], DynValue::F64(1.0));
    }

    #[test]
    fn removing_the_component_ends_the_mapping_and_completes_its_actions() {
        let project = Project::new("remove");
        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        frame(&mut driver, &hub, &wanted(5));
        let completed = listen(&hub, "input::Jump.completed", Channel::Entity(5));
        hub.publish(Channel::Global, pulsar_events::builtin::KeyDown { key: SPACE });
        frame(&mut driver, &hub, &wanted(5));
        frame(&mut driver, &hub, &[]);
        assert_eq!(driver.active_mappings(), 0);
        assert_eq!(completed.lock().unwrap().len(), 1);
    }

    #[test]
    fn changing_a_components_mapping_swaps_it() {
        let project = Project::new("swap");
        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        frame(&mut driver, &hub, &wanted(1));
        assert_eq!(driver.active_mappings(), 1);
        let other = vec![Wanted { holder: 1, owner: 1, mapping: "input/Missing.inputmap".into(), priority: 0 }];
        frame(&mut driver, &hub, &other);
        assert_eq!(driver.active_mappings(), 0, "a mapping that can't be found is simply not active");
        frame(&mut driver, &hub, &wanted(1));
        assert_eq!(driver.active_mappings(), 1);
    }

    #[test]
    fn a_script_can_add_and_remove_a_global_mapping_by_event() {
        let project = Project::new("commands");
        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        frame(&mut driver, &hub, &[]);
        hub.publish_named(Channel::Global, "input::AddMapping", vec![DynValue::Str("input/Game.inputmap".into()), DynValue::I64(0), DynValue::U64(0)]).unwrap();
        frame(&mut driver, &hub, &[]);
        assert_eq!(driver.active_mappings(), 1);

        let seen = listen(&hub, "input::Jump.triggered", Channel::Global);
        hub.publish(Channel::Global, pulsar_events::builtin::KeyDown { key: SPACE });
        frame(&mut driver, &hub, &[]);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(seen.lock().unwrap()[0][0], DynValue::U64(0), "a global mapping has no entity");

        hub.publish_named(Channel::Global, "input::SetMappingEnabled", vec![DynValue::Str("input/Game.inputmap".into()), DynValue::U64(0), DynValue::Bool(false)]).unwrap();
        frame(&mut driver, &hub, &[]);
        hub.publish_named(Channel::Global, "input::RemoveMapping", vec![DynValue::Str("input/Game.inputmap".into()), DynValue::U64(0)]).unwrap();
        frame(&mut driver, &hub, &[]);
        assert_eq!(driver.active_mappings(), 0);
    }

    #[test]
    fn editing_an_action_on_disk_takes_effect_in_the_running_game() {
        let project = Project::new("hot");
        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        frame(&mut driver, &hub, &wanted(3));
        let pressed = listen(&hub, "input::Jump.triggered", Channel::Entity(3));
        // Make Jump fire on press only.
        let mut jump = project.jump.clone();
        jump.triggers = vec![Tolerant::Known(Trigger::Pressed)];
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(project.root.join("input/Jump.inputaction"), jump.to_text()).unwrap();
        driver.last_watch = Instant::now() - WATCH_EVERY * 2;
        hub.publish(Channel::Global, pulsar_events::builtin::KeyDown { key: SPACE });
        for _ in 0..4 {
            frame(&mut driver, &hub, &wanted(3));
        }
        assert_eq!(pressed.lock().unwrap().len(), 1, "fires once, not every frame, now that it is a Pressed trigger");
    }

    #[test]
    fn mouse_motion_and_wheel_reach_the_mapping() {
        let project = Project::new("mouse");
        let look = {
            let mut a = ActionAsset::new();
            a.value_type = crate::model::ValueType::Axis2D;
            a.actuation_threshold = 0.1;
            a
        };
        std::fs::write(project.root.join("input/Look.inputaction"), look.to_text()).unwrap();
        let mut map = MappingAsset::new();
        let cat = map.categories[0].id;
        map.add_entry(cat, "Mouse.XY", Some(ActionRef::new(look.id.clone(), "")));
        std::fs::write(project.root.join("input/Mouse.inputmap"), map.to_text()).unwrap();

        let hub = EventHub::new();
        let mut driver = Driver::new(Some(project.root.clone()), false);
        let w = vec![Wanted { holder: 1, owner: 0, mapping: "input/Mouse.inputmap".into(), priority: 0 }];
        frame(&mut driver, &hub, &w);
        let seen = listen(&hub, "input::Look.triggered", Channel::Entity(0));
        hub.publish(Channel::Global, pulsar_events::builtin::MouseMoved { x: 0.0, y: 0.0, dx: 4.0, dy: -2.0 });
        frame(&mut driver, &hub, &w);
        frame(&mut driver, &hub, &w);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "motion lasts the frame it arrived in");
        assert_eq!(seen[0][2], DynValue::F64(4.0));
        assert_eq!(seen[0][3], DynValue::F64(2.0), "dragging up reads as positive Y");
    }

    #[test]
    fn a_new_hub_starts_the_session_over() {
        let project = Project::new("hubs");
        let mut driver = Driver::new(Some(project.root.clone()), false);
        let a = EventHub::new();
        frame(&mut driver, &a, &wanted(1));
        let b = EventHub::new();
        frame(&mut driver, &b, &[]);
        assert_eq!(driver.active_mappings(), 0);
        assert!(b.descriptor_by_name("input::ActionTriggered").is_some(), "the new hub has the fixed events");
    }
}
