//! Which mappings bind this action.

use gpui::*;
use ui::scroll::ScrollbarAxis;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt};

use super::super::common::chrome::{empty_message, panel_header};
use super::super::common::panel::dock_panel;
use super::super::form::chip;
use super::super::open_asset;
use super::ActionCtx;

pub struct UsagePanel {
    ctx: ActionCtx,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(UsagePanel, "input-action-usage", "Used by");

impl UsagePanel {
    pub fn new(ctx: ActionCtx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify()), cx.observe(&ctx.lib, |_, _, cx| cx.notify())];
        Self { ctx, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

impl Render for UsagePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = self.ctx.asset(cx).id;
        let library = self.ctx.lib.read(cx).library();
        let users = library.mappings_using(&id);

        let rows: Vec<AnyElement> = users
            .iter()
            .enumerate()
            .map(|(n, mapping)| {
                let keys: Vec<String> = mapping
                    .asset
                    .entries()
                    .filter(|(_, e)| e.action_id() == Some(&id))
                    .map(|(_, e)| crate::keys::catalog().get(&e.key).map_or(e.key.clone(), |k| k.label.clone()))
                    .collect();
                let path = mapping.path.clone();
                h_flex()
                    .id(SharedString::from(format!("usage-{n}")))
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|r| r.bg(theme.accent.opacity(0.14)))
                    .on_click(move |_, window, cx| open_asset(path.clone(), window, cx))
                    .child(Icon::new(IconName::Key).size(px(14.0)).text_color(theme.info))
                    .child(div().text_sm().text_color(theme.foreground).child(mapping.stem.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().text_color(theme.muted_foreground).child(keys.join(", ")))
                    .child(chip(format!("{} keys", keys.len()), cx))
                    .into_any_element()
            })
            .collect();

        let body = if rows.is_empty() {
            empty_message(IconName::Key, "No mapping uses this action", "Bind it to a key in an input mapping and it will show up here.", cx).into_any_element()
        } else {
            v_flex().w_full().p_2().gap_0p5().children(rows).into_any_element()
        };
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Key, "Used by", Some(chip(format!("{}", users.len()), cx).into_any_element()), cx))
            .child(v_flex().id("input-action-usage-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(body))
    }
}
