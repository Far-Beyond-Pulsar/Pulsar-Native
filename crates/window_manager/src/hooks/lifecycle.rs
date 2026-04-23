use crate::commands::{WindowCommand, WindowCommandResult};
use crate::validation::errors::{HookError, HookResult};
use std::collections::HashMap;
use ui_types_common::window_types::{WindowId, WindowRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookType {
    BeforeCreate,
    AfterCreate,
    BeforeClose,
    AfterClose,
    BeforeFocus,
    AfterFocus,
    BeforeMinimize,
    AfterMinimize,
    BeforeMaximize,
    AfterMaximize,
}

pub struct HookContext {
    pub hook_type: HookType,
    pub window_id: Option<WindowId>,
    pub window_type: Option<WindowRequest>,
    pub metadata: HashMap<String, String>,
}

impl HookContext {
    pub fn new(hook_type: HookType) -> Self {
        Self {
            hook_type,
            window_id: None,
            window_type: None,
            metadata: HashMap::new(),
        }
    }

    pub fn from_command(command: &WindowCommand) -> Self {
        match command {
            WindowCommand::Create(cmd) => {
                let mut ctx = Self::new(HookType::BeforeCreate);
                ctx.window_type = Some(cmd.window_type.clone());
                ctx.metadata.insert(
                    "parent_window".to_string(),
                    format!("{:?}", cmd.parent_window),
                );
                ctx
            }
            WindowCommand::Close(cmd) => {
                let mut ctx = Self::new(HookType::BeforeClose);
                ctx.window_id = Some(cmd.window_id);
                ctx.metadata
                    .insert("force".to_string(), cmd.force.to_string());
                ctx
            }
            WindowCommand::Focus(cmd) => {
                let mut ctx = Self::new(HookType::BeforeFocus);
                ctx.window_id = Some(cmd.window_id);
                ctx
            }
            WindowCommand::Minimize(cmd) => {
                let mut ctx = Self::new(HookType::BeforeMinimize);
                ctx.window_id = Some(cmd.window_id);
                ctx
            }
            WindowCommand::Maximize(cmd) => {
                let mut ctx = Self::new(HookType::BeforeMaximize);
                ctx.window_id = Some(cmd.window_id);
                ctx.metadata
                    .insert("restore".to_string(), cmd.restore.to_string());
                ctx
            }
            WindowCommand::Move(cmd) => {
                let mut ctx = Self::new(HookType::BeforeCreate);
                ctx.window_id = Some(cmd.window_id);
                ctx
            }
            WindowCommand::Resize(cmd) => {
                let mut ctx = Self::new(HookType::BeforeCreate);
                ctx.window_id = Some(cmd.window_id);
                ctx
            }
            WindowCommand::UpdateTitle(cmd) => {
                let mut ctx = Self::new(HookType::BeforeCreate);
                ctx.window_id = Some(cmd.window_id);
                ctx.metadata.insert("title".to_string(), cmd.title.clone());
                ctx
            }
        }
    }

    pub fn from_result(result: &WindowCommandResult) -> Self {
        match result {
            WindowCommandResult::Created { window_id } => {
                let mut ctx = Self::new(HookType::AfterCreate);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Closed { window_id } => {
                let mut ctx = Self::new(HookType::AfterClose);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Focused { window_id } => {
                let mut ctx = Self::new(HookType::AfterFocus);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Minimized { window_id } => {
                let mut ctx = Self::new(HookType::AfterMinimize);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Maximized { window_id } => {
                let mut ctx = Self::new(HookType::AfterMaximize);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Moved { window_id } => {
                let mut ctx = Self::new(HookType::AfterCreate);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::Resized { window_id } => {
                let mut ctx = Self::new(HookType::AfterCreate);
                ctx.window_id = Some(*window_id);
                ctx
            }
            WindowCommandResult::TitleUpdated { window_id } => {
                let mut ctx = Self::new(HookType::AfterCreate);
                ctx.window_id = Some(*window_id);
                ctx
            }
        }
    }

    pub fn with_window_id(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }

    pub fn with_window_type(mut self, window_type: WindowRequest) -> Self {
        self.window_type = Some(window_type);
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

pub trait WindowHook: Send + Sync {
    fn execute(&self, context: &HookContext) -> HookResult<()>;

    fn priority(&self) -> i32 {
        0
    }

    fn is_blocking(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "UnnamedHook"
    }
}
