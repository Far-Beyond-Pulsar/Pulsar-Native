use super::*;

impl AgentChatPanel {
    pub(super) fn rollback_chat_to_message(
        &mut self,
        display_ix: usize,
        message_index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.is_request_in_flight || display_ix >= self.display_items.len() {
            return;
        }

        self.pending_rollback_confirm_ix = None;
        self.reset_steward();
        self.display_items.truncate(display_ix + 1);
        self.ledger.truncate(message_index + 1);
        if self.ledger.is_empty() {
            self.ledger
                .push(LedgerEntry::system(Self::default_system_prompt(&self.tool_registry)));
        }

        self.streaming_message_ix = None;
        self.streaming_display_item_ix = None;
        self.display_item_heights.clear();
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn fork_chat_here(
        &mut self,
        display_ix: usize,
        message_index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.is_request_in_flight || display_ix >= self.display_items.len() {
            return;
        }

        self.pending_rollback_confirm_ix = None;
        self.reset_steward();
        self.display_items.truncate(display_ix + 1);
        let mut forked_ledger = self.ledger.prefix(message_index + 1);
        if forked_ledger.is_empty() {
            forked_ledger.push(LedgerEntry::system(Self::default_system_prompt(
                &self.tool_registry,
            )));
        }

        // The fork keeps the runs its history refers to, so read_run still works.
        let old_runs = Self::ensure_chats_dir().map(|dir| {
            agent_runtime::RunStore::path_for_chat(&dir, &self.current_chat_id)
        });
        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        if let (Some(old), Some(dir)) = (old_runs, Self::ensure_chats_dir()) {
            if old.exists() {
                let _ = std::fs::copy(
                    &old,
                    agent_runtime::RunStore::path_for_chat(&dir, &self.current_chat_id),
                );
            }
        }
        self.ledger = forked_ledger;
        self.attach_delegation(cx);
        self.streaming_message_ix = None;
        self.streaming_display_item_ix = None;
        self.display_item_heights.clear();

        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn request_rollback_confirmation(
        &mut self,
        display_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if self.is_request_in_flight || display_ix >= self.display_items.len() {
            return;
        }

        self.pending_rollback_confirm_ix = Some(display_ix);
        cx.notify();
    }

    pub(super) fn cancel_rollback_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_rollback_confirm_ix = None;
        cx.notify();
    }
}
