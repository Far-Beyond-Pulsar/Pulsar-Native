/// Blueprint executor backed by the native `pulsar_std` cdylib.
///
/// `BpExecutor` opens the dynamic library once, then for each `BpProgram`
/// resolves `__bp_dispatch_<node_type>` to a typed function pointer indexed
/// by `node_type_idx`. The VM loop in PBGC then calls those pointers directly:
///
/// ```text
/// Instruction::Call { node_type_idx, inputs, output }
///   → dispatch[node_type_idx](scratch.as_ptr(), &mut result)
///   → shim reads real Rust types from raw u64 bits, calls the actual fn
/// ```
///
/// There is no BpValue, no match arm, no type conversion in the executor —
/// just a pointer dereference and a C ABI call.

pub use libloading;

/// ABI of every `__bp_dispatch_<name>` symbol in the native lib.
pub type DispatchFn = unsafe extern "C" fn(inputs: *const u64, output: *mut u64);

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
    fn from(e: libloading::Error) -> Self { ExecutorError::Dylib(e) }
}

// ── BpExecutor ────────────────────────────────────────────────────────────────

/// Loaded native lib + symbol resolver.
///
/// The `Library` must outlive all `DispatchFn` pointers resolved from it.
/// Keep `BpExecutor` alive for the duration of any program execution.
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

    /// Resolve every `node_type` in `node_types` to its `__bp_dispatch_*` symbol.
    ///
    /// Returns a `Vec<DispatchFn>` indexed by `node_type_idx` — ready to pass
    /// directly to `pbgc::vm::run`.
    ///
    /// Fails with `ExecutorError::MissingSymbol` if a node type has no shim
    /// (e.g. a function with a String parameter that opted out of dispatch).
    pub fn resolve_dispatch(
        &self,
        node_types: &[String],
    ) -> Result<Vec<DispatchFn>, ExecutorError> {
        node_types.iter().map(|name| {
            let symbol_name = format!("__bp_dispatch_{}", name);
            let lookup_symbol = format!("{}\0", symbol_name);
            let func: libloading::Symbol<DispatchFn> = unsafe {
                self._lib.get(lookup_symbol.as_bytes())
                    .map_err(|_| ExecutorError::MissingSymbol(symbol_name.clone()))?
            };
            Ok(*func)
        }).collect()
    }
}
