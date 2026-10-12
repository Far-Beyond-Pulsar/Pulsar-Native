//! Sidecar mode for rebuilding and relaunching the release engine executable.
//!
//! The sidecar keeps the old binary available while Cargo replaces the release
//! executable. Its GPUI window provides the same progress and failure UI on
//! Windows, macOS, and Linux.

use gpui::{prelude::*, *};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const SIDECAR_ARG: &str = "--pulsar-bootstrap-sidecar-run";

type SharedState = Arc<(Mutex<BootstrapState>, Condvar)>;

#[derive(Default)]
struct BootstrapState {
    stage: String,
    progress: f32,
    failure: Option<String>,
    restore_error: Option<String>,
    choice: Option<bool>,
    finished: bool,
}

/// Handle the internal sidecar invocation before normal engine startup.
/// Returns `true` when the process should exit instead of starting the UI.
pub fn run_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != SIDECAR_ARG) {
        return false;
    }

    let sidecar = std::env::current_exe().ok();
    run_sidecar_ui(args, sidecar.clone());
    schedule_sidecar_cleanup(sidecar);
    true
}

fn run_sidecar_ui(args: Vec<OsString>, stable_executable: Option<PathBuf>) {
    let shared: SharedState = Arc::new((
        Mutex::new(BootstrapState {
            stage: "Waiting for the editor to close".into(),
            progress: 0.04,
            ..Default::default()
        }),
        Condvar::new(),
    ));
    let worker_state = shared.clone();
    thread::spawn(move || run_bootstrap(&args, stable_executable.as_deref(), &worker_state));

    Application::new().run(|cx| {
        ui::init(cx);
        ui::Theme::change(ui::ThemeMode::Dark, None, cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(480.0), px(230.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("Bootstrap Engine".into()),
                    ..Default::default()
                }),
                is_resizable: false,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| BootstrapView::new(shared, window, cx)),
        )
        .expect("could not open engine bootstrap window");
        cx.activate(true);
    });
}

struct BootstrapView {
    shared: SharedState,
    _poll_task: Task<()>,
}

impl BootstrapView {
    fn new(shared: SharedState, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll_shared = shared.clone();
        let poll_task = cx.spawn(async move |this, cx| loop {
            smol::Timer::after(Duration::from_millis(150)).await;
            let finished = poll_shared
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finished;
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |_, cx| cx.notify());
                }
                if finished {
                    cx.quit();
                }
            });
            if finished {
                break;
            }
        });
        Self {
            shared,
            _poll_task: poll_task,
        }
    }
}

impl Drop for BootstrapView {
    fn drop(&mut self) {
        choose(&self.shared, false);
    }
}

impl Render for BootstrapView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (stage, progress, failure, restore_error) = {
            let state = self
                .shared
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.stage.clone(),
                state.progress,
                state.failure.clone(),
                state.restore_error.clone(),
            )
        };
        let shared_for_relaunch = self.shared.clone();
        let shared_for_close = self.shared.clone();
        let is_failure = failure.is_some();

        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap_4()
            .px_6()
            .py_5()
            .bg(rgb(0x202124))
            .text_color(rgb(0xf1f3f4))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if is_failure {
                        "Engine rebuild failed"
                    } else {
                        "Rebuilding engine"
                    }),
            )
            .child(div().text_sm().text_color(rgb(0xb8c0cc)).child(stage))
            .when(!is_failure, |el| {
                el.child(
                    div()
                        .w_full()
                        .h(px(8.0))
                        .rounded(px(4.0))
                        .bg(rgb(0x41464f))
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(relative(progress.clamp(0.02, 1.0)))
                                .rounded(px(4.0))
                                .bg(rgb(0x75a7ff)),
                        ),
                )
            })
            .when_some(failure, |el, message| {
                el.child(div().text_sm().text_color(rgb(0xffb4ab)).child(format!(
                    "{message}\n\nBootstrap log: {}",
                    bootstrap_log_path().display()
                )))
            })
            .when_some(restore_error, |el, error| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xffb4ab))
                        .child(format!("Could not restore the last stable engine: {error}")),
                )
            })
            .when(is_failure, |el| {
                el.child(
                    div()
                        .flex()
                        .gap_3()
                        .child(action_button("Relaunch last stable", move || {
                            choose(&shared_for_relaunch, true)
                        }))
                        .child(action_button("Quit", move || {
                            choose(&shared_for_close, false)
                        })),
                )
            })
    }
}

fn action_button(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    div()
        .px_3()
        .py_2()
        .rounded(px(4.0))
        .bg(rgb(0x343a46))
        .hover(|style| style.bg(rgb(0x465267)))
        .cursor_pointer()
        .text_sm()
        .child(label)
        .on_click(move |_, _, _| action())
}

fn choose(shared: &SharedState, choice: bool) {
    let (lock, condition) = &**shared;
    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.choice = Some(choice);
    condition.notify_all();
}

