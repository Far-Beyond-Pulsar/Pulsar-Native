use friends_engine::{FriendInfo, FriendsError, RelationStatus};
use gpui::{prelude::*, *};
use ui::{
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    skeleton::Skeleton,
    v_flex, ActiveTheme as _, Icon, IconName,
};

pub struct FriendsPopover {
    pub friends_list: Entity<SearchableList<String>>,
    loading: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for FriendsPopover {}

impl FriendsPopover {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let friends_list = cx.new(|cx| {
            SearchableList::<String>::new(window, cx, Vec::<String>::new(), |u: &String| u.clone())
                .with_empty_text("No friends yet")
                .with_max_width(px(280.0))
                .with_max_height(px(360.0))
        });

        let subscriptions = vec![cx.subscribe(
            &friends_list,
            |_this: &mut Self, _list, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(_) = event {
                    cx.emit(DismissEvent);
                }
            },
        )];

        let mut slf = Self {
            friends_list,
            loading: false,
            _subscriptions: subscriptions,
        };

        if friends_engine::is_authenticated() {
            slf.loading = true;
            slf.refresh(cx);
        }

        slf
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        if !friends_engine::is_authenticated() {
            self.loading = false;
            self.friends_list.update(cx, |list, cx| list.set_items(Vec::new(), cx));
            cx.notify();
            return;
        }

        let (tx, rx) = smol::channel::bounded::<Result<Vec<FriendInfo>, FriendsError>>(1);

        std::thread::spawn(move || {
            let result = friends_engine::get_friends_list();
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or(Err(FriendsError::Network(
                "Channel closed".to_string(),
            )));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.loading = false;
                    match result {
                        Ok(list) => {
                            let friends_usernames: Vec<String> = list
                                .into_iter()
                                .filter(|f| f.relation_status == RelationStatus::Mutual)
                                .map(|f| f.username)
                                .collect();
                            screen.friends_list.update(cx, |list, cx| {
                                list.set_items(friends_usernames, cx)
                            });
                        }
                        Err(e) => {
                            tracing::error!("[FriendsPopover] Failed to load friends: {:?}", e);
                            screen.friends_list.update(cx, |list, cx| {
                                list.set_items(Vec::new(), cx)
                            });
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl Render for FriendsPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .w(px(320.))
            .p_4()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Group)
                            .size(px(16.))
                            .text_color(theme.foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Multiplayer Sessions"),
                    ),
            )
            .child(if self.loading {
                v_flex()
                    .w_full()
                    .gap_2()
                    .children((0..4).map(|i| {
                        h_flex()
                            .id(SharedString::from(format!("friend-skel-{}", i)))
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                Skeleton::new()
                                    .w(px(32.))
                                    .h(px(32.))
                                    .rounded(px(32.)),
                            )
                            .child(Skeleton::new().flex_1().h_4())
                    }))
                    .into_any_element()
            } else {
                self.friends_list.clone().into_any_element()
            })
    }
}
