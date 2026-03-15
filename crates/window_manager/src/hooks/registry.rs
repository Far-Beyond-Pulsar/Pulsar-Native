use super::lifecycle::{HookContext, HookType, WindowHook};
use crate::validation::errors::{HookError, HookResult};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

pub struct HookRegistry {
    hooks: Arc<RwLock<HashMap<HookType, Vec<Box<dyn WindowHook>>>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_hook(&self, hook_type: HookType, hook: Box<dyn WindowHook>) {
        let mut hooks = self.hooks.write();
        let hook_list = hooks.entry(hook_type).or_insert_with(Vec::new);
        hook_list.push(hook);
        hook_list.sort_by_key(|h| h.priority());
    }

    pub fn execute_before(&self, context: &HookContext) -> HookResult<()> {
        let hooks = self.hooks.read();

        if let Some(hook_list) = hooks.get(&context.hook_type) {
            for hook in hook_list.iter() {
                tracing::debug!("Executing before hook: {} for {:?}", hook.name(), context.hook_type);

                match hook.execute(context) {
                    Ok(()) => {
                        tracing::debug!("Hook {} completed successfully", hook.name());
                    }
                    Err(e) => {
                        if hook.is_blocking() {
                            tracing::error!("Blocking hook {} failed: {}", hook.name(), e);
                            return Err(HookError::BlockingHookFailed(format!(
                                "Hook '{}' failed: {}",
                                hook.name(),
                                e
                            )));
                        } else {
                            tracing::warn!("Non-blocking hook {} failed: {}", hook.name(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn execute_after(&self, context: &HookContext) -> HookResult<()> {
        let hooks = self.hooks.read();

        if let Some(hook_list) = hooks.get(&context.hook_type) {
            for hook in hook_list.iter() {
                tracing::debug!("Executing after hook: {} for {:?}", hook.name(), context.hook_type);

                match hook.execute(context) {
                    Ok(()) => {
                        tracing::debug!("Hook {} completed successfully", hook.name());
                    }
                    Err(e) => {
                        if hook.is_blocking() {
                            tracing::error!("Blocking hook {} failed: {}", hook.name(), e);
                            return Err(HookError::BlockingHookFailed(format!(
                                "Hook '{}' failed: {}",
                                hook.name(),
                                e
                            )));
                        } else {
                            tracing::warn!("Non-blocking hook {} failed: {}", hook.name(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn clear_hooks(&self, hook_type: HookType) {
        let mut hooks = self.hooks.write();
        hooks.remove(&hook_type);
    }

    pub fn clear_all_hooks(&self) {
        let mut hooks = self.hooks.write();
        hooks.clear();
    }

    pub fn hook_count(&self, hook_type: HookType) -> usize {
        let hooks = self.hooks.read();
        hooks.get(&hook_type).map(|list| list.len()).unwrap_or(0)
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HookRegistry {
    fn clone(&self) -> Self {
        Self {
            hooks: Arc::clone(&self.hooks),
        }
    }
}