fn run_bootstrap(args: &[OsString], stable_executable: Option<&Path>, shared: &SharedState) {
    let result = (|| -> Result<(), String> {
        let parent_pid = arg(args, 2)?
            .to_string_lossy()
            .parse::<u32>()
            .map_err(|error| format!("Invalid parent process id: {error}"))?;
        let workspace = PathBuf::from(arg(args, 3)?);
        let working_directory = PathBuf::from(arg(args, 5)?);
        let restart_args = args.get(6..).unwrap_or_default().to_vec();
        let release_executable = release_executable_path(&workspace);

        wait_for_process_exit(parent_pid)?;
        let log_path = bootstrap_log_path();
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&log_path)
            .map_err(|error| format!("Could not create {}: {error}", log_path.display()))?;
        append_log("Building debug and release engine binaries...\n");
        let mut build_errors = Vec::new();
        for (profile_name, release, progress_base, progress_span) in
            [("debug", false, 0.12, 0.37), ("release", true, 0.50, 0.42)]
        {
            set_progress(
                shared,
                &format!("Building {profile_name} engine"),
                progress_base,
            );
            match run_cargo_build(&workspace, release, shared, progress_base, progress_span) {
                Ok(status) if status.success() => {}
                Ok(status) => build_errors.push(format!(
                    "The {profile_name} engine build failed with {status}"
                )),
                Err(error) => {
                    build_errors.push(format!("Could not run the {profile_name} build: {error}"))
                }
            }
        }
        if !build_errors.is_empty() {
            return Err(build_errors.join("\n"));
        }

        set_progress(shared, "Launching the rebuilt release engine", 0.96);
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
    })();

    match result {
        Ok(()) => finish(shared, "Engine rebuilt and relaunched"),
        Err(failure) => {
            append_log(&format!("\nBootstrap failed: {failure}\n"));
            show_failure_and_wait(shared, stable_executable, args, &failure);
        }
    }
}

fn show_failure_and_wait(
    shared: &SharedState,
    stable_executable: Option<&Path>,
    args: &[OsString],
    failure: &str,
) {
    let executable = arg(args, 4).ok().map(PathBuf::from);
    let working_directory = arg(args, 5).ok().map(PathBuf::from);
    {
        let mut state = shared
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.stage = failure.to_owned();
        state.failure = Some(failure.to_owned());
        state.progress = 1.0;
    }

    loop {
        let choice = {
            let (lock, condition) = &**shared;
            let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            while state.choice.is_none() {
                state = condition
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            state.choice.take().unwrap_or(false)
        };
        if !choice {
            finish(shared, "Bootstrap closed");
            return;
        }

        let (Some(stable_executable), Some(executable), Some(working_directory)) = (
            stable_executable,
            executable.as_deref(),
            working_directory.as_deref(),
        ) else {
            set_restore_error(shared, "The saved executable path is unavailable".into());
            continue;
        };

        if let Err(error) = fs::copy(stable_executable, executable) {
            append_log(&format!(
                "Could not restore the last stable executable: {error}\n"
            ));
            set_restore_error(shared, error.to_string());
            continue;
        }

        let restart_args = args.get(6..).unwrap_or_default();
        let mut restart = Command::new(executable);
        restart.current_dir(working_directory).args(restart_args);
        hide_console(&mut restart);
        if let Err(error) = restart.spawn() {
            append_log(&format!("Could not relaunch last stable engine: {error}\n"));
            set_restore_error(shared, error.to_string());
            continue;
        }
        finish(shared, "Last stable engine relaunched");
        return;
    }
}

fn run_cargo_build(
    workspace: &Path,
    release: bool,
    shared: &SharedState,
    progress_base: f32,
    progress_span: f32,
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
        .args(["build", "-p", "pulsar_engine"]);
    if release {
        cargo.arg("--release");
    }
    cargo
        .args(["-j", worker_arg.as_str()])
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
        let progress = progress_base
            + progress_span * (1.0 - (-(started.elapsed().as_secs_f32() / 100.0)).exp());
        set_progress(
            shared,
            if release {
                "Building release engine"
            } else {
                "Building debug engine"
            },
            progress.min(progress_base + progress_span * 0.96),
        );
        thread::sleep(Duration::from_millis(350));
    }
}

fn release_executable_path(workspace: &Path) -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"));
    #[cfg(windows)]
    let binary_name = "pulsar_engine.exe";
    #[cfg(not(windows))]
    let binary_name = "pulsar_engine";
    target_dir.join("release").join(binary_name)
}

fn set_progress(shared: &SharedState, stage: &str, progress: f32) {
    let mut state = shared
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.stage = stage.to_owned();
    state.progress = progress.clamp(0.0, 1.0);
}

fn set_restore_error(shared: &SharedState, error: String) {
    let mut state = shared
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.restore_error = Some(error);
    state.choice = None;
}

fn finish(shared: &SharedState, stage: &str) {
    let mut state = shared
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.stage = stage.to_owned();
    state.finished = true;
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

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(not(windows))]
    let _ = command;
}
