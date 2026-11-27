//! The steps of [`PieHost::load`](super::PieHost::load): the library copy, the
//! ABI gate, symbol resolution, the init context and the game's event bus.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use pulsar_events::gamma::ffi::{ForeignBus, RawBus};
use pulsar_pie_abi::{
    EngineContext as PieContext, FnAbiVersion, FnAssetUpdated, FnControl, FnEventBus,
    FnEventsSnapshot, FnInit, FnInput, FnResize, FnShutdown, FnTakeProblems, FnTick,
    PIE_ABI_VERSION, SYM_ABI_VERSION, SYM_ASSET_UPDATED, SYM_CONTROL, SYM_EVENTS_SNAPSHOT,
    SYM_EVENT_BUS, SYM_INIT, SYM_INPUT, SYM_RESIZE, SYM_SHUTDOWN, SYM_TAKE_PROBLEMS, SYM_TICK,
};

use super::{log_cb, pie_lock_world, pie_unlock_world, sym_name, TempCopy};

/// The game's entry points; the optional ones are missing from games built
/// before they existed.
pub(super) struct EntryPoints {
    pub(super) init: FnInit,
    pub(super) tick: FnTick,
    pub(super) resize: FnResize,
    pub(super) input: FnInput,
    pub(super) shutdown: FnShutdown,
    pub(super) asset_updated: Option<FnAssetUpdated>,
    pub(super) events_snapshot: Option<FnEventsSnapshot>,
    pub(super) control: Option<FnControl>,
    pub(super) take_problems: Option<FnTakeProblems>,
    pub(super) event_bus: Option<FnEventBus>,
}

/// What the init context points at or carries.
pub(super) struct ContextParams<'a> {
    pub(super) device: &'a Arc<wgpu::Device>,
    pub(super) queue: &'a Arc<wgpu::Queue>,
    pub(super) color_format: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) project_root: &'a str,
    pub(super) scene_path: Option<&'a str>,
    pub(super) userdata: *mut c_void,
    pub(super) shared_world: *const c_void,
    pub(super) reload: bool,
    pub(super) view: Option<crate::subsystems::render::EditorCameraState>,
    pub(super) level_name: Option<&'a str>,
}

/// The path to load the library from, and the temp copy that owns it when
/// there is one. On Windows the original file stays locked while loaded,
/// which blocks the next `cargo build --lib` (hot-reload), so a temp copy is
/// loaded instead.
// std::fs allowed: a temp copy of a build artifact (libloading needs a real path), not a project file
#[allow(clippy::disallowed_methods)]
pub(super) fn load_path_for(dylib_path: &Path) -> Result<(PathBuf, Option<TempCopy>), String> {
    if cfg!(target_os = "windows") {
        let mut tmp = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = dylib_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pie_game");
        tmp.push(format!("{name}_{stamp}.dll"));
        std::fs::copy(dylib_path, &tmp)
            .map_err(|e| format!("Failed to copy game dll to temp: {e}"))?;
        Ok((tmp.clone(), Some(TempCopy(tmp))))
    } else {
        Ok((dylib_path.to_path_buf(), None))
    }
}

/// Connects the game library to this process, before any of its code runs.
///
/// The library is a second copy of the engine's crates working on the
/// editor's GPU device and world. It shares this process's parking lot, so a
/// thread parked in one copy on a lock the two share (the device's, say) is
/// woken by the other's unlock, and Helio's runtime, so its renderer's
/// threads and pipeline builds are the editor's tasks, as a plugin
/// library's are (`plugin_manager::attach`).
pub(super) unsafe fn attach_host_runtimes(lib: &Library) -> Result<(), String> {
    const REBUILD: &str = "Rebuild the game library (its src/lib.rs must call \
                           pulsar_game::export_pie_runtime_attach!()).";
    let parking: parking_lot_core::AttachFn =
        required(lib, parking_lot_core::ATTACH_SYMBOL.as_bytes())
            .map_err(|error| format!("{error}. {REBUILD}"))?;
    // SAFETY: the symbol is the library's parking lot entry point, and the
    // host keeps the library loaded while its code can run.
    if !unsafe { parking_lot_core::share_with(parking) } {
        return Err(format!("The game library refused the editor's parking lot. {REBUILD}"));
    }
    let helio: helio_core::runtime::AttachFn =
        required(lib, helio_core::runtime::ATTACH_SYMBOL.as_bytes())
            .map_err(|error| format!("{error}. {REBUILD}"))?;
    // SAFETY: as above, the library's Helio entry point.
    if !unsafe { helio_core::runtime::share_with(helio) } {
        return Err(format!("The game library refused the editor's renderer runtime. {REBUILD}"));
    }
    Ok(())
}

