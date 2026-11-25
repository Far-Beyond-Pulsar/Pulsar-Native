//! Writes the default player character's Blueprint
//! (`plugins/vendor/gameplay/assets/classes/PlayerCharacter.class/graph_save.json`).
//!
//! The graph is laid out in five commented sections (trait, move, jump, sprint,
//! look); this keeps every node, pin and wire consistent and the layout on a grid.
//! Regenerate with:
//!
//! ```text
//! REGENERATE_PLAYER_CHARACTER=1 cargo test -p installed_plugins --test regenerate_player_character_graph
//! ```

use installed_plugins as _;
use serde_json::{json, Map, Value};

// ---- the file format -------------------------------------------------------------

fn data(base: &str) -> Value {
    json!({ "Data": { "base_type": base, "wrappers": [], "is_wildcard": false } })
}

fn exec() -> Value {
    json!("Execution")
}

struct PinSpec {
    id: String,
    name: String,
    ty: Value,
}

fn pin(id: &str, ty: Value) -> PinSpec {
    PinSpec {
        id: id.into(),
        name: id.into(),
        ty,
    }
}

fn named(id: &str, name: &str, ty: Value) -> PinSpec {
    PinSpec {
        id: id.into(),
        name: name.into(),
        ty,
    }
}

struct NodeSpec {
    id: String,
    node_type: String,
    at: (f64, f64),
    props: Map<String, Value>,
    inputs: Vec<PinSpec>,
    outputs: Vec<PinSpec>,
}

struct Graph {
    nodes: Vec<NodeSpec>,
    edges: Vec<(String, String, String, String, bool)>,
    comments: Vec<Value>,
}

impl Graph {
    fn node(
        &mut self,
        id: &str,
        node_type: &str,
        at: (f64, f64),
        props: Value,
        inputs: Vec<PinSpec>,
        outputs: Vec<PinSpec>,
    ) {
        self.nodes.push(NodeSpec {
            id: id.into(),
            node_type: node_type.into(),
            at,
            props: props.as_object().cloned().unwrap_or_default(),
            inputs,
            outputs,
        });
    }

    fn exec(&mut self, from: &str, pin: &str, to: &str) {
        self.edges
            .push((from.into(), pin.into(), to.into(), "exec".into(), true));
    }

    fn wire(&mut self, from: &str, pin: &str, to: &str, to_pin: &str) {
        self.edges
            .push((from.into(), pin.into(), to.into(), to_pin.into(), false));
    }

    fn comment(
        &mut self,
        id: &str,
        text: &str,
        at: (f64, f64),
        size: (f64, f64),
        color: [f64; 4],
        contained: &[&str],
    ) {
        self.comments.push(json!({
            "id": id,
            "text": text,
            "position": [at.0, at.1],
            "size": [size.0, size.1],
            "color": color,
            "contained_node_ids": contained,
        }));
    }

    fn render(&self) -> Value {
        let mut connection_ids: Vec<String> = Vec::new();
        let mut connections = Vec::new();
        for (index, (from, pin, to, to_pin, is_exec)) in self.edges.iter().enumerate() {
            let id = format!("c{}", index + 1);
            connection_ids.push(id.clone());
            connections.push(json!({
                "id": id,
                "source_node": from,
                "source_pin": pin,
                "target_node": to,
                "target_pin": to_pin,
                "connection_type": if *is_exec { "Execution" } else { "Data" },
            }));
        }
        let wires_of = |node: &str, pin: &str, output: bool| -> Vec<String> {
            self.edges
                .iter()
                .zip(&connection_ids)
                .filter(|((from, from_pin, to, to_pin, _), _)| {
                    if output {
                        from == node && from_pin == pin
                    } else {
                        to == node && to_pin == pin
                    }
                })
                .map(|(_, id)| id.clone())
                .collect()
        };
        let mut nodes = Map::new();
        for node in &self.nodes {
            let pins = |specs: &[PinSpec], kind: &str| -> Vec<Value> {
                specs
                    .iter()
                    .map(|p| {
                        json!({
                            "id": p.id,
                            "name": p.name,
                            "pin_type": kind,
                            "data_type": p.ty,
                            "connected_to": wires_of(&node.id, &p.id, kind == "Output"),
                        })
                    })
                    .collect()
            };
            nodes.insert(
                node.id.clone(),
                json!({
                    "id": node.id,
                    "node_type": node.node_type,
                    "position": { "x": node.at.0, "y": node.at.1 },
                    "properties": node.props,
                    "inputs": pins(&node.inputs, "Input"),
                    "outputs": pins(&node.outputs, "Output"),
                }),
            );
        }
        json!({
            "nodes": nodes,
            "connections": connections,
            "metadata": {
                "name": "EventGraph",
                "description": "The default player character.",
                "version": "1.0.0",
                "created_at": "2026-11-19T00:00:00Z",
                "modified_at": "2026-11-19T00:00:00Z"
            },
            "comments": self.comments,
            "custom_event_defs": {},
        })
    }
}

