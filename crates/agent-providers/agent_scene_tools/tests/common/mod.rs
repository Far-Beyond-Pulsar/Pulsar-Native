//! Shared fixtures: an in-memory SceneDB scene, a registered test component
//! class and a tool registry over it.

#![allow(dead_code)]

use std::sync::Arc;

use agent_chat_tools::{set_run_scope, AgentRunScope, ToolContext, ToolRegistry};
use agent_scene_tools::{
    register_tools, FixedScene, SceneDiff, SceneImage, SceneSource, SceneToolbox, SharedScene,
};
use engine_class_derive::{engine_class, register_world_component};
use pulsar_scenedb::{Entity, World};
use serde_json::{json, Value};

#[engine_class(category = "Test", default, clone, debug, serialize, deserialize)]
pub struct Lamp {
    #[property]
    pub intensity: f32,
    #[property]
    pub label: String,
    #[property]
    pub on: bool,
}

#[register_world_component]
impl Lamp {}

/// Rejects a Lamp whose label is "forbidden", like a level rule would.
pub struct StrictScene {
    pub inner: FixedScene,
}

impl SceneSource for StrictScene {
    fn scene(&self) -> Result<SharedScene, String> {
        Ok(self.inner.handle())
    }

    fn check_instance(&self, world: &World, instance: Entity) -> Result<(), String> {
        match world.get::<Lamp>(instance) {
            Some(lamp) if lamp.label == "forbidden" => {
                Err("a level cannot have a forbidden lamp".to_string())
            }
            _ => Ok(()),
        }
    }
}

pub struct Rig {
    pub tb: Arc<SceneToolbox>,
    pub scene: SharedScene,
    pub registry: ToolRegistry,
}

pub fn rig() -> Rig {
    let fixed = FixedScene::new();
    let scene = fixed.handle();
    rig_with(Arc::new(fixed), scene)
}

pub fn strict_rig() -> Rig {
    let inner = FixedScene::new();
    let scene = inner.handle();
    rig_with(Arc::new(StrictScene { inner }), scene)
}

fn rig_with(source: Arc<dyn SceneSource>, scene: SharedScene) -> Rig {
    let tb = SceneToolbox::new(source);
    let mut registry = ToolRegistry::new();
    register_tools(&mut registry, &tb);
    Rig { tb, scene, registry }
}

pub fn scope(run_id: &str, sandbox: bool) -> ToolContext {
    let mut ctx = ToolContext::new();
    set_run_scope(
        &mut ctx,
        AgentRunScope {
            run_id: run_id.to_string(),
            sandbox,
            sandbox_mode: None,
        },
    );
    ctx
}

/// A tool context for a sandbox run whose harness chose mode.
pub fn scope_mode(run_id: &str, mode: Option<&str>) -> ToolContext {
    let mut ctx = ToolContext::new();
    set_run_scope(
        &mut ctx,
        AgentRunScope {
            run_id: run_id.to_string(),
            sandbox: true,
            sandbox_mode: mode.map(str::to_string),
        },
    );
    ctx
}

impl Rig {
    /// Like call_sandboxed, with the harness's own mode choice.
    pub fn call_mode(&self, tool: &str, args: Value, run: &str, mode: Option<&str>) -> anyhow::Result<Value> {
        self.registry.execute(tool, args, &scope_mode(run, mode))
    }

    pub fn call(&self, tool: &str, args: Value, run: &str) -> anyhow::Result<Value> {
        self.registry.execute(tool, args, &scope(run, false))
    }

    pub fn call_sandboxed(&self, tool: &str, args: Value, run: &str) -> anyhow::Result<Value> {
        self.registry.execute(tool, args, &scope(run, true))
    }

    /// Like `call`, expecting success.
    pub fn ok(&self, tool: &str, args: Value, run: &str) -> Value {
        self.call(tool, args, run)
            .unwrap_or_else(|e| panic!("{tool} failed: {e}"))
    }

    pub fn err(&self, tool: &str, args: Value, run: &str) -> String {
        match self.call(tool, args, run) {
            Ok(v) => panic!("{tool} should have failed, got {v}"),
            Err(e) => e.to_string(),
        }
    }

    pub fn image(&self) -> SceneImage {
        SceneImage::capture(&self.scene.read().world)
    }

    pub fn ids(&self) -> Vec<String> {
        self.image().order
    }

    /// Assert the live scene equals `before`.
    pub fn assert_unchanged_from(&self, before: &SceneImage) {
        let diff = SceneDiff::compute(before, &self.image());
        assert!(diff.is_empty(), "scene changed: {}", diff.render_text("t", &[]));
    }

    /// A small level: `root` (with a Lamp) and its child `child`.
    pub fn seed(&self) {
        self.ok(
            "scene_write_spawn_object",
            json!({"name": "Root", "id": "root", "kind": "folder",
                   "components": [{"class_name": "Lamp", "properties": {"intensity": 1.0, "label": "seed"}}]}),
            "seed",
        );
        self.ok(
            "scene_write_spawn_object",
            json!({"name": "Child", "id": "child", "parent_id": "root", "position": [1.0, 0.0, 0.0],
                   "components": [{"class_name": "Lamp"}]}),
            "seed",
        );
    }
}