/// Fails when the library was built against another ABI version.
pub(super) unsafe fn check_abi_version(lib: &Library) -> Result<(), String> {
    let abi_version: FnAbiVersion = *lib
        .get(SYM_ABI_VERSION)
        .map_err(|e| format!("Missing symbol {}: {e}", sym_name(SYM_ABI_VERSION)))?;
    let lib_abi = abi_version();
    if lib_abi != PIE_ABI_VERSION {
        return Err(if lib_abi < PIE_ABI_VERSION {
            format!(
                "Game was built against PiE ABI v{lib_abi}, editor expects v{PIE_ABI_VERSION}. \
                 Rebuild the project (Build Core)."
            )
        } else {
            format!(
                "Game was built against PiE ABI v{lib_abi}, editor expects v{PIE_ABI_VERSION}. \
                 Update Pulsar (the game library is newer than this editor)."
            )
        });
    }
    Ok(())
}

/// A required symbol of the library, copied out as a plain `fn` pointer.
unsafe fn required<T: Copy>(lib: &Library, sym: &[u8]) -> Result<T, String> {
    Ok(*lib
        .get::<T>(sym)
        .map_err(|e| format!("Missing symbol {}: {e}", sym_name(sym)))?)
}

/// An optional symbol of the library, copied out as a plain `fn` pointer.
unsafe fn optional<T: Copy>(lib: &Library, sym: &[u8]) -> Option<T> {
    lib.get::<T>(sym).ok().map(|symbol| *symbol)
}

/// Resolves every entry point after the ABI gate.
pub(super) unsafe fn resolve_entry_points(lib: &Library) -> Result<EntryPoints, String> {
    Ok(EntryPoints {
        init: required(lib, SYM_INIT)?,
        tick: required(lib, SYM_TICK)?,
        resize: required(lib, SYM_RESIZE)?,
        input: required(lib, SYM_INPUT)?,
        shutdown: required(lib, SYM_SHUTDOWN)?,
        asset_updated: optional(lib, SYM_ASSET_UPDATED),
        events_snapshot: optional(lib, SYM_EVENTS_SNAPSHOT),
        control: optional(lib, SYM_CONTROL),
        take_problems: optional(lib, SYM_TAKE_PROBLEMS),
        event_bus: optional(lib, SYM_EVENT_BUS),
    })
}

/// The context handed to the game's `init`. Boxed so its address stays fixed
/// while the game holds `&mut *ctx`.
pub(super) fn build_context(params: &ContextParams<'_>) -> Box<PieContext> {
    Box::new(PieContext {
        abi_version: PIE_ABI_VERSION,
        device: Arc::as_ptr(params.device) as *const c_void,
        queue: Arc::as_ptr(params.queue) as *const c_void,
        color_format: params.color_format,
        width: params.width.max(1),
        height: params.height.max(1),
        project_root_ptr: params.project_root.as_ptr(),
        project_root_len: params.project_root.len(),
        scene_path_ptr: params
            .scene_path
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null()),
        scene_path_len: params.scene_path.map(|s| s.len()).unwrap_or(0),
        userdata: params.userdata,
        log: log_cb,
        out_texture: std::ptr::null(),
        shared_world: params.shared_world,
        lock_shared_world: pie_lock_world,
        unlock_shared_world: pie_unlock_world,
        session_flags: if params.reload {
            pulsar_pie_abi::session_flags::RELOAD
        } else {
            0
        },
        view: params
            .view
            .map_or_else(Default::default, |view| pulsar_pie_abi::PieView {
                position: view.position,
                yaw: view.yaw,
                pitch: view.pitch,
                present: 1,
                _reserved: 0,
            }),
        // The game is a session of its own on the editor's world: the
        // host says so (ABI v5), the guest never guesses.
        session_kind: pulsar_pie_abi::session_kind::PLAY_IN_EDITOR,
        _reserved: 0,
        level_name_ptr: params.level_name.map_or(std::ptr::null(), str::as_ptr),
        level_name_len: params.level_name.map_or(0, str::len),
    })
}

/// The game's hub for editor plugins, for the session.
pub(super) unsafe fn export_session_bus(event_bus: Option<FnEventBus>) -> Option<ForeignBus> {
    event_bus.and_then(|export| {
        let mut raw = std::mem::MaybeUninit::<RawBus>::uninit();
        let ok = export(
            raw.as_mut_ptr() as *mut c_void,
            std::mem::size_of::<RawBus>(),
        );
        if ok != 1 {
            return None;
        }
        // SAFETY: the game wrote a fresh export whose reference we own.
        match ForeignBus::from_raw(raw.assume_init()) {
            Ok(bus) => Some(bus),
            Err(error) => {
                tracing::warn!(
                    "PiE: the game's event bus speaks another Gamma FFI version: {error:?}"
                );
                None
            }
        }
    })
}
