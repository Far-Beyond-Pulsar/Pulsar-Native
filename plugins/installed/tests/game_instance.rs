//! The game instance, the default level and level switching, end to end with
//! every installed plugin (`.agents/GAMEPLAY.md`, "The game instance and
//! level switching").
//!
//! The project here has no startup level (`"startup_level": ""`) and a game
//! instance Blueprint, `Main`, with a widget. Level `a` has a falling body, a
//! scripted object and a game mode; level `b` has a floating body and no
//! gravity. `Main` switches between them with the `Load Level` node: from its
//! widget button's `on_action`, then from its tick, then tries a level that
//! does not exist and quits.
//!
//! - Packaged and run headless (`a_game_instance_switches_levels_headless`):
//!   the order (begin_play, on_init, then each level), nothing loaded until
//!   asked, the instance and its state surviving, no role entity, object,
//!   script or physics body of an old level left behind, the new level's
//!   gravity, the failed load leaving the level as it is, Quit.
//! - Hydrating across ticks (`gameplay_waits_for_a_whole_level`): while a
//!   level is partly in the world, nothing of gameplay ticks.
//! - Play-in-Editor (`play_in_editor_switches_levels_and_stop_restores`): the
//!   open level plays after the game instance, Load Level of it reads the
//!   unsaved editor world, Quit is reported, and Stop restores the editor's
//!   world exactly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use installed_plugins as _;

use engine_backend::scene::{attachments, RuntimeLevel, SceneWorldExt, Transform};
use pulsar_content::ContentRoot;
use pulsar_core::TickMode;
use pulsar_game::standalone::{self, HeadlessHooks, HEADLESS_STEP};
use pulsar_game::tick::TickLoop;
use serde_json::{json, Map, Value};

/// Sessions, the plugin set and the installed content are process-wide.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner())
}

// ── The Blueprint graphs ────────────────────────────────────────────────────

fn data(base: &str) -> Value {
    json!({ "Data": { "base_type": base, "wrappers": [], "is_wildcard": false } })
}

#[derive(Default)]
struct Graph {
    nodes: Vec<(
        String,
        String,
        Value,
        Vec<(String, Value)>,
        Vec<(String, Value)>,
    )>,
    edges: Vec<(String, String, String, String, bool)>,
}

impl Graph {
    fn node(
        &mut self,
        id: &str,
        node_type: &str,
        props: Value,
        inputs: &[(&str, &str)],
        outputs: &[(&str, &str)],
    ) {
        let pins = |pins: &[(&str, &str)]| {
            pins.iter()
                .map(|(name, ty)| {
                    let ty = match *ty {
                        "exec" => json!("Execution"),
                        base => data(base),
                    };
                    (name.to_string(), ty)
                })
                .collect()
        };
        self.nodes.push((
            id.into(),
            node_type.into(),
            props,
            pins(inputs),
            pins(outputs),
        ));
    }

    fn then(&mut self, from: &str, pin: &str, to: &str) {
        self.edges
            .push((from.into(), pin.into(), to.into(), "exec".into(), true));
    }

    fn wire(&mut self, from: &str, pin: &str, to: &str, to_pin: &str) {
        self.edges
            .push((from.into(), pin.into(), to.into(), to_pin.into(), false));
    }

    // Recipes.

    fn event(&mut self, id: &str, node_type: &str, outputs: &[(&str, &str)]) {
        let mut all = vec![("Body", "exec")];
        all.extend_from_slice(outputs);
        self.node(id, node_type, json!({ "scope": "self" }), &[], &all);
    }

    fn component_event(&mut self, id: &str, class: &str, member: &str) {
        self.node(
            id,
            &format!("event::on_component::{class}.{member}"),
            json!({ "component_type": class }),
            &[],
            &[("Body", "exec")],
        );
    }

    fn get(&mut self, id: &str, variable: &str, ty: &str) {
        self.node(
            id,
            &format!("get_{variable}"),
            json!({}),
            &[],
            &[("value", ty)],
        );
    }

    fn set(&mut self, id: &str, variable: &str, ty: &str, props: Value) {
        self.node(
            id,
            &format!("set_{variable}"),
            props,
            &[("exec", "exec"), ("value", ty)],
            &[("exec_out", "exec")],
        );
    }

    /// `variable += 1`, as nodes `<id>_get`, `<id>_add` and `<id>` (the set).
    fn increment(&mut self, id: &str, variable: &str) {
        self.get(&format!("{id}_get"), variable, "i64");
        self.node(
            &format!("{id}_add"),
            "add",
            json!({ "b": 1 }),
            &[("a", "i64"), ("b", "i64")],
            &[("result", "i64")],
        );
        self.set(id, variable, "i64", json!({}));
        self.wire(&format!("{id}_get"), "value", &format!("{id}_add"), "a");
        self.wire(&format!("{id}_add"), "result", id, "value");
    }

    fn concat(&mut self, id: &str, props: Value) {
        self.node(
            id,
            "string_concat",
            props,
            &[("a", "String"), ("b", "String")],
            &[("result", "String")],
        );
    }

