//! Task list, status enum, and inter-thread event type for the loading sequence.

pub(crate) const TASKS: &[(&str, u64)] = &[
    ("Initializing renderer", 1200),
    ("Loading project data", 1000),
    ("Starting Rust Analyzer", 1300),
    ("Resolving workspace packages", 1100),
    ("Indexing source files", 1400),
    ("Building symbol database", 1250),
    ("Loading editor configuration", 950),
    ("Spawning asset pipeline", 1150),
    ("Compiling shader cache", 1350),
    ("Hydrating scene graph", 1050),
    ("Connecting language server", 1200),
    ("Finalizing workspace", 1100),
];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TaskStatus {
    Pending,
    Running,
    Done,
}

#[derive(Debug)]
pub(crate) enum LoadingEvent {
    TaskDone(usize),
}
