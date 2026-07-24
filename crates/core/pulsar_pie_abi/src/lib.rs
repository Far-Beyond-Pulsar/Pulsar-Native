//! Shared C-ABI contract for **Play In Editor** (PiE, issue #243).
//!
//! The editor (`engine_backend`, the *host*) compiles a user's game project as a
//! platform dynamic library (`cdylib`) and drives it from its own render loop —
//! no separate window, no separate GPU device. This crate defines the exact
//! `#[repr(C)]` structures and function-pointer types that cross the dylib
//! boundary so both sides agree on layout byte-for-byte.
//!
//! ## Safety model
//!
//! * The crate is `#![no_std]` and depends only on [`core::ffi`]. It pulls in no
//!   `wgpu` (or any other) types, so its layout can never drift because a
//!   transitive dependency resolved to a different version on one side.
//! * GPU handles (`wgpu::Device`, `wgpu::Queue`, `wgpu::Texture`) are passed as
//!   opaque `*const c_void`. They are only sound to dereference when **both**
//!   sides linked the *same* `wgpu` version with the *same* toolchain. That is
//!   guaranteed at runtime by the [`PIE_ABI_VERSION`] gate below plus the
//!   workspace pinning both sides to one `wgpu` major and one Helio git rev.
//! * Every `extern "C"` entry point on the game side must wrap its body in
//!   [`core::panic`]/`catch_unwind` (in the game's `std` context) — unwinding
//!   across the FFI boundary is undefined behaviour.
//!
//! ## Handshake
//!
//! 1. Host loads the dylib and calls [`SYM_ABI_VERSION`]; if it does not equal
//!    [`PIE_ABI_VERSION`] the host refuses to load and asks the user to rebuild.
//! 2. Host fills an [`EngineContext`] (project root + a `.level` path written
//!    from the editor's current `SceneDb`) and calls [`SYM_INIT`]. The game
//!    builds its world + offscreen renderer and writes
//!    [`EngineContext::out_texture`].
//! 3. Each editor frame: host calls [`SYM_TICK`] with the delta time; the game
//!    advances simulation and renders into its offscreen texture. The game runs
//!    independently from here — the editor only displays its frames (Unreal-style
//!    PIE); there is no game→editor scene writeback.
//! 4. Host forwards input via [`SYM_INPUT`] and size changes via [`SYM_RESIZE`].
//! 5. On stop the host calls [`SYM_SHUTDOWN`] and then unloads the library.

#![no_std]

use core::ffi::c_void;

/// ABI revision. **Bump on any change** to the structs or symbol signatures in
/// this crate. The host compares the value it was compiled against with the
/// value [`SYM_ABI_VERSION`] returns from the loaded library and refuses to run
/// on mismatch.
pub const PIE_ABI_VERSION: u64 = 1;

// ── Log levels (match `tracing`) ────────────────────────────────────────────

pub const LOG_ERROR: u32 = 0;
pub const LOG_WARN: u32 = 1;
pub const LOG_INFO: u32 = 2;
pub const LOG_DEBUG: u32 = 3;
pub const LOG_TRACE: u32 = 4;

// ── Callbacks (game → host) ─────────────────────────────────────────────────
//
// PiE follows Unreal's model: the game receives the *initial* scene state and
// then runs independently — the editor only displays its frames. So there is no
// game→editor scene writeback; the only callback is logging.

/// Route a game-side log line into the editor's tracing/log viewer.
///
/// `level` is one of the `LOG_*` constants. The message is UTF-8; the pointer is
/// only valid for the duration of the call.
pub type LogFn =
    extern "C" fn(userdata: *mut c_void, level: u32, msg_ptr: *const u8, msg_len: usize);

// ── EngineContext (host → game) ─────────────────────────────────────────────

/// Everything the host hands the embedded game at init time, plus the one field
/// (`out_texture`) the game fills in for the host to read afterwards.
///
/// `#[repr(C)]`: field order and layout are part of the ABI — only append new
/// fields at the end and bump [`PIE_ABI_VERSION`].
#[repr(C)]
pub struct EngineContext {
    /// Must equal [`PIE_ABI_VERSION`]; lets the game double-check the struct it
    /// was handed matches what it was compiled against.
    pub abi_version: u64,

