use gpui::*;

use crate::screen::MultiplayerWindow;
use crate::utils::types::SessionTab;

pub fn on_create_session(
    this: &mut MultiplayerWindow,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.create_session(window, cx);
}

pub fn on_join_session(
    this: &mut MultiplayerWindow,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.join_session(window, cx);
}

pub fn on_disconnect(
    this: &mut MultiplayerWindow,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.disconnect(window, cx);
}

pub fn on_send_chat(
    this: &mut MultiplayerWindow,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.send_chat_message(window, cx);
}

pub fn on_sync_approve(
    this: &mut MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.approve_file_sync(cx);
}

pub fn on_sync_cancel(
    this: &mut MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.cancel_file_sync(cx);
}

pub fn on_jump_to_user(
    this: &mut MultiplayerWindow,
    peer_id: String,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.jump_to_user_view(peer_id, window, cx);
}

pub fn on_kick_user(
    this: &mut MultiplayerWindow,
    peer_id: String,
    window: &mut Window,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.kick_user(peer_id, window, cx);
}

pub fn on_tab_click(
    this: &mut MultiplayerWindow,
    ix: &usize,
    cx: &mut Context<MultiplayerWindow>,
) {
    this.current_tab = match ix {
        0 => SessionTab::Info,
        1 => SessionTab::Presence,
        2 => SessionTab::FileSync,
        3 => SessionTab::Chat,
        _ => SessionTab::Info,
    };
    cx.notify();
}
