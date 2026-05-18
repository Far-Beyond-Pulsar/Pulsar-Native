/// The pre-compiled `pulsar_std` WASM module, embedded at build time.
///
/// This byte slice is the `wasm32-unknown-unknown` cdylib output of `pulsar_std`
/// compiled with `--features wasm --no-default-features`. It exports every
/// WASM-ABI-compatible blueprint node function as a `#[no_mangle] extern "C"` symbol.
///
/// The engine loads this module into its WASM runtime (wasmtime, wasmer, etc.) at
/// startup and wires it to the `NodeDispatch` implementation used by `BytecodeVm`.
///
/// # Usage
///
/// ```rust,no_run
/// use pulsar_wasm_bundle::PULSAR_STD_WASM;
///
/// // Hand the bytes to your WASM runtime of choice:
/// // let engine = wasmtime::Engine::default();
/// // let module = wasmtime::Module::new(&engine, PULSAR_STD_WASM).unwrap();
/// println!("WASM module: {} bytes", PULSAR_STD_WASM.len());
/// ```
pub const PULSAR_STD_WASM: &[u8] = include_bytes!(env!("PULSAR_STD_WASM_PATH"));