    /// `order = order + text` (a literal), as `<id>_get`, `<id>_cat` and `<id>`.
    fn note(&mut self, id: &str, text: &str) {
        self.get(&format!("{id}_get"), "order", "String");
        self.concat(&format!("{id}_cat"), json!({ "b": text }));
        self.set(id, "order", "String", json!({}));
        self.wire(&format!("{id}_get"), "value", &format!("{id}_cat"), "a");
        self.wire(&format!("{id}_cat"), "result", id, "value");
    }

    fn native(&mut self, id: &str, name: &str, props: Value, params: &[(&str, &str)]) {
        let mut inputs = vec![("exec", "exec")];
        inputs.extend_from_slice(params);
        self.node(
            id,
            &format!("native::{name}"),
            props,
            &inputs,
            &[("exec_out", "exec")],
        );
    }

    /// `if variable == value`, as `<id>_get`, `<id>_eq` and `<id>` (a branch).
    fn when_equals(&mut self, id: &str, variable: &str, value: i64) {
        self.get(&format!("{id}_get"), variable, "i64");
        self.node(
            &format!("{id}_eq"),
            "equals",
            json!({ "b": value }),
            &[("a", "i64"), ("b", "i64")],
            &[("result", "bool")],
        );
        self.node(
            id,
            "branch",
            json!({}),
            &[("exec", "exec"), ("condition", "bool")],
            &[("True", "exec"), ("False", "exec")],
        );
        self.wire(&format!("{id}_get"), "value", &format!("{id}_eq"), "a");
        self.wire(&format!("{id}_eq"), "result", id, "condition");
    }

    fn blueprint(&self, variables: &[(&str, &str, &str)], traits: &[&str]) -> Value {
        let ids: Vec<String> = (0..self.edges.len()).map(|i| format!("c{i}")).collect();
        let wires = |node: &str, pin: &str, output: bool| -> Vec<&String> {
            self.edges
                .iter()
                .zip(&ids)
                .filter(|((from, from_pin, to, to_pin, _), _)| {
                    if output {
                        from == node && from_pin == pin
                    } else {
                        to == node && to_pin == pin
                    }
                })
                .map(|(_, id)| id)
                .collect()
        };
        let mut nodes = Map::new();
        for (index, (id, node_type, props, inputs, outputs)) in self.nodes.iter().enumerate() {
            let pins = |pins: &[(String, Value)], output: bool| -> Vec<Value> {
                pins.iter()
                    .map(|(name, ty)| {
                        json!({
                            "id": name, "name": name,
                            "pin_type": if output { "Output" } else { "Input" },
                            "data_type": ty,
                            "connected_to": wires(id, name, output),
                        })
                    })
                    .collect()
            };
            nodes.insert(
                id.clone(),
                json!({
                    "id": id, "node_type": node_type,
                    "position": { "x": (index % 8) * 260, "y": (index / 8) * 180 },
                    "properties": props,
                    "inputs": pins(inputs, false),
                    "outputs": pins(outputs, true),
                }),
            );
        }
        let connections: Vec<Value> = self
            .edges
            .iter()
            .zip(&ids)
            .map(|((from, pin, to, to_pin, exec), id)| {
                json!({
                    "id": id, "source_node": from, "source_pin": pin,
                    "target_node": to, "target_pin": to_pin,
                    "connection_type": if *exec { "Execution" } else { "Data" },
                })
            })
            .collect();
        let variables: Vec<Value> = variables
            .iter()
            .map(|(name, ty, default)| {
                json!({
                    "id": format!("var_{name}"), "name": name, "data_type": data(ty),
                    "default_value": default, "description": "",
                })
            })
            .collect();
        json!({
            "format_version": 2,
            "main_graph": {
                "nodes": nodes,
                "connections": connections,
                "metadata": { "name": "EventGraph", "description": "", "version": "1.0.0",
                    "created_at": "2026-11-26T00:00:00Z", "modified_at": "2026-11-26T00:00:00Z" },
                "comments": [],
                "custom_event_defs": {}
            },
            "subgraphs": [], "local_events": [], "variables": variables,
            "blueprint_metadata": {
                "blueprint_type": "Generic", "description": "", "category": "Test",
                "tags": [], "implemented_trait_ids": traits
            }
        })
    }
}

