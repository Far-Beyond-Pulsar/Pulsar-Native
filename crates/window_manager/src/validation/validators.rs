use crate::commands::WindowCommand;
use crate::state::WindowState;
use crate::validation::errors::{WindowError, WindowResult};
use parking_lot::RwLock;
use std::sync::Arc;

pub trait ValidationRule: Send + Sync {
    fn validate(&self, command: &WindowCommand, state: &WindowState) -> WindowResult<()>;
    fn name(&self) -> &'static str;
}

pub struct WindowValidator {
    rules: Arc<RwLock<Vec<Box<dyn ValidationRule>>>>,
}

impl WindowValidator {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn add_rule(&self, rule: Box<dyn ValidationRule>) {
        let mut rules = self.rules.write();
        rules.push(rule);
    }

    pub fn validate(&self, command: &WindowCommand, state: &WindowState) -> WindowResult<()> {
        match command {
            WindowCommand::Close(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }

                if !cmd.force && state.window_count() == 1 {
                    return Err(WindowError::CannotClose(
                        cmd.window_id,
                        "Cannot close the last window".to_string(),
                    ));
                }
            }
            WindowCommand::Focus(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::Minimize(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::Maximize(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::Move(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::Resize(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::UpdateTitle(cmd) => {
                if !state.window_exists(cmd.window_id) {
                    return Err(WindowError::WindowNotFound(cmd.window_id));
                }
            }
            WindowCommand::Create(_) => {
            }
        }

        let rules = self.rules.read();
        for rule in rules.iter() {
            tracing::debug!("Validating with rule: {}", rule.name());
            rule.validate(command, state)?;
        }

        Ok(())
    }

    pub fn clear_rules(&self) {
        let mut rules = self.rules.write();
        rules.clear();
    }

    pub fn rule_count(&self) -> usize {
        self.rules.read().len()
    }
}

impl Default for WindowValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WindowValidator {
    fn clone(&self) -> Self {
        Self {
            rules: Arc::clone(&self.rules),
        }
    }
}

pub struct MaxWindowsRule {
    max_windows: usize,
}

impl MaxWindowsRule {
    pub fn new(max_windows: usize) -> Self {
        Self { max_windows }
    }
}

impl ValidationRule for MaxWindowsRule {
    fn validate(&self, command: &WindowCommand, state: &WindowState) -> WindowResult<()> {
        if let WindowCommand::Create(_) = command {
            if state.window_count() >= self.max_windows {
                return Err(WindowError::ValidationFailed(format!(
                    "Maximum window count ({}) reached",
                    self.max_windows
                )));
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "MaxWindowsRule"
    }
}
