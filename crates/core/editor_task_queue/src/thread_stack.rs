//! The stack of another thread of this process, for the UI watcher.
//!
//! When the UI thread stops for good (a deadlock, a wait that never ends) the
//! task queue's own samples only say which task was running. The stack says
//! what it is waiting on (a lock, a channel, a GPU call) and in which
//! function. Windows only (the editor's target); elsewhere [`capture`]
//! returns `None`.
//!
//! The stack is walked with `dbghelp` from a suspended thread. That is not
//! safe in general (the thread might hold the allocator or the loader lock),
//! so the watcher only asks for the stack of a thread that has made no
//! progress for seconds.

/// The id of the calling thread, to hand to [`capture`] later.
#[cfg(windows)]
pub fn current_thread_id() -> u32 {
    // SAFETY: no arguments, no preconditions.
    unsafe { ffi::GetCurrentThreadId() }
}

/// The id of the calling thread, to hand to [`capture`] later.
#[cfg(not(windows))]
pub fn current_thread_id() -> u32 {
    0
}

/// The frames of thread `thread_id`, innermost first, one line each
/// (`symbol+0xoffset`, with `file:line` when the symbols have it).
#[cfg(windows)]
pub fn capture(thread_id: u32) -> Option<Vec<String>> {
    use ffi::*;
    use std::sync::Once;
    const MAX_FRAMES: usize = 48;
    static INIT: Once = Once::new();
    // SAFETY: plain Win32 calls with buffers sized as documented; the thread
    // is resumed on every path after it was suspended.
    unsafe {
        let process = GetCurrentProcess();
        INIT.call_once(|| {
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
            SymInitialize(process, std::ptr::null(), 1);
        });
        let thread = OpenThread(THREAD_ALL_ACCESS, 0, thread_id);
        if thread.is_null() {
            return None;
        }
        if SuspendThread(thread) == u32::MAX {
            CloseHandle(thread);
            return None;
        }
        let mut context = Context([0; 1232]);
        context.set_u32(0x30, CONTEXT_CONTROL_INTEGER);
        let mut pcs: Vec<u64> = Vec::with_capacity(MAX_FRAMES);
        if GetThreadContext(thread, &mut context) != 0 {
            let mut frame = [0u8; 512];
            frame_put(&mut frame, 0, context.u64(0xF8)); // AddrPC
            frame_put(&mut frame, 32, context.u64(0xA0)); // AddrFrame (rbp)
            frame_put(&mut frame, 48, context.u64(0x98)); // AddrStack (rsp)
            for offset in [8, 40, 56] {
                frame[offset + 4] = 3; // AddrModeFlat
            }
            while pcs.len() < MAX_FRAMES
                && StackWalk64(
                    IMAGE_FILE_MACHINE_AMD64,
                    process,
                    thread,
                    frame.as_mut_ptr(),
                    &mut context,
                    std::ptr::null(),
                    SymFunctionTableAccess64,
                    SymGetModuleBase64,
                    std::ptr::null(),
                ) != 0
            {
                let pc = u64::from_le_bytes(frame[0..8].try_into().expect("8 bytes"));
                if pc == 0 {
                    break;
                }
                pcs.push(pc);
            }
        }
        ResumeThread(thread);
        CloseHandle(thread);
        if pcs.is_empty() {
            return None;
        }
        Some(
            pcs.iter()
                .enumerate()
                .map(|(i, pc)| describe(process, *pc, i == 0))
                .collect(),
        )
    }
}

/// The frames of thread `thread_id` (not available off Windows).
#[cfg(not(windows))]
pub fn capture(_thread_id: u32) -> Option<Vec<String>> {
    None
}