/// The game instance: notes the order of what it hears, switches levels from
/// its widget button and its tick, then quits.
fn main_graph() -> Value {
    let mut g = Graph::default();

    g.event("begin", "begin_play", &[]);
    g.note("n_begin", "begin;");
    g.then("begin", "Body", "n_begin");

    g.event("init", "event::on::Main.on_init", &[]);
    g.note("n_init", "init;");
    g.node(
        "show",
        "comp_set_prop::WidgetComponent::enabled",
        json!({ "value": true }),
        &[("exec", "exec"), ("value", "bool")],
        &[("exec_out", "exec")],
    );
    g.native(
        "menu_mode",
        "game::set_input_mode",
        json!({ "mode": "UI" }),
        &[("mode", "String")],
    );
    g.then("init", "Body", "n_init");
    g.then("n_init", "exec_out", "show");
    g.then("show", "exec_out", "menu_mode");

    // order += level + ";"
    g.event(
        "loaded",
        "event::on::Main.on_level_loaded",
        &[("level", "String")],
    );
    g.get("l_get", "order", "String");
    g.concat("l_level", json!({}));
    g.concat("l_semi", json!({ "b": ";" }));
    g.set("n_loaded", "order", "String", json!({}));
    g.wire("l_get", "value", "l_level", "a");
    g.wire("loaded", "level", "l_level", "b");
    g.wire("l_level", "result", "l_semi", "a");
    g.wire("l_semi", "result", "n_loaded", "value");
    g.then("loaded", "Body", "n_loaded");

    // The widget's button: count it, play level a.
    g.component_event("click", "WidgetComponent", "on_action");
    g.increment("clicked", "clicks");
    g.native(
        "play",
        "game::load_level",
        json!({ "level": "scene/a.level" }),
        &[("level", "String")],
    );
    g.then("click", "Body", "clicked");
    g.then("clicked", "exec_out", "play");

    g.event("tick", "on_tick", &[("delta_time", "f64")]);
    g.increment("counted", "ticks");
    g.then("tick", "Body", "counted");
    let steps: [(i64, &str, Value); 4] = [
        (10, "game::load_level", json!({ "level": "scene/b.level" })),
        (16, "game::load_level", json!({ "level": "scene/a.level" })),
        (
            19,
            "game::load_level",
            json!({ "level": "scene/missing.level" }),
        ),
        (23, "game::quit", json!({})),
    ];
    let mut from = ("counted".to_string(), "exec_out".to_string());
    for (index, (at, native, props)) in steps.into_iter().enumerate() {
        let branch = format!("at{index}");
        g.when_equals(&branch, "ticks", at);
        g.then(&from.0, &from.1, &branch);
        let action = format!("do{index}");
        let params: &[(&str, &str)] = if native == "game::load_level" {
            &[("level", "String")]
        } else {
            &[]
        };
        g.native(&action, native, props, params);
        g.then(&branch, "True", &action);
        from = (branch, "False".to_string());
    }

    // Shutdown: the widget is still there to take a call.
    g.event("shutdown", "event::on::Main.on_shutdown", &[]);
    g.native(
        "bye",
        "log::info",
        json!({ "message": "MAIN on_shutdown" }),
        &[("message", "String")],
    );
    g.node(
        "still",
        "comp_call::WidgetComponent::set_integer",
        json!({ "name": "score", "value": 1 }),
        &[("exec", "exec"), ("name", "String"), ("value", "i64")],
        &[("exec_out", "exec")],
    );
    g.get("final_get", "order", "String");
    g.node(
        "final_log",
        "native::log::info",
        json!({}),
        &[("exec", "exec"), ("message", "String")],
        &[("exec_out", "exec")],
    );
    g.wire("final_get", "value", "final_log", "message");
    g.then("shutdown", "Body", "bye");
    g.then("bye", "exec_out", "still");
    g.then("still", "exec_out", "final_log");

    g.blueprint(
        &[
            ("order", "String", ""),
            ("ticks", "i64", "0"),
            ("clicks", "i64", "0"),
        ],
        &["pulsar.GameInstance"],
    )
}

/// A scripted level object: it says when it ends.
fn prop_graph() -> Value {
    let mut g = Graph::default();
    g.event("begin", "begin_play", &[]);
    g.set("began", "began", "bool", json!({ "value": true }));
    g.then("begin", "Body", "began");
    g.event("end", "on_end_play", &[]);
    g.native(
        "ended",
        "log::info",
        json!({ "message": "PROP end_play" }),
        &[("message", "String")],
    );
    g.then("end", "Body", "ended");
    g.blueprint(&[("began", "bool", "false")], &[])
}

// ── The project ─────────────────────────────────────────────────────────────

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write_class(root: &Path, name: &str, id: &str, graph: &Value, components: Value) {
    let dir = root.join(format!("classes/{name}.class"));
    write(
        &dir.join("class.json"),
        &json!({ "class_id": id }).to_string(),
    );
    write(&dir.join("graph_save.json"), &graph.to_string());
    if components.as_array().is_some_and(|c| !c.is_empty()) {
        write(
            &dir.join("prefab.json"),
            &json!({ "prefab_version": 1, "name": name, "components": components }).to_string(),
        );
    }
}

