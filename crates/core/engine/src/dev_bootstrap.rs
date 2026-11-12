//! Sidecar mode for rebuilding and relaunching the release engine executable.
//!
//! The sidecar keeps the old binary available while Cargo replaces the release
//! executable. The progress and the failure choice are shown by the
//! `pulsar-dialog` helper (a progress window in the engine's own look), so the
//! sidecar itself has no UI and looks the same on Windows, macOS and Linux.

use pulsar_dialog_protocol::progress::{self, ProgressRequest, ProgressWindow};
use pulsar_dialog_protocol::{client, ButtonKind, DialogButton, DialogRequest, Severity};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SIDECAR_ARG: &str = "--pulsar-bootstrap-sidecar-run";

/// The button ids of the failure choice.
const RELAUNCH: &str = "relaunch";
const QUIT: &str = "quit";

/// Handle the internal sidecar invocation before normal engine startup.
/// Returns `true` when the process should exit instead of starting the UI.
pub fn run_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != SIDECAR_ARG) {
        return false;
    }

    let sidecar = std::env::current_exe().ok();
    run_sidecar(&args, sidecar.as_deref());
    schedule_sidecar_cleanup(sidecar);
    true
}

/// The progress window, if the helper could open one. Without it the rebuild
/// still runs; only the failure choice is asked another way.
struct Reporter {
    window: Option<ProgressWindow>,
}

impl Reporter {
    fn set(&mut self, stage: &str, progress: f32) {
        if let Some(window) = self.window.as_mut() {
            window.update(Some(stage), Some(progress));
        }
    }
}

fn run_sidecar(args: &[OsString], stable_executable: Option<&Path>) {
    use_the_engines_helper(args);
    let request = ProgressRequest::new("Bootstrap Engine", "Rebuilding engine")
        .status("Waiting for the editor to close");
    let mut reporter = Reporter {
        window: progress::start(&request).ok(),
    };
    reporter.set("Waiting for the editor to close", 0.04);

    match build_and_relaunch(args, &mut reporter) {
        Ok(()) => {
            if let Some(mut window) = reporter.window.take() {
                window.set_heading("Engine rebuilt and relaunched");
                window.update(Some("Launching the rebuilt release engine"), Some(1.0));
                window.finish();
            }
        }
        Err(failure) => {
            append_log(&format!("\nBootstrap failed: {failure}\n"));
            recover(reporter.window.take(), stable_executable, args, &failure);
        }
    }
}

/// The sidecar is a copy of the engine in the temp directory; the helper was
/// placed beside the engine it was copied from (`args[4]`). Use that one, so
/// nothing is written next to the temporary copy.
fn use_the_engines_helper(args: &[OsString]) {
    let helper = arg(args, 4)
        .ok()
        .map(PathBuf::from)
        .and_then(|exe| exe.parent().map(|dir| dir.join(client::helper_file_name())));
    if let Some(helper) = helper.filter(|path| path.is_file()) {
        client::set_helper_path(helper);
    }
}

// std::fs allowed: restart sidecar: truncates the bootstrap log in the OS temp dir (process plumbing, no project)
#[allow(clippy::disallowed_methods)]
fn build_and_relaunch(args: &[OsString], reporter: &mut Reporter) -> Result<(), String> {
    let parent_pid = arg(args, 2)?
        .to_string_lossy()
        .parse::<u32>()
        .map_err(|error| format!("Invalid parent process id: {error}"))?;
    let workspace = PathBuf::from(arg(args, 3)?);
    let working_directory = PathBuf::from(arg(args, 5)?);
    let restart_args = args.get(6..).unwrap_or_default().to_vec();
    let target_dir = cargo_target_dir(&workspace);
    let release_executable = release_executable_path(&target_dir);

    wait_for_process_exit(parent_pid)?;
    let log_path = bootstrap_log_path();
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)
        .map_err(|error| format!("Could not create {}: {error}", log_path.display()))?;
    append_log(&format!(
        "Building the release engine binary in {}...\n",
        target_dir.display()
    ));
    reporter.set("Building release engine", 0.12);
    match run_cargo_build(&workspace, &target_dir, reporter)? {
        status if status.success() => {}
        status => return Err(format!("The release engine build failed with {status}")),
    }

    reporter.set("Launching the rebuilt release engine", 0.96);
    let mut restart = Command::new(&release_executable);
    restart.current_dir(&working_directory).args(&restart_args);
    hide_console(&mut restart);
    restart.spawn().map_err(|error| {
        format!(
            "The release build succeeded, but {} could not be launched: {error}",
            release_executable.display()
        )
    })?;
    append_log(&format!(
        "\nRelease engine rebuilt from source and launched from {}.\n",
        release_executable.display()
    ));
    Ok(())
}

/// The rebuild failed: offer to relaunch the last stable engine or quit, and
/// keep offering if restoring it fails too.
fn recover(
    window: Option<ProgressWindow>,
    stable_executable: Option<&Path>,
    args: &[OsString],
    failure: &str,
) {
    let mut relaunch = ask_after_failure(window, failure);
    while relaunch {
        match restore_and_relaunch(stable_executable, args) {
            Ok(()) => return,
            Err(error) => {
                append_log(&format!(
                    "Could not restore the last stable engine: {error}\n"
                ));
                relaunch = ask_after_restore_failure(&error);
            }
        }
    }
}

