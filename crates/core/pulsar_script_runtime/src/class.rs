//! A linked script class and one live instance of it.

use std::sync::Arc;

use pulsar_scenedb::Entity;
use pulsar_script_vm::{
    CapabilityPolicy, Continuation, DebugSnapshot, Debugger, EventCatalog, FuncId, Instance,
    Latent, Module, NativeRegistry, Program, Type, Wake,
};

use crate::RuntimeError;

pub const BEGIN_PLAY: &str = "begin_play";
pub const TICK: &str = "tick";
pub const END_PLAY: &str = "end_play";

pub(crate) struct Entries {
    pub(crate) begin_play: Option<FuncId>,
    pub(crate) tick: Option<FuncId>,
    pub(crate) end_play: Option<FuncId>,
}

pub(crate) struct Class {
    pub(crate) program: Program,
    pub(crate) entries: Entries,
}

impl Class {
    pub(crate) fn link(
        module: Arc<Module>,
        natives: &NativeRegistry,
        events: Option<&dyn EventCatalog>,
        policy: &CapabilityPolicy,
    ) -> Result<Self, RuntimeError> {
        let class = module.name.clone();
        let program = Program::link_with_policy(Arc::clone(&module), natives, events, policy)
            .map_err(|source| {
                let site = module.locate_link_error(&source);
                RuntimeError::Link {
                    class: class.clone(),
                    source,
                    site,
                }
            })?;
        let entry = |name: &'static str, params: &[Type], expected: &'static str| match program
            .module()
            .function(name)
        {
            Some((index, f)) if f.exported => {
                if f.params == params && f.ret == Type::Unit {
                    Ok(Some(FuncId(index)))
                } else {
                    Err(RuntimeError::BadEntryPoint {
                        class: class.clone(),
                        name,
                        expected,
                    })
                }
            }
            _ => Ok(None),
        };
        let entries = Entries {
            begin_play: entry(BEGIN_PLAY, &[], "fn begin_play()")?,
            tick: entry(TICK, &[Type::Float], "fn tick(delta_time: float)")?,
            end_play: entry(END_PLAY, &[], "fn end_play()")?,
        };
        Ok(Self { program, entries })
    }
}

/// One live instance of a class.
pub(crate) struct ScriptInstance {
    pub(crate) class: String,
    pub(crate) state: Instance,
    pub(crate) entity: Option<Entity>,
    /// Lifetime instructions executed by this instance, including calls
    /// that returned an error or suspended on a wait.
    pub(crate) instructions_executed: u64,
    /// Suspended calls and the game time each resumes at.
    pub(crate) waiting: Vec<(f64, Continuation)>,
    /// Calls suspended by a latent native (`wait::frames`, `wait::until`,
    /// `wait::event`), with what each waits for.
    pub(crate) blocked: Vec<(Wake, Continuation)>,
    /// Timers and the pending wake request of the running call.
    pub(crate) latent: Latent,
    /// Calls stopped by the debugger. These resume only on an explicit
    /// debugger command, never from the regular tick loop.
    pub(crate) paused: Vec<(DebugSnapshot, Continuation)>,
    /// Breakpoints and stepping state belong to this object, even though
    /// the runtime reuses one VM for all instances.
    pub(crate) debugger: Debugger,
}
