use super::lifecycle::{HookContext, WindowHook};
use crate::validation::errors::HookResult;
use engine_state::EngineContext;

pub struct LoggingHook;

impl WindowHook for LoggingHook {
    fn execute(&self, context: &HookContext) -> HookResult<()> {
        tracing::info!(
            hook_type = ?context.hook_type,
            window_id = ?context.window_id,
            window_type = ?context.window_type,
            "Window hook executed"
        );
        Ok(())
    }

    fn priority(&self) -> i32 {
        -100
    }

    fn is_blocking(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "LoggingHook"
    }
}

pub struct TelemetryHook;

impl WindowHook for TelemetryHook {
    fn execute(&self, context: &HookContext) -> HookResult<()> {
        tracing::debug!(
            hook_type = ?context.hook_type,
            window_id = ?context.window_id,
            "Telemetry hook executed"
        );
        Ok(())
    }

    fn priority(&self) -> i32 {
        -90
    }

    fn is_blocking(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "TelemetryHook"
    }
}

pub struct EngineContextSyncHook {
    engine_context: EngineContext,
}

impl EngineContextSyncHook {
    pub fn new(engine_context: EngineContext) -> Self {
        Self { engine_context }
    }
}

impl WindowHook for EngineContextSyncHook {
    fn execute(&self, context: &HookContext) -> HookResult<()> {
        use super::lifecycle::HookType;

        match context.hook_type {
            HookType::AfterCreate => {
                if let (Some(window_id), Some(window_type)) = (context.window_id, &context.window_type) {
                    tracing::debug!("Registering window {} in EngineContext", window_id);
                    self.engine_context.register_window(
                        window_id,
                        engine_state::WindowContext::new(window_id, window_type.clone()),
                    );
                }
            }
            HookType::AfterClose => {
                if let Some(window_id) = context.window_id {
                    tracing::debug!("Unregistering window {} from EngineContext", window_id);
                    self.engine_context.unregister_window(&window_id);
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn priority(&self) -> i32 {
        100
    }

    fn is_blocking(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "EngineContextSyncHook"
    }
}
