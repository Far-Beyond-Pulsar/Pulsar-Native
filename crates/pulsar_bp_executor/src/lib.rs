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
            ExecutorError::Dylib(e)         => write!(f, "dylib error: {}", e),
            ExecutorError::MissingSymbol(s) => write!(f, "missing symbol: {}", s),
        }
    }
}
impl std::error::Error for ExecutorError {}
impl From<libloading::Error> for ExecutorError {
    fn from(e: libloading::Error) -> Self { ExecutorError::Dylib(e) }
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
    pub fn prepare(&self, program: &mut pbgc::BpProgram) -> Result<(), ExecutorError> {
        use pbgc::Instruction;
        for instr in &mut program.instructions {
            if let Instruction::Call { fn_ptr, node_type, .. } = instr {
                let sym_name = format!("__bp_dispatch_{}\0", node_type);
                let ptr: libloading::Symbol<pbgc::DispatchFn> = unsafe {
                    self._lib.get(sym_name.as_bytes())
                        .map_err(|_| ExecutorError::MissingSymbol(sym_name.clone()))?
                };
                *fn_ptr = *ptr as usize as u64;
            }
        }
        Ok(())
    }
}
