use crate::commands::{
    CloseWindowCommand,
    CreateWindowCommand,
    FocusWindowCommand,
    MinimizeWindowCommand,
    MaximizeWindowCommand,
    MoveWindowCommand,
    ResizeWindowCommand,
    UpdateTitleCommand,
    WindowCommand,
    WindowCommandResult,
};
use crate::hooks::{HookContext, HookRegistry, HookType, LoggingHook, TelemetryHook, WindowHook};
use crate::state::WindowState;
use crate::telemetry::TelemetrySender;
use crate::validation::{ValidationRule, WindowError, WindowResult, WindowValidator};
use gpui::{AnyView, AnyWindowHandle, App, AppContext, Context, EventEmitter, Global, IntoElement, Render, Window, WindowOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use ui_types_common::window_types::{WindowId, WindowRequest};

struct AnyViewRoot(AnyView);

impl Render for AnyViewRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

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
    /// Create a new window and return both its handle and assigned ID.
    pub fn create_window<F>(
        &self,
        window_type: WindowRequest,
        options: WindowOptions,
        content_builder: F,
        cx: &mut App,
    ) -> WindowResult<(WindowId, AnyWindowHandle)>
    where
        F: FnOnce(&mut Window, &mut App) -> AnyView + Send + 'static,
    {
        let command = WindowCommand::Create(CreateWindowCommand::new(
            window_type.clone(),
            options,
            content_builder,
        ));

        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        let window_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let cmd = if let WindowCommand::Create(cmd) = command {
            cmd
        } else {
            unreachable!()
        };
        let content = cmd.content_builder;
        let wtype = window_type.clone();

        let handle = cx
            .open_window(cmd.options, move |window, cx| {
                let view = content(window, cx);
                cx.new(|_| AnyViewRoot(view))
            })
            .map_err(|e| WindowError::GpuiError(format!("{:?}", e)))?;

        let handle: AnyWindowHandle = handle.into();

        self.state.register_window(window_id, wtype.clone(), cmd.parent_window);
        let result = WindowCommandResult::Created { window_id };
        self.telemetry.record_command_result(&result);
        let mut after = HookContext::from_result(&result);
        after.window_type = Some(wtype.clone());
        self.hooks.execute_after(&after)?;
        self.telemetry.record_window_created(window_id, &wtype);
        self.telemetry.record_window_count(self.state.window_count());
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
        self.telemetry.record_window_count(self.state.window_count());
        Ok(())
    }

    /// Focus a window.
    pub fn focus_window(&self, window_id: WindowId, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Focus(FocusWindowCommand { window_id });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        // Activate the window to bring it to the foreground
        window.activate_window();

        let result = WindowCommandResult::Focused { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    pub fn minimize_window(&self, window_id: WindowId, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Minimize(MinimizeWindowCommand { window_id });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        window.minimize_window();

        let result = WindowCommandResult::Minimized { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    pub fn maximize_window(&self, window_id: WindowId, restore: bool, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Maximize(MaximizeWindowCommand { window_id, restore });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        window.zoom_window();

        let result = WindowCommandResult::Maximized { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    pub fn move_window(&self, window_id: WindowId, position: gpui::Point<gpui::Pixels>, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Move(MoveWindowCommand { window_id, position });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        // Note: gpui does not expose a direct set_position API; position change is a no-op here.
        let _ = position;

        let result = WindowCommandResult::Moved { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    pub fn resize_window(&self, window_id: WindowId, size: gpui::Size<gpui::Pixels>, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::Resize(ResizeWindowCommand { window_id, size });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        // Fix .set_rem_size(size) call to use a value convertible to Pixels
        window.set_rem_size(size.width);

        let result = WindowCommandResult::Resized { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
    }

    pub fn update_title(&self, window_id: WindowId, title: String, window: &mut Window) -> WindowResult<()> {
        let command = WindowCommand::UpdateTitle(UpdateTitleCommand { window_id, title: title.clone() });
        self.telemetry.record_command_executed(&command);
        self.validator.validate(&command, &self.state)?;
        let before = HookContext::from_command(&command);
        self.hooks.execute_before(&before)?;

        // Fix .set_window_title() call to match gpui API
        window.set_window_title(&title);

        let result = WindowCommandResult::TitleUpdated { window_id };
        self.telemetry.record_command_result(&result);
        let after = HookContext::from_result(&result);
        self.hooks.execute_after(&after)?;
        Ok(())
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