fn object(id: &str, y: f64) -> Value {
    json!({
        "id": id, "name": id, "object_type": "Empty",
        "transform": { "position": [0.0, y, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
        "parent": null, "visible": true, "locked": false, "props": {}
    })
}

fn component(class: &str, data: Value) -> Value {
    json!({ "class_name": class, "enabled": true, "data": data })
}

fn level(objects: Vec<Value>, components: Value, settings: Value) -> String {
    let mut world_settings = json!({ "pipeline_version": 1 });
    for (key, value) in settings.as_object().unwrap() {
        world_settings[key] = value.clone();
    }
    json!({
        "version": "2.2", "objects": objects, "components": components,
        "metadata": {}, "world_settings": world_settings
    })
    .to_string()
}

const MAIN_ID: &str = "9a000000-0000-4000-8000-000000000001";
const PROP_ID: &str = "9a000000-0000-4000-8000-000000000002";

fn project(root: &Path) {
    write(
        &root.join("Pulsar.toml"),
        "[project]\nname = \"Instance Game\"\nengine_version = \"nightly\"\n",
    );
    write(
        &root.join("Pulsar/project.json"),
        r#"{ "startup_level": "", "game_instance": "classes/Main.class" }"#,
    );
    write(
        &root.join("ui/Menu.widget"),
        include_str!("../../vendor/ui_editor/examples_assets/HealthBar.widget"),
    );
    write_class(
        root,
        "Main",
        MAIN_ID,
        &main_graph(),
        json!([{
            "slot_id": "9a000000-0000-4000-8000-0000000000b1",
            "class_name": "WidgetComponent", "enabled": true,
            "data": { "widget_asset": "ui/Menu.widget" }
        }]),
    );
    write_class(root, "Prop", PROP_ID, &prop_graph(), json!([]));
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../crates/core/pulsar_package/tests/fixtures/role_game/classes/RoleProbeMode.class",
    );
    for file in ["class.json", "graph_save.json"] {
        write(
            &root.join(format!("classes/RoleProbeMode.class/{file}")),
            &std::fs::read_to_string(probe.join(file)).unwrap(),
        );
    }
    write(
        &root.join("scene/a.level"),
        &level(
            vec![object("a_body", 10.0), object("a_prop", 0.0)],
            json!({
                "a_body": [component("RigidBodyComponent", json!({})), component("ColliderComponent", json!({}))],
                "a_prop": [component("ClassInstance", json!({ "class": "", "class_name": "Prop" }))],
            }),
            json!({
                "gravity": [0.0, -9.81, 0.0],
                "game_mode_blueprint": "classes/RoleProbeMode.class/graph_save.json"
            }),
        ),
    );
    write(
        &root.join("scene/b.level"),
        &level(
            vec![object("b_body", 10.0)],
            json!({ "b_body": [component("RigidBodyComponent", json!({})), component("ColliderComponent", json!({}))] }),
            json!({ "gravity": [0.0, 0.0, 0.0] }),
        ),
    );
}

fn package_loose(project: &Path, out: &Path) -> pulsar_package::PackageReport {
    let mut options = pulsar_package::PackageOptions::new(project, out);
    options.loose = true;
    options.skip_build = true;
    pulsar_package::package(&options).unwrap_or_else(|e| panic!("packaging failed: {e}"))
}

// ── Observing a run ─────────────────────────────────────────────────────────

/// What the world held after one tick.
#[derive(Clone, Debug)]
struct Snapshot {
    level: Option<String>,
    objects: Vec<String>,
    roles: usize,
    classes: Vec<String>,
    bodies: (usize, usize),
    ticks: Option<i64>,
    y: Vec<(String, f64)>,
    mode: pulsar_game::game_control::InputMode,
}

fn objects_of(world: &pulsar_scenedb::World) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next: Vec<pulsar_scenedb::Entity> = world.children_of(None);
    while let Some(entity) = next.pop() {
        ids.push(world.stable_id_of(entity).unwrap_or("?").to_owned());
        next.extend(world.children_of(Some(entity)));
    }
    ids.sort();
    ids
}

fn snapshot(game: &TickLoop) -> Snapshot {
    installed_plugins::physics_plugin::wait_idle(game.session());
    let bodies = installed_plugins::physics_plugin::simulated_counts(game.session());
    let driver = game.scripts.as_ref().expect("scripting").lock().unwrap();
    let runtime = driver.runtime();
    let mut classes: Vec<String> = runtime
        .instance_ids()
        .iter()
        .filter_map(|id| runtime.class_of(id).map(str::to_owned))
        .collect();
    classes.sort();
    let ticks = driver.game_instance().and_then(|entity| {
        let id = driver.instance_of(entity)?;
        match runtime.variable(id, "ticks")? {
            pulsar_script_vm::Value::Int(ticks) => Some(*ticks),
            _ => None,
        }
    });
    let store = game.scene_store.read();
    let world = &store.world;
    let y = ["a_body", "b_body"]
        .into_iter()
        .filter_map(|id| {
            let entity = world.entity_for(id)?;
            Some((id.to_owned(), world.get::<Transform>(entity)?.position[1]))
        })
        .collect();
    Snapshot {
        level: game.current_level().map(str::to_owned),
        objects: objects_of(world),
        roles: world.query::<&pulsar_game::roles::RoleEntity>().count(),
        classes,
        bodies,
        ticks,
        y,
        mode: game.control().input_mode(),
    }
}

