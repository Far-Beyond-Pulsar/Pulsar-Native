use super::*;

impl AgentChatPanel {
    /// Request auth token from the user for a given provider.
    /// In the new config-based system, auth is handled by the provider's
    /// config fields. This function is kept as a stub during migration.
    pub(super) fn start_auth_flow(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        tracing::warn!("auth_flow called — auth is now handled via provider config fields");
    }

    pub(super) fn cancel_auth(&mut self, _cx: &mut Context<Self>) {
    }
}