fn recovery_buttons(relaunch_label: &str) -> Vec<DialogButton> {
    vec![
        DialogButton::new(RELAUNCH, relaunch_label)
            .kind(ButtonKind::Primary)
            .default(),
        DialogButton::new(QUIT, "Quit").cancel(),
    ]
}

/// Show the failure, in the progress window if there is one, else as a dialog.
/// `true` means relaunch the last stable engine.
fn ask_after_failure(window: Option<ProgressWindow>, failure: &str) -> bool {
    let log = format!("Bootstrap log: {}", bootstrap_log_path().display());
    match window {
        Some(window) => window
            .fail(
                Some("Engine rebuild failed"),
                failure,
                Some(&log),
                recovery_buttons("Relaunch last stable"),
            )
            .is(RELAUNCH),
        None => {
            let mut request = DialogRequest::new("Engine rebuild failed", failure)
                .severity(Severity::Error)
                .detail(log);
            for button in recovery_buttons("Relaunch last stable") {
                request = request.button(button);
            }
            client::show(&request).is(RELAUNCH)
        }
    }
}

fn ask_after_restore_failure(error: &str) -> bool {
    let mut request = DialogRequest::new("Could not restore the last stable engine", error)
        .severity(Severity::Error);
    for button in recovery_buttons("Try again") {
        request = request.button(button);
    }
    client::show(&request).is(RELAUNCH)
}

// std::fs allowed: restart sidecar: restores the engine's own executable file (process plumbing, no project)
#[allow(clippy::disallowed_methods)]
fn restore_and_relaunch(stable_executable: Option<&Path>, args: &[OsString]) -> Result<(), String> {
    let executable = arg(args, 4).ok().map(PathBuf::from);
    let working_directory = arg(args, 5).ok().map(PathBuf::from);
    let (Some(stable_executable), Some(executable), Some(working_directory)) = (
        stable_executable,
        executable.as_deref(),
        working_directory.as_deref(),
    ) else {
        return Err("The saved executable path is unavailable".into());
    };

    fs::copy(stable_executable, executable).map_err(|error| error.to_string())?;
    let mut restart = Command::new(executable);
    restart
        .current_dir(working_directory)
        .args(args.get(6..).unwrap_or_default());
    hide_console(&mut restart);
    restart
        .spawn()
        .map_err(|error| format!("could not relaunch the last stable engine: {error}"))?;
    Ok(())
}

// std::fs allowed: restart sidecar: opens the bootstrap log in the OS temp dir as cargo's output (process plumbing)
#[allow(clippy::disallowed_methods)]
fn run_cargo_build(
    workspace: &Path,
    target_dir: &Path,
    reporter: &mut Reporter,
) -> Result<std::process::ExitStatus, String> {
    let log_path = bootstrap_log_path();
    let log = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("Could not open {}: {error}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .map_err(|error| format!("Could not open bootstrap log: {error}"))?;
    let workers = (thread::available_parallelism().map_or(1, usize::from) / 2).max(1);
    let worker_arg = workers.to_string();
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(workspace)
        .args(["build", "-p", "pulsar_engine", "--release"])
        .args(["-j", worker_arg.as_str()])
        .arg("--target-dir")
        .arg(target_dir)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    hide_console(&mut cargo);

    let mut child = cargo
        .spawn()
        .map_err(|error| format!("could not start cargo: {error}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect cargo build: {error}"))?
        {
            return Ok(status);
        }
        let progress = 0.15 + 0.77 * (1.0 - (-(started.elapsed().as_secs_f32() / 100.0)).exp());
        reporter.set("Building release engine", progress.min(0.92));
        thread::sleep(Duration::from_millis(350));
    }
}

fn cargo_target_dir(workspace: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"))
}

fn release_executable_path(target_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    let binary_name = "pulsar_engine.exe";
    #[cfg(not(windows))]
    let binary_name = "pulsar_engine";
    target_dir.join("release").join(binary_name)
}

fn arg(args: &[OsString], index: usize) -> Result<&OsString, String> {
    args.get(index)
        .ok_or_else(|| format!("Missing sidecar argument at position {index}"))
}

fn wait_for_process_exit(pid: u32) -> Result<(), String> {
    loop {
        #[cfg(windows)]
        let alive = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
            .map_err(|error| format!("Could not inspect editor process: {error}"))?;

        #[cfg(unix)]
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .map_err(|error| format!("Could not inspect editor process: {error}"))?;

        if !alive {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn bootstrap_log_path() -> PathBuf {
    std::env::temp_dir().join("pulsar-engine-bootstrap.log")
}

// std::fs allowed: restart sidecar: appends to the bootstrap log in the OS temp dir (process plumbing)
#[allow(clippy::disallowed_methods)]
fn append_log(message: &str) {
    if let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(bootstrap_log_path())
    {
        let _ = log.write_all(message.as_bytes());
    }
}

// std::fs allowed: restart sidecar: removes the sidecar copy of the engine executable (process plumbing)
#[allow(clippy::disallowed_methods)]
fn schedule_sidecar_cleanup(sidecar: Option<PathBuf>) {
    let Some(sidecar) = sidecar else { return };
    #[cfg(windows)]
    {
        let command = format!(
            "timeout /t 2 /nobreak >NUL & del /F /Q \"{}\"",
            sidecar.display()
        );
        let _ = Command::new("cmd")
            .args(["/C", &command])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::fs::remove_file(sidecar);
    }
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(not(windows))]
    let _ = command;
}
