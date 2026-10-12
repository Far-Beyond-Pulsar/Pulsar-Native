//! Sidecar mode for rebuilding the engine executable that launched the editor.
//!
//! The menu action starts a copy of the current executable from the system
//! temp directory. That copy waits for the editor to exit, builds the engine,
//! then starts the newly built executable with the original arguments.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const SIDECAR_ARG: &str = "--pulsar-bootstrap-sidecar-run";

/// Handle the internal sidecar invocation before normal engine startup.
/// Returns `true` when the process should exit instead of starting the UI.
pub fn run_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != SIDECAR_ARG) {
        return false;
    }

    let sidecar = std::env::current_exe().ok();
    if let Err(error) = run_sidecar(&args, sidecar.as_deref()) {
        append_log(&format!("Bootstrap failed: {error}\n"));
    }
    schedule_sidecar_cleanup(sidecar);
    true
}

fn run_sidecar(
    args: &[OsString],
    stable_executable: Option<&std::path::Path>,
) -> Result<(), String> {
    let parent_pid = arg(args, 2)?
        .to_string_lossy()
        .parse::<u32>()
        .map_err(|error| format!("Invalid parent process id: {error}"))?;
    let workspace = PathBuf::from(arg(args, 3)?);
    let executable = PathBuf::from(arg(args, 4)?);
    let working_directory = PathBuf::from(arg(args, 5)?);
    let restart_args = args[6..].to_vec();

    wait_for_process_exit(parent_pid)?;

    let log_path = bootstrap_log_path();
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)
        .map_err(|error| format!("Could not create {}: {error}", log_path.display()))?;
    append_log("Building debug and release engine binaries...\n");

    for release in [false, true] {
        let profile_name = if release { "release" } else { "debug" };
        let status = match run_cargo_build(&workspace, release) {
            Ok(status) => status,
            Err(error) => {
                let message = format!("Could not run the {profile_name} build: {error}");
                append_log(&format!("\n{message}\n"));
                offer_stable_relaunch(
                    stable_executable,
                    &executable,
                    &working_directory,
                    &restart_args,
                    &message,
                );
                return Ok(());
            }
        };
        if !status.success() {
            let message = format!("The {profile_name} engine build failed with {status}");
            append_log(&format!("\n{message}.\n"));
            offer_stable_relaunch(
                stable_executable,
                &executable,
                &working_directory,
                &restart_args,
                &message,
            );
            return Ok(());
        }
    }

    let active_executable = executable.clone();
    let mut restart = Command::new(&active_executable);
    restart.current_dir(working_directory).args(restart_args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        restart.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    restart
        .spawn()
        .map_err(|error| format!("Could not relaunch {}: {error}", executable.display()))?;

    append_log(&format!(
        "\nEngine rebuilt from source and relaunched from {}.\n",
        active_executable.display()
    ));
    Ok(())
}

fn run_cargo_build(
    workspace: &std::path::Path,
    release: bool,
) -> Result<std::process::ExitStatus, String> {
    let log_path = bootstrap_log_path();
    let log = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .map_err(|error| format!("Could not open {}: {error}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .map_err(|error| format!("Could not open bootstrap log: {error}"))?;
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(workspace)
        .args(["build", "-p", "pulsar_engine"]);
    if release {
        cargo.arg("--release");
    }
    cargo.stdout(Stdio::from(log)).stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cargo.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    cargo
        .status()
        .map_err(|error| format!("could not start cargo: {error}"))
}

fn offer_stable_relaunch(
    stable_executable: Option<&std::path::Path>,
    executable: &std::path::Path,
    working_directory: &std::path::Path,
    restart_args: &[OsString],
    failure: &str,
) {
    let log_path = bootstrap_log_path();
    let choice = rfd::MessageDialog::new()
        .set_title("Engine Bootstrap Failed")
        .set_description(format!(
            "The engine source could not be rebuilt. {failure}.\n\nBootstrap log: {}\n\nRelaunch the last stable engine version?",
            log_path.display()
        ))
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    if !matches!(choice, rfd::MessageDialogResult::Yes) {
        return;
    }

    let Some(stable_executable) = stable_executable else {
        append_log("Could not locate the saved stable executable for relaunch.\n");
        return;
    };
    if let Err(error) = fs::copy(stable_executable, executable) {
        append_log(&format!(
            "Could not restore the last stable executable: {error}\n"
        ));
        let _ = rfd::MessageDialog::new()
            .set_title("Could Not Restore Engine")
            .set_description(format!(
                "The last stable executable could not be restored.\n\n{error}\n\nBootstrap log: {}",
                log_path.display()
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
        return;
    }

    let mut restart = Command::new(executable);
    restart.current_dir(working_directory).args(restart_args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        restart.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Err(error) = restart.spawn() {
        append_log(&format!("Could not relaunch last stable engine: {error}\n"));
    }
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

fn append_log(message: &str) {
    if let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(bootstrap_log_path())
    {
        let _ = log.write_all(message.as_bytes());
    }
}

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
