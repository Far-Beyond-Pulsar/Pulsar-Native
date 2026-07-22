use gpui::*;
use ui::{
    h_flex,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;
use crate::utils::types::{ActiveSession, FileSyncStatus, SessionTab};
use crate::handlers;
use crate::components::{
    chat_tab::render_chat_tab,
    file_sync_tab::render_file_sync_tab,
    presence_tab::render_presence_tab,
    session_info::render_session_info_tab,
};

pub fn render_tab_bar(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let selected_index = match this.current_tab {
        SessionTab::Info => 0,
        SessionTab::Presence => 1,
        SessionTab::FileSync => 2,
        SessionTab::Chat => 3,
    };

    let out_of_sync_count = this
        .file_assets
        .iter()
        .filter(|a| a.status != FileSyncStatus::Synced)
        .count();

    let tab_labels: [SharedString; 4] = [
        "Info".into(),
        "Presence".into(),
        if out_of_sync_count > 0 {
            format!("Files ({})", out_of_sync_count).into()
        } else {
            "Files".into()
        },
        if !this.chat_messages.is_empty() {
            format!("Chat ({})", this.chat_messages.len()).into()
        } else {
            "Chat".into()
        },
    ];
    let tab_count = tab_labels.len();

    TabBar::new("multiplayer-tabs")
        .w_full()
        .bg(cx.theme().secondary)
        .border_b_1()
        .border_color(cx.theme().border)
        .selected_index(selected_index)
        .on_click(cx.listener(|this, ix: &usize, _window, cx| {
            handlers::on_tab_click(this, ix, cx);
        }))
        .build_tabs(
            tab_count,
            tab_labels.iter().map(|l| (Some(l.clone()), false)).collect(),
            {
                let labels = tab_labels.clone();
                move |ix, _, _| Tab::new(labels[ix].clone())
            },
        )
}

pub fn render_active_session(
    this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .child(render_tab_bar(this, cx))
        .child(match this.current_tab {
            SessionTab::Info => render_session_info_tab(this, session, cx).into_any_element(),
            SessionTab::Presence => render_presence_tab(this, cx).into_any_element(),
            SessionTab::FileSync => render_file_sync_tab(this, cx).into_any_element(),
            SessionTab::Chat => render_chat_tab(this, cx).into_any_element(),
        })
}
