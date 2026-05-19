/// Blueprint executor backed by the native `pulsar_std` cdylib.
///
/// `BpExecutor::prepare` resolves every `__bp_dispatch_<name>` symbol from the
/// loaded library and patches the address directly into `Instruction::Call::fn_ptr`
/// inside the program. After that, `pbgc::vm::run(&program)` executes with zero
/// table lookups — each Call is one `transmute` + one direct function call.
pub use libloading;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ExecutorError {
    Dylib(libloading::Error),
    MissingSymbol(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::Dylib(e) => write!(f, "dylib error: {}", e),
            ExecutorError::MissingSymbol(s) => write!(f, "missing symbol: {}", s),
        }
    }
}
impl std::error::Error for ExecutorError {}
impl From<libloading::Error> for ExecutorError {
    fn from(e: libloading::Error) -> Self {
        ExecutorError::Dylib(e)
    }
}

// ── BpExecutor ────────────────────────────────────────────────────────────────

pub struct BpExecutor {
    _lib: libloading::Library,
}

impl BpExecutor {
    /// Load the native `pulsar_std` cdylib from `path`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use pulsar_bp_executor::BpExecutor;
    /// use pulsar_std_bundle::extract_to_tempfile;
    ///
    /// let tmp = extract_to_tempfile().unwrap();
    /// let executor = BpExecutor::load(&tmp.path).unwrap();
    /// ```
    pub fn load(path: &std::path::Path) -> Result<Self, ExecutorError> {
        let lib = unsafe { libloading::Library::new(path)? };
        Ok(Self { _lib: lib })
    }

    /// Patch `fn_ptr` in every `Instruction::Call` by resolving
    /// `__bp_dispatch_<node_type>` from the native lib.
    ///
    /// After this call `pbgc::vm::run(&program)` needs no dispatch table.
    /// Call once per program after loading or deserializing.
    ///
    /// # Safety
    ///
    /// The raw function pointers written into `program` are valid only while
    /// this `BpExecutor` (and the `TempLib` that backs it) remains alive.
    /// Dropping the executor — or the `TempLib` it was loaded from — before
    /// calling `pbgc::vm::run(&program)` results in dangling pointers and
    /// undefined behaviour. Keep the executor alive at least until the program
    /// finishes executing.
    pub fn prepare(&self, program: &mut pbgc::BpProgram) -> Result<(), ExecutorError> {
        use pbgc::Instruction;
        for instr in &mut program.instructions {
            if let Instruction::Call {
                fn_ptr, node_type, ..
            } = instr
            {
                // Build a NUL-terminated key for libloading, but keep a clean
                // copy without the NUL for use in error messages.
                let display_name = format!("__bp_dispatch_{}", node_type);
                let lookup_key = format!("{}\0", display_name);
                let ptr: libloading::Symbol<pbgc::DispatchFn> = unsafe {
                    self._lib
                        .get(lookup_key.as_bytes())
                        .map_err(|_| ExecutorError::MissingSymbol(display_name))?
                };
                *fn_ptr = *ptr as usize as u64;
            }
        }
        Ok(())
    }
}
