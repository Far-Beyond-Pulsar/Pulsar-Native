use gpui::{*, prelude::FluentBuilder};
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, divider::Divider};
use ui_types_common::TraitAsset;
use std::path::PathBuf;

pub struct TraitEditor {
    file_path: Option<PathBuf>,
    asset: Option<TraitAsset>,
    error_message: Option<String>,
    focus_handle: FocusHandle,
}

impl TraitEditor {
    pub fn new_with_file(file_path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the trait data
        let (asset, error_message) = match std::fs::read_to_string(&file_path) {
            Ok(json_content) => {
                match serde_json::from_str::<TraitAsset>(&json_content) {
                    Ok(asset) => (Some(asset), None),
                    Err(e) => (None, Some(format!("Failed to parse trait: {}", e))),
                }
            }
            Err(e) => (None, Some(format!("Failed to read file: {}", e))),
        };

        Self {
            file_path: Some(file_path),
            asset,
            error_message,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }
}

impl Render for TraitEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Header
                v_flex()
                    .w_full()
                    .p_4()
                    .bg(cx.theme().secondary.opacity(0.5))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_xl()
                                    .child("🔧")
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(
                                        self.asset.as_ref()
                                            .map(|a| a.display_name.clone())
                                            .unwrap_or_else(|| "Trait Editor".to_string())
                                    )
                            )
                    )
                    .when(self.asset.is_some(), |this| {
                        let asset = self.asset.as_ref().unwrap();
                        this.child(
                            div()
                                .mt_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("Name: {}", asset.name))
                        )
                    })
            )
            .child(
                // Content
                v_flex()
                    .flex_1()
                    .p_4()
                    .gap_4()
                    .overflow_hidden()
                    .when(self.error_message.is_some(), |this| {
                        let error = self.error_message.as_ref().unwrap();
                        this.child(
                            div()
                                .p_4()
                                .bg(hsla(0.0, 0.8, 0.5, 0.1))
                                .border_1()
                                .border_color(hsla(0.0, 0.8, 0.5, 1.0))
                                .rounded(px(6.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(hsla(0.0, 0.8, 0.5, 1.0))
                                        .child(error.clone())
                                )
                        )
                    })
                    .when(self.asset.is_some(), |this| {
                        let asset = self.asset.as_ref().unwrap();
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child("Description")
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    asset.description.clone()
                                                        .unwrap_or_else(|| "No description".to_string())
                                                )
                                        )
                                )
                                .child(Divider::horizontal())
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child(format!("Methods ({})", asset.methods.len()))
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .children(
                                                    asset.methods.iter().map(|method| {
                                                        h_flex()
                                                            .gap_3()
                                                            .p_2()
                                                            .bg(cx.theme().secondary.opacity(0.3))
                                                            .rounded(px(4.0))
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .font_medium()
                                                                    .text_color(cx.theme().foreground)
                                                                    .child(method.name.clone())
                                                            )
                                                    })
                                                )
                                        )
                                )
                        )
                    })
            )
    }
}

impl Focusable for TraitEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TraitEditor {}

impl Panel for TraitEditor {
    fn panel_name(&self) -> &'static str {
        "Trait Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        self.asset.as_ref()
            .map(|a| a.display_name.clone())
            .unwrap_or_else(|| "Trait".to_string())
            .into_any_element()
    }

}