/// Log lines of the run, as `message` text.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    fn capture(&self) -> tracing::subscriber::DefaultGuard {
        let writer = self.clone();
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        )
    }
}

/// Raise the widget button's `on_action` on the game instance's widget, as the
/// interaction system does for a click.
fn click_menu_button(game: &TickLoop) {
    let entity = game
        .scripts
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .game_instance()
        .expect("a game instance runs");
    let mut store = game.scene_store.write();
    let widget = attachments::instances(&store.world, entity)
        .into_iter()
        .find(|instance| {
            attachments::meta(&store.world, *instance)
                .is_some_and(|meta| meta.class_name == "WidgetComponent")
        })
        .expect("the game instance has its widget");
    widget_component::component::queue_action(
        &mut store.world,
        widget,
        widget_component::WidgetAction {
            action: "play".into(),
            node: "play_button".into(),
        },
    );
}

fn widget_is_enabled(game: &TickLoop) -> bool {
    let store = game.scene_store.read();
    let found = store
        .world
        .query::<&widget_component::WidgetComponent>()
        .any(|(_, widget)| widget.enabled);
    found
}

#[test]
fn a_game_instance_switches_levels_headless() {
    let _guard = lock();
    let source = tempfile::tempdir().unwrap();
    project(source.path());
    let out = tempfile::tempdir().unwrap();
    let packaged = package_loose(source.path(), out.path());
    assert_eq!(packaged.startup_level, None, "{:?}", packaged.warnings);

    let content = ContentRoot::open(out.path().join("Content")).unwrap();
    let settings = content.settings();
    assert_eq!(
        settings.startup_level.as_deref(),
        Some(""),
        "none stays none"
    );
    assert_eq!(
        settings.game_instance.as_deref(),
        Some(MAIN_ID),
        "cooked by id"
    );
    assert!(
        content.exists("plugins/Default Pipelines/default/Deferred.rendergraph"),
        "the pipeline for a game with no level is cooked"
    );
    content.install();

    let logs = Logs::default();
    let guard = logs.capture();
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    game.enable_project_scripting().expect("scripting starts");
    let seen: Arc<Mutex<Vec<Snapshot>>> = Arc::default();
    let widget_shown = Arc::new(Mutex::new(false));
    let report = {
        let seen = Arc::clone(&seen);
        let widget_shown = Arc::clone(&widget_shown);
        standalone::run_headless_with(
            &mut game,
            &content,
            60,
            HeadlessHooks {
                after_load: None,
                after_tick: Some(Box::new(move |game: &mut TickLoop, tick| {
                    if tick == 3 {
                        *widget_shown.lock().unwrap() = widget_is_enabled(game);
                        click_menu_button(game);
                    }
                    seen.lock().unwrap().push(snapshot(game));
                })),
            },
        )
        .expect("the headless run completes")
    };
    drop(guard);
    let seen = seen.lock().unwrap().clone();
    let logs = logs.text();
    let timeline = seen
        .iter()
        .enumerate()
        .map(|(tick, s)| format!("{tick}: {s:?}"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(report.quit, "Quit Game ends the run\n{timeline}");
    assert!(report.frames < 60, "{}", report.frames);
    assert_eq!(report.level, None, "no startup level");
    assert_eq!(
        report.levels_loaded,
        ["scene/a.level", "scene/b.level", "scene/a.level"],
        "{timeline}"
    );
    assert_eq!(report.final_level.as_deref(), Some("scene/a.level"));

    // The only problem is the level that does not exist, and it left level a.
    assert_eq!(
        report.problems.len(),
        1,
        "{:?}\n{timeline}",
        report.problems
    );
    assert!(
        report.problems[0]
            .message
            .contains("Load Level 'scene/missing.level' failed"),
        "{:?}",
        report.problems
    );
    assert_eq!(report.scripts.script_errors, 0);

    // Before the click: nothing but the game instance, its widget shown, in
    // the UI input mode, its script ticking.
    let before = &seen[2];
    assert_eq!(before.level, None);
    assert_eq!(before.objects, ["pulsar_game_instance"]);
    assert_eq!(before.classes, ["Main"]);
    assert_eq!(before.mode, pulsar_game::game_control::InputMode::Ui);
    assert!(
        *widget_shown.lock().unwrap(),
        "the game instance's widget is on"
    );

    // The instance's own ticks never restart: it survives every switch.
    let ticks: Vec<i64> = seen.iter().filter_map(|s| s.ticks).collect();
    assert_eq!(ticks.len(), seen.len(), "{timeline}");
    assert!(ticks.windows(2).all(|w| w[1] >= w[0]), "{ticks:?}");

    let in_level = |name: &str| -> Vec<&Snapshot> {
        seen.iter()
            .filter(|s| s.level.as_deref() == Some(name))
            .collect()
    };
    let a = in_level("scene/a.level");
    let first_a = a
        .iter()
        .take_while(|s| s.objects.contains(&"a_body".into()));
    for snapshot in first_a.skip(2) {
        let level_objects: Vec<&String> = snapshot
            .objects
            .iter()
            .filter(|id| !id.starts_with("RoleProbeMode"))
            .collect();
        assert_eq!(
            level_objects,
            ["a_body", "a_prop", "pulsar_game_instance"],
            "{timeline}"
        );
        assert_eq!(snapshot.roles, 1, "{timeline}");
        assert_eq!(snapshot.classes, ["Main", "Prop", "RoleProbeMode"]);
        assert_eq!(snapshot.bodies, (1, 1), "{timeline}");
    }
    let b = in_level("scene/b.level");
    assert!(!b.is_empty(), "{timeline}");
    for snapshot in &b {
        assert_eq!(
            snapshot.objects,
            ["b_body", "pulsar_game_instance"],
            "{timeline}"
        );
        assert_eq!(snapshot.roles, 0, "no role entity of a is left: {timeline}");
        assert_eq!(snapshot.classes, ["Main"], "{timeline}");
    }
    assert_eq!(
        b.last().unwrap().bodies,
        (1, 1),
        "only b's body: {timeline}"
    );
    // a's gravity pulls its body down; b has none, and its body stays.
    let height = |snapshots: &[&Snapshot], id: &str| -> Vec<f64> {
        snapshots
            .iter()
            .filter_map(|s| s.y.iter().find(|(name, _)| name == id).map(|(_, y)| *y))
            .collect()
    };
    let falling = height(&a, "a_body");
    assert!(falling.iter().any(|y| *y < 9.99), "a falls: {falling:?}");
    let floating = height(&b, "b_body");
    assert!(
        floating.iter().all(|y| (*y - 10.0).abs() < 0.01),
        "b's gravity is its own: {floating:?}"
    );
    // The reload of a starts it afresh.
    assert_eq!(a.last().unwrap().roles, 1, "{timeline}");
    assert_eq!(a.last().unwrap().bodies, (1, 1), "{timeline}");

    // The order the game instance heard: before any level.
    let main = report.instance_of("Main").expect("one game instance");
    assert_eq!(
        main.variables["order"],
        json!("begin;init;scene/a.level;scene/b.level;scene/a.level;")
    );
    assert_eq!(main.variables["clicks"], json!(1));
    let at = |needle: &str| {
        logs.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` is logged\n{logs}"))
    };
    assert!(at("Game instance started") < at("Level loaded level=scene/a.level"));
    assert_eq!(
        logs.matches("PROP end_play").count(),
        2,
        "once per level end"
    );
    assert!(
        at("MAIN on_shutdown") > logs.rfind("PROP end_play").unwrap(),
        "the game instance ends after the level"
    );

    // Nothing outlives the session.
    let store = game.scene_store.read();
    assert!(
        objects_of(&store.world).is_empty(),
        "{:?}",
        objects_of(&store.world)
    );
    drop(store);
    assert_eq!(
        installed_plugins::physics_plugin::simulated_counts(game.session()),
        (0, 0)
    );
}

#[test]
fn a_game_instance_load_level_in_on_init_replaces_the_default_level() {
    let _guard = lock();
    let source = tempfile::tempdir().unwrap();
    project(source.path());
    // A default level, and a game instance that asks for another in on_init.
    write(
        &source.path().join("Pulsar/project.json"),
        r#"{ "startup_level": "scene/a.level", "game_instance": "classes/Main.class" }"#,
    );
    let mut graph = main_graph();
    let nodes = graph["main_graph"]["nodes"].as_object_mut().unwrap();
    nodes.get_mut("menu_mode").unwrap()["node_type"] = json!("native::game::load_level");
    nodes.get_mut("menu_mode").unwrap()["properties"] = json!({ "level": "scene/b.level" });
    nodes.get_mut("menu_mode").unwrap()["inputs"][1]["id"] = json!("level");
    nodes.get_mut("menu_mode").unwrap()["inputs"][1]["name"] = json!("level");
    write(
        &source.path().join("classes/Main.class/graph_save.json"),
        &graph.to_string(),
    );
    let out = tempfile::tempdir().unwrap();
    package_loose(source.path(), out.path());
    let content = ContentRoot::open(out.path().join("Content")).unwrap();
    content.install();
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    game.enable_project_scripting().unwrap();
    let report = standalone::run_headless(&mut game, &content, 3).unwrap();
    assert!(report.ok(), "{:?}", report.problems);
    assert_eq!(report.level.as_deref(), Some("scene/b.level"));
    assert_eq!(report.levels_loaded, ["scene/b.level"]);
}

#[test]
fn packaging_refuses_a_game_instance_without_the_trait() {
    let _guard = lock();
    let source = tempfile::tempdir().unwrap();
    project(source.path());
    write(
        &source.path().join("Pulsar/project.json"),
        r#"{ "startup_level": "", "game_instance": "classes/Prop.class" }"#,
    );
    let out = tempfile::tempdir().unwrap();
    let mut options = pulsar_package::PackageOptions::new(source.path(), out.path());
    options.loose = true;
    options.skip_build = true;
    let error = pulsar_package::package(&options).expect_err("Prop is no game instance");
    assert!(error.to_string().contains("Game Instance trait"), "{error}");
}

fn object_count(game: &TickLoop) -> usize {
    game.scene_store
        .read()
        .world
        .query::<&engine_backend::scene::StableId>()
        .count()
}

fn instance_ticks(game: &TickLoop) -> i64 {
    let driver = game.scripts.as_ref().unwrap().lock().unwrap();
    let entity = driver.game_instance().expect("a game instance runs");
    let id = driver.instance_of(entity).expect("its script runs");
    match driver.runtime().variable(id, "ticks") {
        Some(pulsar_script_vm::Value::Int(ticks)) => *ticks,
        other => panic!("ticks is an int: {other:?}"),
    }
}

/// A game instance that only counts its ticks (the switching one would ask for
/// other levels meanwhile).
fn counting_graph() -> Value {
    let mut g = Graph::default();
    g.event("tick", "on_tick", &[("delta_time", "f64")]);
    g.increment("counted", "ticks");
    g.then("tick", "Body", "counted");
    g.blueprint(&[("ticks", "i64", "0")], &["pulsar.GameInstance"])
}

/// A host that is not blocking (the windowed game, Play-in-Editor): the level
/// is read on a worker while the game goes on, then hydrates over several
/// ticks, during which nothing of gameplay runs.
#[test]
fn gameplay_waits_for_a_whole_level() {
    let _guard = lock();
    let source = tempfile::tempdir().unwrap();
    project(source.path());
    // A level big enough that a 4 ms budget spans ticks.
    const OBJECTS: usize = 30_000;
    write(
        &source.path().join("classes/Main.class/graph_save.json"),
        &counting_graph().to_string(),
    );
    let objects: Vec<Value> = (0..OBJECTS)
        .map(|i| object(&format!("o{i}"), 0.0))
        .collect();
    write(
        &source.path().join("scene/big.level"),
        &level(objects, json!({}), json!({})),
    );
    let out = tempfile::tempdir().unwrap();
    package_loose(source.path(), out.path());
    let content = ContentRoot::open(out.path().join("Content")).unwrap();
    content.install();
    standalone::prepare_engine(&content).unwrap();
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    game.enable_project_scripting().unwrap();
    standalone::start_game(&mut game, &content);
    for _ in 0..3 {
        game.tick_once();
    }
    game.request_level("scene/big.level");
    let started = std::time::Instant::now();
    let (mut partial_ticks, mut preparing_ticks) = (0, 0);
    let mut last = instance_ticks(&game);
    while game.level_load_pending() {
        assert!(started.elapsed().as_secs() < 60, "the level never loaded");
        game.tick_once();
        let ticks = instance_ticks(&game);
        let partial = game.current_level().is_none() && object_count(&game) > 1;
        if partial {
            partial_ticks += 1;
            assert_eq!(ticks, last, "no script ticks on a partial level");
        } else if game.level_load_pending() {
            preparing_ticks += 1;
        }
        last = ticks;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        partial_ticks > 1,
        "hydration spanned ticks: {partial_ticks}"
    );
    assert!(preparing_ticks > 0, "the game ran while the file was read");
    assert_eq!(game.current_level(), Some("scene/big.level"));
    assert_eq!(object_count(&game), OBJECTS + 1);
    game.tick_once();
    assert!(instance_ticks(&game) > last, "gameplay runs again");
    game.end_session();
}

// ── Play-in-Editor ──────────────────────────────────────────────────────────

fn level_json(world: &pulsar_scenedb::World, root: &Path, path: &Path) -> Value {
    let registry = pulsar_class::ClassRegistry::scan(root);
    let snapshot = ui_level_editor::scene_edit::level_io::snapshot_level(
        world,
        &registry,
        None,
        Default::default(),
    );
    ui_level_editor::scene_edit::level_io::write_level(snapshot, path).unwrap();
    let mut value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("metadata");
    value
}

#[test]
fn play_in_editor_switches_levels_and_stop_restores() {
    let _guard = lock();
    let source = tempfile::tempdir().unwrap();
    let root: PathBuf = source.path().to_path_buf();
    project(&root);
    // PIE plays the project: compile its classes first, as Play does.
    let compiled_out = tempfile::tempdir().unwrap();
    package_loose(&root, compiled_out.path());
    let content = ContentRoot::project(root.clone());
    content.install();
    standalone::prepare_engine(&content).unwrap();

    // The editor has level a open, with an unsaved object.
    let mut editor = ui_level_editor::LevelEditorState::new();
    let shared = editor.scene.shared_scene();
    {
        let mut scene = shared.write();
        RuntimeLevel::load_into(&root.join("scene/a.level"), &mut scene.world).unwrap();
        scene
            .world
            .spawn_object(engine_backend::scene::SpawnObject {
                stable_id: Some("unsaved".into()),
                ..engine_backend::scene::SpawnObject::new("unsaved")
            })
            .unwrap();
    }
    let scratch = tempfile::tempdir().unwrap();
    let before = level_json(
        &shared.read().world,
        &root,
        &scratch.path().join("before.level"),
    );

    // Play: the editor snapshots its world and writes it for the game.
    editor.scene.enter_play_mode();
    let play_level = root.join("target/pie/play.level");
    let _ = level_json(&shared.read().world, &root, &play_level);

    let mut tick_loop = TickLoop::with_scene_store(
        Arc::clone(&shared),
        TickMode::Fixed { dt: HEADLESS_STEP },
        1,
    );
    tick_loop.enable_project_scripting().unwrap();
    tick_loop.load_world_roles(&play_level, &root);
    tick_loop.set_level_source("scene/a.level", play_level.clone());
    tick_loop.set_blocking_level_loads(true);
    let mut session = pulsar_game::embed::PieSession::new(tick_loop, Some("scene/a.level".into()));
    // The open level plays, after the game instance (not the default level).
    for _ in 0..3 {
        session.tick();
    }
    let playing = snapshot(&session.tick_loop);
    assert_eq!(playing.level.as_deref(), Some("scene/a.level"));
    assert!(
        playing.objects.contains(&"unsaved".to_string()),
        "{playing:?}"
    );
    assert!(playing.classes.contains(&"Main".to_string()), "{playing:?}");

    // Another level, then the open one again: the unsaved object is there,
    // so it came from the play snapshot, not the saved file.
    session.tick_loop.request_level("scene/b.level");
    session.tick();
    let b = snapshot(&session.tick_loop);
    assert_eq!(b.objects, ["b_body", "pulsar_game_instance"], "{b:?}");
    session.tick_loop.request_level("scene/a.level");
    session.tick();
    session.tick();
    let again = snapshot(&session.tick_loop);
    assert!(again.objects.contains(&"unsaved".to_string()), "{again:?}");
    assert_eq!(again.bodies, (1, 1), "{again:?}");

    // The game instance's own script drives it on to Quit (tick 23).
    let quit = pulsar_pie_abi::control::QUIT_REQUESTED;
    for _ in 0..60 {
        if session.control(quit, 0) == 1 {
            break;
        }
        session.tick();
    }
    assert_eq!(
        session.control(quit, 0),
        1,
        "Quit Game is reported to the host"
    );

    session.shutdown();
    drop(session);
    editor.scene.exit_play_mode();
    let after = level_json(
        &shared.read().world,
        &root,
        &scratch.path().join("after.level"),
    );
    assert_eq!(after, before, "Stop restores the editor's world");
    let scene = shared.read();
    assert_eq!(
        scene
            .world
            .query::<&pulsar_game::scripting::GameInstanceEntity>()
            .count(),
        0
    );
    assert!(scene.world.entity_for("pulsar_game_instance").is_none());
}

// ── The Blueprint editor ────────────────────────────────────────────────────

/// The `game::` nodes are in every Blueprint's palette, and the Load Level and
/// Set Input Mode pins name reflected types whose property editors are
/// registered (the Blueprint pin editor uses them).
#[test]
fn the_game_nodes_are_in_the_palette_with_pin_editors() {
    let natives = pulsar_script_vm::NativeRegistry::with_engine_natives();
    let nodes = blueprint_compiler::palette::native_nodes(&natives);
    for (node_type, title) in [
        ("native::game::load_level", "Load Level"),
        ("native::game::set_input_mode", "Set Input Mode"),
        ("native::game::quit", "Quit Game"),
        ("native::game::instance", "Get Game Instance"),
    ] {
        let node = nodes
            .iter()
            .find(|node| node.node_type == node_type)
            .unwrap_or_else(|| panic!("{node_type} is in the palette"));
        assert_eq!(node.name, title);
        assert_eq!(node.category, "Game", "{node_type}");
    }
    for (native, pin, editor_type) in [
        ("game::load_level", "level", "LevelPath"),
        ("game::set_input_mode", "mode", "InputModeName"),
    ] {
        let function = natives.get(native).expect("a native");
        assert_eq!(function.attr(&format!("pin_type.{pin}")), Some(editor_type));
        let info = pulsar_reflection::RUNTIME_TYPE_REGISTRY
            .get_by_name(editor_type)
            .unwrap_or_else(|| panic!("{editor_type} is a reflected type"));
        assert!(
            ui_common::PROPERTY_EDITOR_REGISTRY.has(info.type_id),
            "{editor_type} has a property editor"
        );
        // The literal the pin stores reads back as the editor's value.
        pulsar_reflection::RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(info, json!("scene/a.level"))
            .expect("a string literal");
    }
    let engine_trait = pulsar_traits::builtin::TRAIT_FILES
        .iter()
        .any(|file| file.path == "traits/game_instance.trait.json");
    assert!(engine_trait);
    assert!(
        pulsar_traits::roles::roles()
            .iter()
            .all(|role| role.trait_id != pulsar_traits::builtin::GAME_INSTANCE_TRAIT_ID),
        "the game instance is not a level role (World Settings does not list it)"
    );
}
