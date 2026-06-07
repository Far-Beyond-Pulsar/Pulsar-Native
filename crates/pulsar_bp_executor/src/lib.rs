/// Blueprint executor backed by the native `pulsar_std` cdylib.
///
/// `BpExecutor::prepare` resolves every `__bp_dispatch_<name>` symbol from the
/// loaded library and patches the address directly into `Instruction::Call::fn_ptr`
/// inside the program. After that, `pbgc::vm::run(&program)` executes with zero
/// table lookups — each Call is one `transmute` + one direct function call.
pub use libloading;
use sha2::{Digest, Sha256};

// ── Safe DLL search path (Windows) ─────────────────────────────────────────────
//
// On Windows, LoadLibraryW searches CWD and PATH for dependencies before safe
// system directories.  An attacker who can write to CWD or any PATH entry can
// plant a malicious DLL.
//
// SetDefaultDllDirectories restricts the search to:
//   LOAD_LIBRARY_SEARCH_APPLICATION_DIR  — the .exe directory
//   LOAD_LIBRARY_SEARCH_SYSTEM32         — C:\Windows\System32
//
// This prevents DLL hijacking via CWD or PATH.
#[cfg(target_os = "windows")]
fn set_safe_dll_search_path() {
    const LOAD_LIBRARY_SEARCH_APPLICATION_DIR: u32 = 0x00000200;
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x00000800;
    extern "system" {
        fn SetDefaultDllDirectories(directory_flags: u32) -> i32;
    }
    unsafe {
        SetDefaultDllDirectories(
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
    }
}

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
                let exp_hex = expected
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
                let act_hex = actual
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
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
            let bytes = std::fs::read(path).map_err(ExecutorError::Io)?;
            let actual: [u8; 32] = Sha256::digest(&bytes).into();
            if &actual != expected {
                return Err(ExecutorError::HashMismatch {
                    expected: *expected,
                    actual,
                });
            }
        }

        #[cfg(target_os = "windows")]
        set_safe_dll_search_path();

        let lib = unsafe { libloading::Library::new(path)? };
        Ok(Self { _lib: lib })
    }

    /// Whitelist of allowed blueprint node names, matched against the bare
    /// `node_type` (i.e. without the `__bp_dispatch_` prefix). Entries may
    /// use `*` as a wildcard — at the start, end, or both — e.g. `"array_*"`
    /// matches `array_push`/`array_get`/…, `"*_port"` matches `http_port`/…
    /// Entries without `*` require an exact match.
    ///
    /// This only allows resolving symbols that correspond to real
    /// `#[blueprint]`-annotated nodes in `pulsar_std`, rather than trusting
    /// every `__bp_dispatch_*` symbol the embedded cdylib happens to export.
    /// Regenerate this list (see `crates/pulsar_std/src/**/*.rs` for
    /// `#[blueprint] pub fn <name>`) whenever new node categories are added.
    const ALLOWED_NODE_NAMES: &'static [&'static str] = &[
        // Category-prefix groups (covers most array/file/string/... nodes)
        "array_*", "assert_*", "atomic_*", "bit_*", "bitwise_*", "break_*",
        "channel_*", "color_*", "count_*", "create_*", "current_*", "debug_*",
        "dir_*", "do_*", "ease_*", "file_*", "format_*", "generate_*",
        "get_*", "greater_*", "hash_*", "hashmap_*", "hashset_*", "hex_*",
        "http_*", "is_*", "join_*", "json_*", "less_*", "log_*",
        "make_*", "mixed_*", "multi_*", "on_*", "parse_*", "path_*",
        "print_*", "process_*", "random_*", "rect_*", "remove_*", "select_*",
        "set_*", "shell_*", "string_*", "switch_*", "system_*", "unix_*",
        "url_*", "validate_*", "vector2_*", "vector3_*",

        // Individually-named nodes that don't share a common group prefix
        "Var1", "Var2", "abs", "add", "add_seconds", "and",
        "angle_difference", "base64_encode", "begin_play", "benchmark_function", "bool_to_string", "bounce_value",
        "branch", "breakpoint", "build_url", "bytes_to_string", "caesar_cipher", "ceil",
        "center_justify", "checksum", "cidr_to_mask", "clamp", "clamp_to_range", "clear_bit",
        "coin_flip", "command_success", "compare_hashes", "conditional_print", "cos", "crc_checksum",
        "crypto_url_encode", "days_to_seconds", "degrees_to_radians", "delay", "denormalize", "distance2d",
        "distance3d", "divide", "divide_add", "dns_port", "emit_event", "equals",
        "example", "first_char", "flip_flop", "floor", "for_loop", "from_percentage",
        "ftp_port", "gate", "hours_to_seconds", "https_port", "in_range", "insert_at",
        "last_char", "left_justify", "lerp", "list_env", "lock_mutex", "main",
        "map_range", "max", "mean", "median", "min", "minutes_to_seconds",
        "modulo", "mongodb_port", "ms_to_seconds", "multiply", "mysql_port", "nearly_equal",
        "normalize", "normalize_path", "not", "not_equals", "now", "number_to_string",
        "or", "percentage", "ping_pong", "postgresql_port", "power", "println",
        "proportion", "radians_to_degrees", "randexec", "range", "range_switch", "ratio",
        "redis_port", "repeat", "retriggerable_delay", "reverse_string", "right_justify", "rot13",
        "round", "run_command", "runlua", "seconds_to_ms", "sequence", "shuffle_seed",
        "sign", "sin", "sleep_ms", "smoothstep", "smtp_port", "spawn_thread",
        "split_path", "sqrt", "ssh_port", "std_dev", "subtract", "subtract_seconds",
        "tan", "templateLua", "timestamp_difference", "toggle_bit", "transform_new", "unlock_mutex",
        "variance", "verify_checksum", "while_loop", "xor", "xor_cipher",
    ];

    /// Match `name` against a whitelist `pattern` that may use `*` as a
    /// leading and/or trailing wildcard (e.g. `"array_*"`, `"*_port"`,
    /// `"*lua*"`, or bare `"*"` to match anything). A pattern with no `*`
    /// requires an exact match.
    fn glob_match(pattern: &str, name: &str) -> bool {
        match (pattern.starts_with('*'), pattern.ends_with('*')) {
            (false, false) => pattern == name,
            (false, true) => name.starts_with(&pattern[..pattern.len() - 1]),
            (true, false) => name.ends_with(&pattern[1..]),
            (true, true) => {
                if pattern.len() == 1 {
                    true
                } else {
                    name.contains(&pattern[1..pattern.len() - 1])
                }
            }
        }
    }

    /// Check whether `node_type` names a whitelisted blueprint node.
    fn is_allowed_node(node_type: &str) -> bool {
        Self::ALLOWED_NODE_NAMES
            .iter()
            .any(|pattern| Self::glob_match(pattern, node_type))
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
                // Whitelist check: only allow known blueprint node names.
                // (Matched against the bare `node_type` — dispatch symbols
                // are named `__bp_dispatch_<node_type>` with no category
                // infix, so the whitelist patterns mirror that convention.)
                if !Self::is_allowed_node(node_type) {
                    return Err(ExecutorError::MissingSymbol(format!(
                        "Dispatch '__bp_dispatch_{}' is not on the allowed whitelist. \
                         Only whitelisted blueprint node types can be executed.",
                        node_type
                    )));
                }

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
