use gpui::*;
use ui::{
    h_flex,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme as _,
};

use crate::components::{
    chat_tab::render_chat_tab, file_sync_tab::render_file_sync_tab,
    presence_tab::render_presence_tab, session_info::render_session_info_tab,
};
use crate::handlers;
use crate::screen::MultiplayerWindow;
use crate::utils::types::{ActiveSession, FileSyncStatus, SessionTab};

pub fn render_tab_bar(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let selected_index = match this.current_tab {
        SessionTab::Session => 0,
        SessionTab::FileSync => 1,
    };
    let out_of_sync_count = this
        .file_assets
        .iter()
        .filter(|asset| asset.status != FileSyncStatus::Synced)
        .count();
    let labels: [SharedString; 2] = [
        "Session".into(),
        if out_of_sync_count > 0 {
            format!("Files ({out_of_sync_count})").into()
        } else {
            "Files".into()
        },
    ];
    TabBar::new("multiplayer-tabs")
        .w_full()
        .bg(cx.theme().background)
        .border_b_1()
        .border_color(cx.theme().border)
        .selected_index(selected_index)
        .on_click(cx.listener(|this, ix: &usize, _window, cx| handlers::on_tab_click(this, ix, cx)))
        .build_tabs(
            labels.len(),
            labels
                .iter()
                .map(|label| (Some(label.clone()), false))
                .collect(),
            {
                let labels = labels.clone();
                move |ix, _, _| Tab::new(labels[ix].clone())
            },
        )
}

fn render_session_workspace(
    this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    h_flex()
        .size_full()
        .min_h_0()
        .min_w_0()
        .child(
            v_flex()
                .w(px(420.))
                .h_full()
                .flex_shrink_0()
                .min_h_0()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(render_session_info_tab(this, session, cx))
                .child(div().h(px(1.)).w_full().bg(cx.theme().border))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(render_presence_tab(this, cx)),
                ),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(render_chat_tab(this, cx)),
        )
}

pub fn render_active_session(
    this: &MultiplayerWindow,
    session: &ActiveSession,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .min_h_0()
        .child(render_tab_bar(this, cx))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(match this.current_tab {
                    SessionTab::Session => {
                        render_session_workspace(this, session, cx).into_any_element()
                    }
                    SessionTab::FileSync => render_file_sync_tab(this, cx).into_any_element(),
                }),
        )
}