    /// `*const wgpu::Device` — the editor's (GPUI's) device. Borrowed; the game
    /// must not drop it. Valid until [`SYM_SHUTDOWN`] returns.
    pub device: *const c_void,
    /// `*const wgpu::Queue` for the same device. Borrowed.
    pub queue: *const c_void,
    /// `wgpu::TextureFormat` reinterpreted as `u32` — the color format the host
    /// viewport expects the game's `out_texture` to use.
    pub color_format: u32,
    /// Initial render target size in physical pixels.
    pub width: u32,
    pub height: u32,

    /// UTF-8 path to the game project root (the directory containing its
    /// `Cargo.toml` and `.pulsar/` settings tree). Valid only for the duration
    /// of the [`SYM_INIT`] call; the game copies what it needs.
    pub project_root_ptr: *const u8,
    pub project_root_len: usize,

    /// UTF-8 path to the `.level` file the game should load. The editor writes
    /// its *current* `SceneDb` to a temp `.level` before Play so unsaved edits
    /// are reflected. Valid only for the duration of the [`SYM_INIT`] call.
    pub scene_path_ptr: *const u8,
    pub scene_path_len: usize,

    /// Opaque host handle echoed back into the log callback.
    pub userdata: *mut c_void,
    /// Log callback (game → editor).
    pub log: LogFn,

    /// Filled by the game during [`SYM_INIT`]: `*const wgpu::Texture` for the
    /// offscreen render target the game draws into each tick. The host samples
    /// this into its viewport. Null until init succeeds. Because both sides share
    /// the same `wgpu::Device`, no cross-device import is needed.
    pub out_texture: *const c_void,
}

// ── Input (host → game) ─────────────────────────────────────────────────────

/// Discriminant for [`InputEvent::kind`].
pub mod input_kind {
    pub const MOUSE_MOVE: u32 = 0;
    pub const MOUSE_BUTTON: u32 = 1;
    pub const MOUSE_WHEEL: u32 = 2;
    pub const KEY: u32 = 3;
}

/// A single input event forwarded from the editor's input abstraction. Kept flat
/// and `#[repr(C)]` so no platform-specific handling leaks into the game lib.
#[repr(C)]
pub struct InputEvent {
    /// One of the [`input_kind`] constants.
    pub kind: u32,
    /// Cursor position in normalized viewport coordinates (0..1), for
    /// `MOUSE_MOVE` / `MOUSE_BUTTON`.
    pub x: f32,
    pub y: f32,
    /// Mouse button index (`MOUSE_BUTTON`) or virtual key code (`KEY`).
    pub button_or_key: u32,
    /// `1` = pressed/down, `0` = released/up. Unused for move/wheel.
    pub pressed: u32,
    /// Scroll delta for `MOUSE_WHEEL`.
    pub delta: f32,
}

// ── Exported-symbol signatures (used by the host loader) ────────────────────

/// `extern "C" fn() -> u64` — returns [`PIE_ABI_VERSION`] the lib was built with.
pub type FnAbiVersion = unsafe extern "C" fn() -> u64;
/// `extern "C" fn(*mut EngineContext) -> u32` — `1` on success, `0` on failure.
pub type FnInit = unsafe extern "C" fn(*mut EngineContext) -> u32;
/// `extern "C" fn(delta_seconds: f32)` — advance + render one frame.
pub type FnTick = unsafe extern "C" fn(f32);
/// `extern "C" fn(width: u32, height: u32)` — resize the offscreen target.
pub type FnResize = unsafe extern "C" fn(u32, u32);
/// `extern "C" fn(*const InputEvent)` — forward one input event.
pub type FnInput = unsafe extern "C" fn(*const InputEvent);
/// `extern "C" fn()` — tear down world + renderer before the lib is unloaded.
pub type FnShutdown = unsafe extern "C" fn();

/// Success/failure sentinel for [`FnInit`].
pub const INIT_OK: u32 = 1;
pub const INIT_ERR: u32 = 0;

// Exported symbol names the host resolves. Keep in sync with the `#[no_mangle]`
// functions the generated `lib.rs` defines.
pub const SYM_ABI_VERSION: &[u8] = b"pulsar_pie_abi_version";
pub const SYM_INIT: &[u8] = b"pulsar_pie_init";
pub const SYM_TICK: &[u8] = b"pulsar_pie_tick";
pub const SYM_RESIZE: &[u8] = b"pulsar_pie_resize";
pub const SYM_INPUT: &[u8] = b"pulsar_pie_input";
pub const SYM_SHUTDOWN: &[u8] = b"pulsar_pie_shutdown";
