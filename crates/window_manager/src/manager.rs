use crate::commands::{
    CloseWindowCommand, CreateWindowCommand, FocusWindowCommand, MaximizeWindowCommand,
    MinimizeWindowCommand, MoveWindowCommand, ResizeWindowCommand, UpdateTitleCommand,
    WindowCommand, WindowCommandResult,
};
use crate::hooks::{HookContext, HookRegistry, HookType, LoggingHook, TelemetryHook, WindowHook};
use crate::state::WindowState;
use crate::telemetry::TelemetrySender;
use crate::validation::{ValidationRule, WindowError, WindowResult, WindowValidator};
use gpui::{
    AnyWindowHandle, App, AppContext, Context, EventEmitter, Global, Render, Window, WindowOptions,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use ui_types_common::window_types::{WindowId, WindowRequest};

pub struct WindowManager {
    hooks: HookRegistry,
    validator: WindowValidator,
    state: WindowState,
    telemetry: TelemetrySender,
    next_id: Arc<AtomicU64>,
}

impl WindowManager {
    pub fn new() -> Self {
        let hooks = HookRegistry::new();

        // built-in hooks
        hooks.register_hook(HookType::AfterCreate, Box::new(LoggingHook));
        hooks.register_hook(HookType::AfterCreate, Box::new(TelemetryHook));
        hooks.register_hook(HookType::AfterClose, Box::new(LoggingHook));
        hooks.register_hook(HookType::AfterClose, Box::new(TelemetryHook));

        Self {
            hooks,
            validator: WindowValidator::new(),
            state: WindowState::new(),
            telemetry: TelemetrySender::new(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn register_hook(&self, hook_type: HookType, hook: Box<dyn WindowHook>) {
        self.hooks.register_hook(hook_type, hook);
    }

    pub fn add_validation_rule(&self, rule: Box<dyn ValidationRule>) {
        self.validator.add_rule(rule);
    }

    /// Create a new window via the manager. Returns GPUI handle on success.
    /// Generic version that preserves the view type - this is the preferred method.
    /// The content_builder should return Entity<V> (created with cx.new)
    pub fn create_window<V, F>(
        &self,
        window_type: WindowRequest,
        options: WindowOptions,
        content_builder: F,
        cx: &mut App,
    ) -> WindowResult<(WindowId, AnyWindowHandle)>
    where
        V: Render + 'static,
        F: FnOnce(&mut Window, &mut App) -> gpui::Entity<V> + Send + 'static,
    {
        // Build metadata-only command for telemetry, validation, and hooks.
        // The content_builder is passed directly to cx.open_window below —
        // no dummy/panic closure is needed.
        let command = WindowCommand::Create(CreateWindowCommand::new(
            window_type.clone(),
            WindowOptions::default(),
        ));

        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        let window_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let wtype = window_type.clone();

        let handle = cx
            .open_window(options, content_builder)
            .map_err(|e| WindowError::GpuiError(format!("{:?}", e)))?;

        let handle: AnyWindowHandle = handle.into();

        self.state.register_window(window_id, wtype.clone(), None);
        let result = WindowCommandResult::Created { window_id };
        self.telemetry.record_command_result(&result);
        let mut after = HookContext::from_result(&result);
        after.window_type = Some(wtype.clone());
        self.hooks.execute_after(&after)?;
        self.telemetry.record_window_created(window_id, &wtype);
        self.telemetry
            .record_window_count(self.state.window_count());
        Ok((window_id, handle))
    }

    /// Close an existing window through the manager.
    pub fn close_window(&self, window_id: WindowId, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Close(CloseWindowCommand::new(window_id));
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        window.remove_window();
        self.state.unregister_window(window_id);

        let result = WindowCommandResult::Closed { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        self.telemetry.record_window_closed(window_id);
        self.telemetry
            .record_window_count(self.state.window_count());
        Ok(())
    }

    /// Shared pipeline for the 6 simple window operations:
    /// validate → before-hooks → op (actual work + result) → after-hooks.
    fn run_operation<F>(&self, command: WindowCommand, op: F) -> WindowResult<()>
    where
        F: FnOnce() -> WindowCommandResult,
    {
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        let result = op();

        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    /// Focus a window.
    pub fn focus_window(&self, window_id: WindowId, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Focus(FocusWindowCommand { window_id });
        self.run_operation(command, || {
            window.activate_window();
            WindowCommandResult::Focused { window_id }
        })
    }

    pub fn minimize_window(&self, window_id: WindowId, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Minimize(MinimizeWindowCommand { window_id });
        self.run_operation(command, || {
            window.minimize_window();
            WindowCommandResult::Minimized { window_id }
        })
    }

    pub fn maximize_window(
        &self,
        window_id: WindowId,
        restore: bool,
        window: &mut Window,
    ) -> WindowResult<()> {
        let command = WindowCommand::Maximize(MaximizeWindowCommand { window_id, restore });
        self.run_operation(command, || {
            window.zoom_window();
            WindowCommandResult::Maximized { window_id }
        })
    }

    pub fn move_window(
        &self,
        window_id: WindowId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
    ) -> WindowResult<()> {
        let command = WindowCommand::Move(MoveWindowCommand {
            window_id,
            position,
        });
        self.run_operation(command, || {
            // Note: gpui does not expose a direct set_position API; position change is a no-op here.
            let _ = (position, window);
            WindowCommandResult::Moved { window_id }
        })
    }

    pub fn resize_window(
        &self,
        window_id: WindowId,
        size: gpui::Size<gpui::Pixels>,
        window: &mut Window,
    ) -> WindowResult<()> {
        let command = WindowCommand::Resize(ResizeWindowCommand { window_id, size });
        self.run_operation(command, || {
            window.set_rem_size(size.width);
            WindowCommandResult::Resized { window_id }
        })
    }

    pub fn update_title(
        &self,
        window_id: WindowId,
        title: String,
        window: &mut Window,
    ) -> WindowResult<()> {
        let command = WindowCommand::UpdateTitle(UpdateTitleCommand {
            window_id,
            title: title.clone(),
        });
        self.run_operation(command, || {
            window.set_window_title(&title);
            WindowCommandResult::TitleUpdated { window_id }
        })
    }

    pub fn window_count(&self) -> usize {
        self.state.window_count()
    }

    pub fn window_exists(&self, window_id: WindowId) -> bool {
        self.state.window_exists(window_id)
    }
}

impl Global for WindowManager {}

#[derive(Clone, PartialEq, Eq)]
pub enum WindowManagerEvent {
    WindowCreated { window_id: WindowId },
    WindowClosed { window_id: WindowId },
    WindowFocused { window_id: WindowId },
    CommandFailed { window_id: Option<WindowId> },
}

impl EventEmitter<WindowManagerEvent> for WindowManager {}
