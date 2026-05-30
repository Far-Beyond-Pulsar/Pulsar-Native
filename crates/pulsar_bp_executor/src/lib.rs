/// Blueprint executor backed by the native `pulsar_std` cdylib.
///
/// `BpExecutor::prepare` resolves every `__bp_dispatch_<name>` symbol from the
/// loaded library and patches the address directly into `Instruction::Call::fn_ptr`
/// inside the program. After that, `pbgc::vm::run(&program)` executes with zero
/// table lookups — each Call is one `transmute` + one direct function call.
pub use libloading;
use sha2::{Digest, Sha256};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ExecutorError {
    Dylib(libloading::Error),
    MissingSymbol(String),

    /// The library file could not be read for hash verification.
    Io(std::io::Error),

    /// The library's SHA-256 digest did not match the expected hash.
    /// This indicates the file was tampered with between extraction and load
    /// (TOCTOU attack) or the file is not the expected trusted library.
    HashMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::Dylib(e) => write!(f, "dylib error: {}", e),
            ExecutorError::MissingSymbol(s) => write!(f, "missing symbol: {}", s),
            ExecutorError::Io(e) => write!(f, "I/O error during hash verification: {}", e),
            ExecutorError::HashMismatch { expected, actual } => {
                let exp_hex = expected.iter().map(|b| format!("{:02x}", b)).collect::<String>();
                let act_hex = actual.iter().map(|b| format!("{:02x}", b)).collect::<String>();
                write!(
                    f,
                    "SHA-256 mismatch: expected {exp_hex}, got {act_hex}. \
                     The library may have been tampered with.",
                )
            }
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
    /// When `expected_hash` is `Some`, the file's SHA-256 digest is verified
    /// *before* the library is loaded (mitigating TOCTOU races between write
    /// and load).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use pulsar_bp_executor::BpExecutor;
    /// use pulsar_std_bundle::{extract_to_tempfile, expected_sha256};
    ///
    /// let tmp = extract_to_tempfile().unwrap();
    /// let executor = BpExecutor::load(&tmp.path, Some(expected_sha256())).unwrap();
    /// ```
    pub fn load(
        path: &std::path::Path,
        expected_hash: Option<&[u8; 32]>,
    ) -> Result<Self, ExecutorError> {
        // Verify file integrity before passing to unsafe Library::new.
        // This prevents TOCTOU attacks where a temp file is replaced between
        // write and load.
        if let Some(expected) = expected_hash {
            let bytes =
                std::fs::read(path).map_err(ExecutorError::Io)?;
            let actual: [u8; 32] = Sha256::digest(&bytes).into();
            if &actual != expected {
                return Err(ExecutorError::HashMismatch {
                    expected: *expected,
                    actual,
                });
            }
        }

        let lib = unsafe { libloading::Library::new(path)? };
        Ok(Self { _lib: lib })
    }

    /// Whitelist of allowed dispatch node type prefixes.
    /// Only symbols matching one of these prefixes will be resolved.
    /// This prevents arbitrary code execution by limiting which functions
    /// the blueprint VM can call.
    const ALLOWED_DISPATCH_PREFIXES: &'static [&'static str] = &[
        "__bp_dispatch_std_",
        "__bp_dispatch_core_",
        "__bp_dispatch_math_",
        "__bp_dispatch_string_",
        "__bp_dispatch_array_",
        "__bp_dispatch_flow_",
        "__bp_dispatch_debug_",
        "__bp_dispatch_file_",
        "__bp_dispatch_time_",
        "__bp_dispatch_random_",
        "__bp_dispatch_input_",
        "__bp_dispatch_render_",
        "__bp_dispatch_physics_",
        "__bp_dispatch_scene_",
        "__bp_dispatch_engine_",
        "__bp_dispatch_game_",
        "__bp_dispatch_crypto_",
        "__bp_dispatch_ui_",
        "__bp_dispatch_agent_",
        "__bp_dispatch_network_",
        "__bp_dispatch_audio_",
        "__bp_dispatch_animation_",
        "__bp_dispatch_transform_",
        "__bp_dispatch_collision_",
        "__bp_dispatch_events_",
        "__bp_dispatch_blueprint_",
    ];

    /// Check if a dispatch symbol name is on the allowed whitelist.
    fn is_allowed_dispatch(symbol_name: &str) -> bool {
        Self::ALLOWED_DISPATCH_PREFIXES
            .iter()
            .any(|prefix| symbol_name.starts_with(prefix))
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

                // Whitelist check: only allow known dispatch prefixes.
                if !Self::is_allowed_dispatch(&display_name) {
                    return Err(ExecutorError::MissingSymbol(format!(
                        "Dispatch '{}' is not on the allowed whitelist. \
                         Only whitelisted blueprint node types can be executed.",
                        display_name
                    )));
                }

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