#[cfg(windows)]
fn frame_put(frame: &mut [u8; 512], at: usize, value: u64) {
    frame[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// `symbol+0xoffset @ file:line` for a return address (or the innermost pc).
///
/// # Safety
/// `process` is the current process and dbghelp is initialised for it.
#[cfg(windows)]
unsafe fn describe(process: ffi::Handle, pc: u64, innermost: bool) -> String {
    use ffi::*;
    let mut symbol = [0u8; 88 + 1024];
    symbol[0..4].copy_from_slice(&88u32.to_le_bytes()); // SizeOfStruct
    symbol[80..84].copy_from_slice(&1000u32.to_le_bytes()); // MaxNameLen
    let mut displacement = 0u64;
    let mut text = if SymFromAddr(process, pc, &mut displacement, symbol.as_mut_ptr()) != 0 {
        let name = &symbol[84..];
        let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        format!("{}+{displacement:#x}", String::from_utf8_lossy(&name[..end]))
    } else {
        format!("{pc:#x}")
    };
    let mut line = [0u8; 64];
    line[0..4].copy_from_slice(&40u32.to_le_bytes()); // SizeOfStruct
    let mut line_displacement = 0u32;
    // A return address points after the call; look up the call itself.
    let at = if innermost { pc } else { pc.saturating_sub(1) };
    if SymGetLineFromAddr64(process, at, &mut line_displacement, line.as_mut_ptr()) != 0 {
        let number = u32::from_le_bytes(line[16..20].try_into().expect("4 bytes"));
        let file = usize::from_le_bytes(line[24..32].try_into().expect("8 bytes")) as *const std::ffi::c_char;
        if !file.is_null() {
            let file = std::ffi::CStr::from_ptr(file).to_string_lossy();
            text.push_str(&format!(" @ {file}:{number}"));
        }
    }
    text
}

#[cfg(windows)]
#[allow(non_snake_case, clippy::too_many_arguments)]
mod ffi {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const THREAD_ALL_ACCESS: u32 = 0x001F_FFFF;
    pub const CONTEXT_CONTROL_INTEGER: u32 = 0x0010_0003;
    pub const IMAGE_FILE_MACHINE_AMD64: u32 = 0x8664;
    pub const SYMOPT_UNDNAME: u32 = 0x2;
    pub const SYMOPT_DEFERRED_LOADS: u32 = 0x4;
    pub const SYMOPT_LOAD_LINES: u32 = 0x10;

    /// An x64 `CONTEXT` (1232 bytes, 16-aligned), read by offset.
    #[repr(C, align(16))]
    pub struct Context(pub [u8; 1232]);

    impl Context {
        pub fn set_u32(&mut self, at: usize, value: u32) {
            self.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        pub fn u64(&self, at: usize) -> u64 {
            u64::from_le_bytes(self.0[at..at + 8].try_into().expect("8 bytes"))
        }
    }

    type FunctionTableAccess = unsafe extern "system" fn(Handle, u64) -> *mut c_void;
    type ModuleBase = unsafe extern "system" fn(Handle, u64) -> u64;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetCurrentProcess() -> Handle;
        pub fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> Handle;
        pub fn SuspendThread(thread: Handle) -> u32;
        pub fn ResumeThread(thread: Handle) -> u32;
        pub fn GetThreadContext(thread: Handle, context: *mut Context) -> i32;
        pub fn CloseHandle(handle: Handle) -> i32;
    }

    #[link(name = "dbghelp")]
    extern "system" {
        pub fn SymSetOptions(options: u32) -> u32;
        pub fn SymInitialize(process: Handle, search_path: *const u8, invade: i32) -> i32;
        pub fn SymFromAddr(
            process: Handle,
            address: u64,
            displacement: *mut u64,
            symbol: *mut u8,
        ) -> i32;
        pub fn SymGetLineFromAddr64(
            process: Handle,
            address: u64,
            displacement: *mut u32,
            line: *mut u8,
        ) -> i32;
        pub fn SymFunctionTableAccess64(process: Handle, address: u64) -> *mut c_void;
        pub fn SymGetModuleBase64(process: Handle, address: u64) -> u64;
        pub fn StackWalk64(
            machine: u32,
            process: Handle,
            thread: Handle,
            frame: *mut u8,
            context: *mut Context,
            read_memory: *const c_void,
            function_table_access: FunctionTableAccess,
            module_base: ModuleBase,
            translate: *const c_void,
        ) -> i32;
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A thread blocked in a wait shows its wait in the captured stack.
    #[test]
    fn captures_the_stack_of_a_blocked_thread() {
        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let blocked = std::thread::spawn(move || {
            id_tx.send(current_thread_id()).unwrap();
            let _ = stop_rx.recv();
        });
        let id = id_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let frames = capture(id).expect("a stack");
        assert!(frames.len() >= 3, "frames: {frames:#?}");
        assert!(
            frames
                .iter()
                .any(|f| f.contains("Wait") || f.contains("park") || f.contains("recv")),
            "frames: {frames:#?}"
        );
        drop(stop_tx);
        blocked.join().unwrap();
    }
}
