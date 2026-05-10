#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use std::{fs, path::PathBuf};

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut core::ffi::c_void;
}

#[cfg(target_os = "macos")]
fn ax_is_process_trusted(_prompt: bool) -> bool {
    use std::process;

    unsafe {
        let pid = process::id() as i32;
        let app_ref = AXUIElementCreateApplication(pid);
        
        if app_ref.is_null() {
            eprintln!("[PERMISSIONS] AXUIElementCreateApplication returned null - trust not granted");
            false
        } else {
            eprintln!("[PERMISSIONS] AXUIElementCreateApplication succeeded - trust confirmed");
            core_foundation_sys::base::CFRelease(app_ref.cast());
            true
        }
    }
}

#[cfg(target_os = "macos")]
fn prompt_state_file() -> Option<PathBuf> {
    let project_dirs = directories::ProjectDirs::from("dev", "Pulsar", "Pulsar")?;
    Some(
        project_dirs
            .data_local_dir()
            .join("permissions")
            .join("accessibility_prompt_last_unix_s"),
    )
}

#[cfg(target_os = "macos")]
fn now_unix_seconds() -> Option<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_secs())
}

#[cfg(target_os = "macos")]
fn prompt_cooldown_active(cooldown_secs: u64) -> bool {
    let Some(path) = prompt_state_file() else {
        return false;
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(last_prompt_s) = raw.trim().parse::<u64>() else {
        return false;
    };
    let Some(now_s) = now_unix_seconds() else {
        return false;
    };

    now_s.saturating_sub(last_prompt_s) < cooldown_secs
}

#[cfg(target_os = "macos")]
fn mark_prompted_now() {
    let Some(path) = prompt_state_file() else {
        return;
    };
    let Some(now_s) = now_unix_seconds() else {
        return;
    };

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, now_s.to_string());
}

/// Prompt for macOS Accessibility permission and block until it is granted.
///
/// This is intentionally called at startup so later relative-mouse APIs used by
/// the viewport do not run before accessibility trust is available.
#[cfg(target_os = "macos")]
pub fn ensure_accessibility_permission_blocking() {
    const PROMPT_COOLDOWN_SECS: u64 = 60 * 60 * 12;

    if ax_is_process_trusted(false) {
        return;
    }

    if let Ok(exe) = std::env::current_exe() {
        eprintln!("[PERMISSIONS] Executable: {}", exe.display());
    }

    // Trigger the system prompt once at startup, but throttle it across launches
    // to avoid repeated system dialogs when TCC state is delayed or identity differs.
    if prompt_cooldown_active(PROMPT_COOLDOWN_SECS) {
        eprintln!(
            "[PERMISSIONS] Prompt throttled (cooldown active). Skipping system prompt this launch."
        );
    } else {
        let _ = ax_is_process_trusted(true);
        mark_prompted_now();
    }

    eprintln!(
        "[PERMISSIONS] Waiting for macOS Accessibility permission so viewport input can start safely..."
    );

    let start = Instant::now();
    let timeout = Duration::from_secs(180);
    let mut attempt: u64 = 0;

    loop {
        attempt += 1;
        let elapsed = start.elapsed();
        let trusted = ax_is_process_trusted(false);
        eprintln!(
            "[PERMISSIONS] Check #{attempt}: trusted={trusted} elapsed={}ms",
            elapsed.as_millis()
        );

        if trusted {
            eprintln!("[PERMISSIONS] Accessibility permission granted");
            break;
        }

        if elapsed >= timeout {
            eprintln!(
                "[PERMISSIONS] Accessibility permission was not detected within 180s; continuing startup and relying on runtime guards"
            );
            break;
        }

        thread::sleep(Duration::from_millis(300));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_accessibility_permission_blocking() {}
