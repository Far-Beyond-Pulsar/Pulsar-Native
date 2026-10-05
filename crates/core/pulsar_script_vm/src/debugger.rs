//! Language-neutral controls and snapshots for an attached script debugger.
//!
//! Breakpoints use function names and bytecode pcs. Frontends can resolve
//! source locations through [`crate::Function::location`] before installing
//! one, keeping editor concepts out of the VM.

use std::collections::HashSet;

use crate::{SourceLoc, Value};

/// A stable breakpoint key understood by any script frontend.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Breakpoint {
    pub function: String,
    pub pc: usize,
}

/// Requested execution behavior after a debugger stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugCommand {
    Continue,
    StepInto,
    StepOver,
    StepOut,
}

/// Why execution stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    Step,
}

/// A register and its current value in one script frame.
#[derive(Clone, Debug)]
pub struct RegisterSnapshot {
    pub index: usize,
    pub ty: String,
    pub value: Value,
}

/// One frame in the language-neutral call stack, outermost first.
#[derive(Clone, Debug)]
pub struct FrameSnapshot {
    pub function: String,
    pub pc: usize,
    pub location: Option<SourceLoc>,
    pub registers: Vec<RegisterSnapshot>,
    /// Values corresponding to source output pins, if the frontend supplied
    /// a register-to-pin map in the function's debug info.
    pub output_values: Vec<OutputValueSnapshot>,
}

/// One currently visible graph output value.
#[derive(Clone, Debug)]
pub struct OutputValueSnapshot {
    pub node: String,
    pub pin: String,
    pub register: usize,
    pub value: Value,
}

/// State at a stopped instruction, suitable for an editor or future DAP adapter.
#[derive(Clone, Debug)]
pub struct DebugSnapshot {
    pub reason: StopReason,
    pub call_stack: Vec<FrameSnapshot>,
    pub instance_variables: Vec<(String, Value)>,
}

/// Breakpoints and stepping state for one VM execution stream.
pub struct Debugger {
    breakpoints: HashSet<Breakpoint>,
    command: DebugCommand,
    step_origin: Option<(String, usize, usize, Option<SourceLoc>)>,
    skip_breakpoint_once: bool,
}

impl Default for Debugger {
    fn default() -> Self {
        Self {
            breakpoints: HashSet::new(),
            command: DebugCommand::Continue,
            step_origin: None,
            skip_breakpoint_once: false,
        }
    }
}

impl Debugger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_breakpoint(&mut self, function: impl Into<String>, pc: usize) {
        self.breakpoints.insert(Breakpoint {
            function: function.into(),
            pc,
        });
    }

    pub fn set_source_breakpoint(
        &mut self,
        module: &crate::Module,
        function: &str,
        location: &SourceLoc,
    ) -> usize {
        let Some((_, function_def)) = module.function(function) else {
            return 0;
        };
        let pcs = function_def
            .debug
            .as_ref()
            .into_iter()
            .flat_map(|debug| debug.ranges.iter())
            .filter(|range| range.loc == *location)
            .map(|range| range.start as usize)
            .collect::<Vec<_>>();
        for pc in &pcs {
            self.set_breakpoint(function, *pc);
        }
        pcs.len()
    }

    /// Set breakpoints on every instruction emitted from a graph node.
    pub fn set_node_breakpoint(
        &mut self,
        module: &crate::Module,
        function: &str,
        file: &str,
        node: &str,
    ) -> usize {
        let location = SourceLoc::node(file, node);
        self.set_source_breakpoint(module, function, &location)
    }

    pub fn remove_breakpoint(&mut self, function: &str, pc: usize) -> bool {
        self.breakpoints.remove(&Breakpoint {
            function: function.into(),
            pc,
        })
    }

    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
    }

    pub fn breakpoints(&self) -> impl Iterator<Item = &Breakpoint> {
        self.breakpoints.iter()
    }

    /// Resume a stopped VM. Stepping stops at the next source location when
    /// available, otherwise at the next instruction boundary.
    pub fn command(&mut self, command: DebugCommand, stopped: &DebugSnapshot) {
        self.command = command;
        let top = stopped.call_stack.last();
        self.step_origin = top.map(|frame| {
            (
                frame.function.clone(),
                frame.pc,
                stopped.call_stack.len(),
                frame.location.clone(),
            )
        });
        self.skip_breakpoint_once = true;
    }

    pub(crate) fn should_stop(
        &mut self,
        function: &str,
        pc: usize,
        depth: usize,
        location: Option<&SourceLoc>,
    ) -> Option<StopReason> {
        let key = Breakpoint {
            function: function.into(),
            pc,
        };
        let at_breakpoint = self.breakpoints.contains(&key);
        if self.skip_breakpoint_once {
            self.skip_breakpoint_once = false;
        } else if at_breakpoint {
            self.command = DebugCommand::Continue;
            self.step_origin = None;
            return Some(StopReason::Breakpoint);
        }
        if self.command == DebugCommand::Continue {
            return None;
        }
        let Some((origin_function, origin_pc, origin_depth, origin_location)) = &self.step_origin
        else {
            return Some(StopReason::Step);
        };
        let changed_location = match (location, origin_location) {
            (Some(current), Some(origin)) => current != origin,
            _ => function != origin_function || pc != *origin_pc,
        };
        let stop = match self.command {
            DebugCommand::Continue => false,
            DebugCommand::StepInto => {
                changed_location || function != origin_function || depth > *origin_depth
            }
            DebugCommand::StepOver => depth <= *origin_depth && changed_location,
            DebugCommand::StepOut => depth < *origin_depth,
        };
        if stop {
            self.command = DebugCommand::Continue;
            self.step_origin = None;
            Some(StopReason::Step)
        } else {
            None
        }
    }
}
