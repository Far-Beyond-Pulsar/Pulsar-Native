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
//!    from the editor's current scene) and calls [`SYM_INIT`]. Under **ABI v2**
//!    the context also carries the host's authoritative world token (see
//!    [`EngineContext::shared_world`]); the game adopts that world instead of
//!    loading its own copy, which is what makes the two-way bridge work.
//!    The game builds its offscreen renderer and writes
//!    [`EngineContext::out_texture`].
//! 3. Each editor frame: host calls [`SYM_TICK`] with the delta time; the game
//!    advances simulation -- acquiring the shared world through the lock
//!    callbacks for exactly its tick slice -- and renders into its offscreen
//!    texture. Editor edits made mid-session are visible to the next tick;
//!    gameplay mutations are visible to the editor's panels and viewport the
//!    same way (subscriptions fire on the one shared world).
//! 4. Host forwards input via [`SYM_INPUT`] and size changes via [`SYM_RESIZE`].
//! 5. On stop the host calls [`SYM_SHUTDOWN`] and then unloads the library.
//!
//! ## The shared-world contract (ABI v2, issue #635)
//!
//! `EngineContext::shared_world` is a `*const c_void` that both sides
//! reinterpret as `*const RwLock<WorldSceneStore>` (the concrete types live in
//! `engine_backend::scene`, linked identically into host and guest from the
//! same workspace build -- the same single-universe guarantee the raw wgpu
//! handles above rely on). The guest NEVER locks that RwLock directly: all
//! access goes through the phase-boundary lock callbacks, which keeps the
//! LOCKING POLICY owned by the host and lets the host assert the protocol is
//! being followed.
//!
//! Protocol (the full statement of issue #635's locking rules):
//!
//! 1. **Guest tick slice**: during [`SYM_TICK`] the guest calls
//!    `lock_shared_world(userdata)` once, receives an exclusive
//!    `*mut WorldSceneStore` (as `*mut c_void`), runs simulation + render
//!    preparation against it, and calls `unlock_shared_world(userdata)`
//!    exactly once before returning. The pointer is invalid after unlock --
//!    never cached across slices, never freed.
//! 2. **Host discipline**: the host never holds its own lock across a frame
//!    boundary; every host-side touch of the world is a short scope (this is
//!    already true of the post-#634/#637 render sync passes). While the guest
//!    holds its slice the host simply doesn't contend.
//! 3. **Non-reentrancy as witness**: a second `lock_shared_world` call while
//!    one slice is open returns null. This makes the callback pair a dynamic
//!    borrow witness: at most one exclusive slice exists at any time, which
//!    is what makes handing the guest `&mut WorldSceneStore` sound rather
//!    than trusting convention.
//!
//! ### FFI safety story for `&mut WorldSceneStore` (decision)
//!
//! Two designs were weighed (issue #635's last checklist item):
//!
//! * *Command queue* -- the guest submits serialized world commands; the host
//!   applies them between frames. Maximally safe, but every property/method
//!   surface doubles into a message type, and latency lands between
//!   submit and apply (violating same-frame visibility).
//! * *Direct exclusive reference under witness* (**chosen**) -- the lock
//!   callback pair IS the witness: it hands out one provably-unaliased
//!   `&mut WorldSceneStore` per tick slice. Soundness rests on three
//!   invariants, all enforced mechanically: (a) the guest runs its slice on
//!   the single thread that owns the embedded game (`thread_local!` GAME,
//!   unchanged since #243); (b) exclusivity is guaranteed by the host's own
//!   RwLock, which the host itself only touches in short scopes; (c)
//!   non-reentrancy is checked, not assumed (null on violation). A command
//!   queue remains the escape hatch if a future multi-threaded guest ever
//!   breaks invariant (a); the ABI would grow a batched-command variant
//!   alongside this one, not replace it.

#![no_std]

use core::ffi::c_void;

/// ABI revision. **Bump on any change** to the structs or symbol signatures in
/// this crate. The host compares the value it was compiled against with the
/// value [`SYM_ABI_VERSION`] returns from the loaded library and refuses to run
/// on mismatch.
///
/// History: v1 handed the game a rendered-copy contract (guest-owned world,
/// one-way texture out, explicitly no writeback). v2 (#635) adds the
/// shared-world token + lock callbacks below and changes ownership of scene
/// state to the host; the wire structs changed shape, hence the bump.
pub const PIE_ABI_VERSION: u64 = 2;

// ── Log levels (match `tracing`) ────────────────────────────────────────────

pub const LOG_ERROR: u32 = 0;
pub const LOG_WARN: u32 = 1;
pub const LOG_INFO: u32 = 2;
pub const LOG_DEBUG: u32 = 3;
pub const LOG_TRACE: u32 = 4;

// ── Callbacks (game → host) ─────────────────────────────────────────────────
//
// PiE v1 followed Unreal's model: the game received the *initial* scene state
// and then ran independently — the editor only displayed its frames. ABI v2
// (#635) inverts the ownership: the game operates on the EDITOR's world, so
// besides logging it receives the phase-boundary lock callbacks below.

/// Route a game-side log line into the editor's tracing/log viewer.
///
/// `level` is one of the `LOG_*` constants. The message is UTF-8; the pointer is
/// only valid for the duration of the call.
pub type LogFn =
    extern "C" fn(userdata: *mut c_void, level: u32, msg_ptr: *const u8, msg_len: usize);

/// Acquire the host's authoritative world for ONE guest tick slice.
///
/// Returns an exclusive `*mut WorldSceneStore` (as `*mut c_void`) that is valid
/// until the matching [`UnlockWorldFn`] call, or null if a slice is already
/// open (non-reentrancy witness -- see the module doc's locking protocol).
/// The returned pointer must never be cached past unlock, written through as
/// anything but its true type, or freed.
pub type LockWorldFn = unsafe extern "C" fn(userdata: *mut c_void) -> *mut c_void;

/// Close the slice opened by [`LockWorldFn`]. Passing a userdata other than
/// the one handed at init is a contract violation; unlocking an un-opened
/// slice is tolerated (idempotent) so early-error paths can't wedge the host.
pub type UnlockWorldFn = unsafe extern "C" fn(userdata: *mut c_void);

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

    // ── ABI v2 additions (#635): the shared-world token ────────────────────
    /// The host's authoritative world: `*const RwLock<WorldSceneStore>` (the
    /// allocation behind the host's `Arc`, which outlives the whole PIE
    /// session). Under v2 this is non-null and OWNERSHIP OF SCENE STATE stays
    /// with the host -- the guest adopts it (via the documented single-count
    /// `Arc` transfer) instead of loading its own copy, and must not hydrate
    /// a level file into it again. Never dereference directly; go through
    /// [`lock_shared_world`]/[`unlock_shared_world`] per the locking protocol.
    /// Null only in legacy/v1 contexts (rejected by the version gate).
    pub shared_world: *const c_void,
    /// Acquire one exclusive tick slice of [`shared_world`]. See
    /// [`LockWorldFn`] and the module doc's locking protocol.
    pub lock_shared_world: LockWorldFn,
    /// Release the slice. See [`UnlockWorldFn`].
    pub unlock_shared_world: UnlockWorldFn,
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