// ---- node recipes -------------------------------------------------------------------

/// The trait's custom event `name` (an empty handler).
fn trait_event(g: &mut Graph, id: &str, name: &str, at: (f64, f64), field: Option<(&str, &str)>) {
    let mut outputs = vec![pin("Body", exec())];
    if let Some((field, ty)) = field {
        outputs.push(pin(field, data(ty)));
    }
    g.node(
        id,
        &format!("custom_event:{name}"),
        at,
        json!({ "event_uid": name }),
        vec![pin("__return__", data("wildcard"))],
        outputs,
    );
}

const PHASES: [(&str, &str); 5] = [
    ("Started", "started"),
    ("Ongoing", "ongoing"),
    ("Triggered", "triggered"),
    ("Completed", "completed"),
    ("Canceled", "canceled"),
];

/// An "On <action>" node of the Player Input component's mapping: every phase an
/// exec output, the action's typed value, and the shared timing pins.
fn on_action(
    g: &mut Graph,
    id: &str,
    action: &str,
    action_id: &str,
    value: (&str, &str, &str),
    at: (f64, f64),
) {
    let (pin_name, pin_type, lowering) = value;
    let events = PHASES
        .iter()
        .map(|(pin, word)| format!("{pin}=input::{action}.{word}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut outputs: Vec<PinSpec> = PHASES.iter().map(|(pin, _)| pin_exec(pin)).collect();
    outputs.push(pin(pin_name, data(pin_type)));
    outputs.push(pin("Elapsed", data("f64")));
    outputs.push(pin("TriggeredFor", data("f64")));
    outputs.push(pin("Entity", data("Entity")));
    g.node(
        id,
        &format!("event::family::input.action:{action_id}"),
        at,
        json!({
            "action": action_id,
            "phases": PHASES.iter().map(|(_, w)| *w).collect::<Vec<_>>().join(","),
            "events": events,
            "lowering": format!("{pin_name}={lowering};Elapsed=f64:elapsed;TriggeredFor=f64:triggered_for;Entity=entity:entity"),
            "family": "input.action",
            "title": format!("On {action}"),
            "component_type": "PlayerInputComponent",
            "scope": "self",
        }),
        vec![pin("component_ref", data("PlayerInputComponent"))],
        outputs,
    );
}

fn pin_exec(id: &str) -> PinSpec {
    pin(id, exec())
}

/// A call of a method of the character component on this object: exec in and out.
fn character_call(
    g: &mut Graph,
    id: &str,
    class: &str,
    method: &str,
    at: (f64, f64),
    params: Vec<PinSpec>,
    props: Value,
) {
    let mut inputs = vec![pin_exec("exec"), pin("component_ref", data(class))];
    inputs.extend(params);
    g.node(
        id,
        &format!("comp_call::{class}::{method}"),
        at,
        props,
        inputs,
        vec![pin_exec("exec_out")],
    );
}

/// A pure value-type operation: `native::Type::op`.
fn pure_native(
    g: &mut Graph,
    id: &str,
    name: &str,
    at: (f64, f64),
    params: Vec<PinSpec>,
    ret: &str,
    props: Value,
) {
    g.node(
        id,
        &format!("native::{name}"),
        at,
        props,
        params,
        vec![pin("result", data(ret))],
    );
}

fn getter(g: &mut Graph, id: &str, variable: &str, at: (f64, f64)) {
    g.node(
        id,
        &format!("get_{variable}"),
        at,
        json!({}),
        vec![],
        vec![named("value", variable, data("f64"))],
    );
}

fn build() -> Value {
    let mut g = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        comments: Vec::new(),
    };
    let yellow = [0.9, 0.75, 0.2, 0.28];
    let blue = [0.25, 0.45, 0.75, 0.22];
    let green = [0.25, 0.65, 0.35, 0.22];
    let purple = [0.6, 0.35, 0.75, 0.22];
    let orange = [0.85, 0.5, 0.25, 0.22];
    let teal = [0.2, 0.65, 0.65, 0.22];

    // ── 1. The Player Character trait ──────────────────────────────────────────
    trait_event(&mut g, "spawned", "on_spawned", (0.0, 40.0), None);
    trait_event(
        &mut g,
        "possessed",
        "on_possessed",
        (0.0, 200.0),
        Some(("controller_id", "i64")),
    );
    trait_event(&mut g, "unpossessed", "on_unpossessed", (0.0, 400.0), None);

    // ── 2. Move: IA_Move ───────────────────────────────────────────────────────────
    on_action(
        &mut g,
        "mv",
        "IA_Move",
        "6c0d3e4a-1b52-4a8f-9d21-0e7f5a3c6b01",
        ("Value", "Vec2", "vec2:x,y"),
        (0.0, 780.0),
    );
    pure_native(
        &mut g,
        "mv_x",
        "Vec2::x",
        (640.0, 1100.0),
        vec![pin("v", data("Vec2"))],
        "f64",
        json!({}),
    );
    pure_native(
        &mut g,
        "mv_y",
        "Vec2::y",
        (640.0, 1220.0),
        vec![pin("v", data("Vec2"))],
        "f64",
        json!({}),
    );
    character_call(
        &mut g,
        "mv_set",
        "CharacterComponent",
        "set_move",
        (1180.0, 780.0),
        vec![pin("x", data("f32")), pin("y", data("f32"))],
        json!({}),
    );
    character_call(
        &mut g,
        "mv_stop",
        "CharacterComponent",
        "set_move",
        (1180.0, 1100.0),
        vec![pin("x", data("f32")), pin("y", data("f32"))],
        json!({ "x": 0.0, "y": 0.0 }),
    );
    for phase in ["Started", "Ongoing", "Triggered"] {
        g.exec("mv", phase, "mv_set");
    }
    for phase in ["Completed", "Canceled"] {
        g.exec("mv", phase, "mv_stop");
    }
    g.wire("mv", "Value", "mv_x", "v");
    g.wire("mv", "Value", "mv_y", "v");
    g.wire("mv_x", "result", "mv_set", "x");
    g.wire("mv_y", "result", "mv_set", "y");

    // ── 3. Jump: IA_Jump ───────────────────────────────────────────────────────────
    on_action(
        &mut g,
        "jp",
        "IA_Jump",
        "6c0d3e4a-1b52-4a8f-9d21-0e7f5a3c6b03",
        ("Pressed", "bool", "bool:value"),
        (0.0, 1720.0),
    );
    character_call(
        &mut g,
        "jp_do",
        "CharacterComponent",
        "request_jump",
        (640.0, 1720.0),
        vec![],
        json!({}),
    );
    g.exec("jp", "Triggered", "jp_do");

    // ── 4. Sprint: IA_Sprint ─────────────────────────────────────────────────────────
    on_action(
        &mut g,
        "sp",
        "IA_Sprint",
        "6c0d3e4a-1b52-4a8f-9d21-0e7f5a3c6b04",
        ("Pressed", "bool", "bool:value"),
        (0.0, 2300.0),
    );
    getter(&mut g, "sp_fast_speed", "sprint_speed", (640.0, 2300.0));
    getter(&mut g, "sp_slow_speed", "walk_speed", (640.0, 2560.0));
    for (id, at, source) in [
        ("sp_fast", (1180.0, 2300.0), "sp_fast_speed"),
        ("sp_slow", (1180.0, 2560.0), "sp_slow_speed"),
    ] {
        g.node(
            id,
            "comp_set_prop::CharacterComponent::walk_speed",
            at,
            json!({}),
            vec![
                pin_exec("exec"),
                pin("component_ref", data("CharacterComponent")),
                pin("value", data("f32")),
            ],
            vec![pin_exec("exec_out")],
        );
        g.wire(source, "value", id, "value");
    }
    for phase in ["Started", "Triggered"] {
        g.exec("sp", phase, "sp_fast");
    }
    for phase in ["Completed", "Canceled"] {
        g.exec("sp", phase, "sp_slow");
    }

    // ── 5. Look: IA_Look ───────────────────────────────────────────────────────────
    on_action(
        &mut g,
        "lk",
        "IA_Look",
        "6c0d3e4a-1b52-4a8f-9d21-0e7f5a3c6b02",
        ("Value", "Vec2", "vec2:x,y"),
        (0.0, 3120.0),
    );
    // The look value's two axes go to the Character component, which owns how far a unit
    // turns the view (its `look_degrees_per_unit`) and the pitch limits, so the frame's late
    // look turns the view by the same amount the tick does.
    pure_native(
        &mut g,
        "lk_x",
        "Vec2::x",
        (640.0, 3120.0),
        vec![pin("v", data("Vec2"))],
        "f64",
        json!({}),
    );
    pure_native(
        &mut g,
        "lk_y",
        "Vec2::y",
        (640.0, 3480.0),
        vec![pin("v", data("Vec2"))],
        "f64",
        json!({}),
    );
    character_call(
        &mut g,
        "lk_turn",
        "CharacterComponent",
        "add_look",
        (1400.0, 3120.0),
        vec![pin("x", data("f32")), pin("y", data("f32"))],
        json!({}),
    );
    for phase in ["Started", "Ongoing", "Triggered"] {
        g.exec("lk", phase, "lk_turn");
    }
    g.wire("lk", "Value", "lk_x", "v");
    g.wire("lk", "Value", "lk_y", "v");
    g.wire("lk_x", "result", "lk_turn", "x");
    g.wire("lk_y", "result", "lk_turn", "y");
    // ── Comments: the title, then one box per section ─────────────────────────────────
    g.comment(
        "title",
        "DEFAULT PLAYER CHARACTER\nShipped with the Gameplay plugin (read-only: copy the .class folder into your project to edit it).\nThe Player Input component below uses the IMC_Default mapping, whose actions IA_Move, IA_Look, IA_Jump and IA_Sprint\nraise the events these sections handle, on this object only. Every action node offers all of its phases as exec\noutputs; only the ones wired here are listened to. The sections run top to bottom: the trait, Move, Jump, Sprint, Look.",
        (-40.0, -330.0),
        (2900.0, 200.0),
        yellow,
        &[],
    );
    g.comment(
        "sec_trait",
        "1. PLAYER CHARACTER TRAIT\nThe engine's Player Character role calls these when the character is placed in the world and when a controller\ntakes or lets go of it. This character has nothing to do on them; they exist because the trait requires them.",
        (-40.0, -90.0),
        (1100.0, 640.0),
        blue,
        &["spawned", "possessed", "unpossessed"],
    );
    g.comment(
        "sec_move",
        "2. MOVE  (IA_Move: a 2D axis, X right and Y forward)\nWhile the keys or stick are held the axis is read into x and y and handed to the character, which walks that way\nin the direction it faces. Released or canceled, it stands still.",
        (-40.0, 640.0),
        (1780.0, 760.0),
        green,
        &["mv", "mv_x", "mv_y", "mv_set", "mv_stop"],
    );
    g.comment(
        "sec_jump",
        "3. JUMP  (IA_Jump: pressed once)\nAsks the character to jump. It jumps on the next frame its controller is on the ground, so a press in the air\nis not remembered once it lands.",
        (-40.0, 1540.0),
        (1380.0, 460.0),
        orange,
        &["jp", "jp_do"],
    );
    g.comment(
        "sec_sprint",
        "4. SPRINT  (IA_Sprint: held)\nWhile held the walk speed is the sprint speed; released or canceled it goes back to the walk speed. Both speeds are\nvariables of this class (Variables panel), so they are changed in one place.",
        (-40.0, 2140.0),
        (1780.0, 800.0),
        purple,
        &["sp", "sp_fast_speed", "sp_slow_speed", "sp_fast", "sp_slow"],
    );
    g.comment(
        "sec_look",
        "5. LOOK  (IA_Look: a 2D axis from the mouse or the right stick)\nBoth axes go to the Character component's CONTROL ROTATION (the controller's view direction), nothing else: the\nCharacter component turns it by its look_degrees_per_unit (yaw negative: moving right turns right; pitch negative\ninverts), the pitch held between view_pitch_min and view_pitch_max. The camera follows the control rotation and the move axes are walked relative to\nits yaw; whether the body turns with it (Use Controller Rotation Yaw) or toward its movement (Orient Rotation To\nMovement) are the Character component's own options.",
        (-40.0, 2980.0),
        (3000.0, 1500.0),
        teal,
        &[
            "lk",
            "lk_x",
            "lk_y",
            "lk_turn",
        ],
    );

    json!({
        "format_version": 2,
        "main_graph": g.render(),
        "subgraphs": [],
        "local_events": [
            { "uid": "on_spawned", "name": "on_spawned", "fields": [], "return_type": "" },
            {
                "uid": "on_possessed",
                "name": "on_possessed",
                "fields": [{ "name": "controller_id", "type_name": "i64" }],
                "return_type": ""
            },
            { "uid": "on_unpossessed", "name": "on_unpossessed", "fields": [], "return_type": "" }
        ],
        "variables": [
            {
                "id": "v_walk",
                "name": "walk_speed",
                "data_type": data("f64"),
                "default_value": "4.0",
                "description": "Metres per second at full input, restored when sprint ends."
            },
            {
                "id": "v_sprint",
                "name": "sprint_speed",
                "data_type": data("f64"),
                "default_value": "7.0",
                "description": "Metres per second while IA_Sprint is held."
            }
        ],
        "editor_state": {
            "open_tab_ids": ["main"],
            "active_tab_index": 0,
            "graph_view_states": { "main": { "pan_offset_x": 0.0, "pan_offset_y": 0.0, "zoom": 0.5 } }
        },
        "blueprint_metadata": {
            "blueprint_type": "Actor",
            "parent_class": null,
            "description": "The default player character: a character body, a camera and the player's input.",
            "category": "Gameplay",
            "tags": ["gameplay", "character", "input"],
            "implemented_trait_ids": ["pulsar.PlayerCharacter"]
        }
    })
}

#[test]
fn regenerate_player_character_graph() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/gameplay/assets/classes/PlayerCharacter.class/graph_save.json");
    let header = "// The default Player Character, shipped with the Gameplay plugin.\n\
// A level selects it in World Settings > Player Character. The file is read-only\n\
// (a plugin folder); copy the .class folder into your project to edit it. It is\n\
// written by plugins/installed/tests/regenerate_player_character_graph.rs.\n";
    let text = format!(
        "{header}{}\n",
        serde_json::to_string_pretty(&build()).unwrap()
    );
    if std::env::var("REGENERATE_PLAYER_CHARACTER").is_ok() {
        std::fs::write(&path, text).unwrap();
        return;
    }
    // Without the switch the checked-in file must be what the generator makes.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        on_disk.replace("\r\n", "\n"),
        text,
        "the checked-in graph is stale: run with REGENERATE_PLAYER_CHARACTER=1"
    );
}
