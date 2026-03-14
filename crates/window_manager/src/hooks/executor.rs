use super::lifecycle::{HookContext, WindowHook};
use crate::validation::errors::HookResult;

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

