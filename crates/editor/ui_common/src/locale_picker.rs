use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, DismissEvent, Entity,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled as _, Window,
};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{
    h_flex,
    input::{InputState, TextInput},
    set_locale, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
};

const ALL_LOCALES: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh-CN", "简体中文 (Simplified Chinese)"),
    ("zh-HK", "繁體中文 (Traditional Chinese)"),
    ("ru", "Русский (Russian)"),
    ("it", "Italiano (Italian)"),
    ("de", "Deutsch (German)"),
    ("pt-BR", "Português (Portuguese)"),
    ("fr", "Français (French)"),
    ("fr-CA", "Français (Canadian French)"),
    ("hi", "हिन्दी (Hindi)"),
    ("ar", "العربية (Arabic)"),
    ("ja", "日本語 (Japanese)"),
    ("es", "Español (Spanish)"),
    ("ko", "한국어 (Korean)"),
    ("uk", "Українська (Ukrainian)"),
    ("lol", "Lolcat"),
];

pub struct LocalePicker {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
}

impl LocalePicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search languages…"));
        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            scroll_handle: ScrollHandle::new(),
            scroll_state: ScrollbarState::default(),
        }
    }
}

impl gpui::EventEmitter<DismissEvent> for LocalePicker {}

impl Focusable for LocalePicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LocalePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().to_string();
        let query_lower = query.to_lowercase();

        let current_locale = ui::locale().to_string();

        let locales: Vec<&(&str, &str)> = ALL_LOCALES
            .iter()
            .filter(|(code, name)| {
                query_lower.is_empty()
                    || code.to_lowercase().contains(&query_lower)
                    || name.to_lowercase().contains(&query_lower)
            })
            .collect();

        let is_empty = locales.is_empty();

        let bg = cx.theme().background;
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let hover_bg = cx.theme().secondary;
        let active_bg = cx.theme().list_active;

        v_flex()
            .w(px(280.))
            .bg(bg)
            .rounded_xl()
            .shadow_xl()
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .px_2()
                    .py(px(6.))
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .child(TextInput::new(&self.search_input).small()),
                    ),
            )
            .child(
                div()
                    .id("locale-picker-list")
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("locale-picker-scroll")
                            .max_h(px(400.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .py_1()
                            .when(is_empty, |el| {
                                el.child(
                                    div()
                                        .px_4()
                                        .py_4()
                                        .text_sm()
                                        .text_color(muted)
                                        .child("No languages match your search."),
                                )
                            })
                            .children(locales.into_iter().map(|(code, name)| {
                                let is_active = *code == current_locale;
                                let code_for_click = *code;
                                let name_for_display = *name;

                                h_flex()
                                    .id(SharedString::from(format!("locale-{}", code)))
                                    .w_full()
                                    .px_3()
                                    .py(px(6.))
                                    .gap_2()
                                    .items_center()
                                    .cursor_pointer()
                                    .bg(if is_active { active_bg } else { bg })
                                    .hover(|s| s.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            set_locale(code_for_click);
                                            cx.refresh_windows();
                                            cx.emit(DismissEvent);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(format!("{}  ({})", name_for_display, code)),
                                    )
                                    .when(is_active, |el| {
                                        el.child(
                                            Icon::new(IconName::Check)
                                                .size(px(14.))
                                                .text_color(fg),
                                        )
                                    })
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(
                                &self.scroll_state,
                                &self.scroll_handle,
                            )),
                    ),
            )
    }
}
