//! The configurator's general, Rust build mode and advanced sections, and the
//! card every section sits in.

use engine_state::build_config::{BuildConfiguration, BuildProfile};
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use rust_i18n::t;
use ui::input::TextInput;
use ui::{h_flex, v_flex};

use super::{BuildConfiguratorWindow, Palette};
use crate::text::{profile_label, profile_summary};

/// A titled card. `description` explains the section in one line.
pub(super) fn section(
    title: String,
    description: String,
    p: Palette,
    content: impl IntoElement,
) -> AnyElement {
    v_flex()
        .gap_3()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .child(
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(div().text_xs().text_color(p.muted).child(description)),
        )
        .child(content)
        .into_any_element()
}

/// A label above a control.
pub(super) fn labeled(label: String, p: Palette, control: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(p.muted).child(label))
        .child(control)
}

impl BuildConfiguratorWindow {
    pub(super) fn render_general(
        &mut self,
        _config: &BuildConfiguration,
        p: Palette,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let error = self.name_error.clone();
        section(
            t!("Build.General.Title").to_string(),
            t!("Build.General.Description").to_string(),
            p,
            v_flex()
                .gap_3()
                .child(labeled(
                    t!("Build.General.Name").to_string(),
                    p,
                    v_flex()
                        .gap_1()
                        .child(TextInput::new(&self.name))
                        .children(error.map(|e| div().text_xs().text_color(p.danger).child(e))),
                ))
                .child(labeled(
                    t!("Build.General.ConfigDescription").to_string(),
                    p,
                    TextInput::new(&self.description),
                )),
        )
    }

    pub(super) fn render_profile(
        &mut self,
        config: &BuildConfiguration,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = config.profile;
        let cards = BuildProfile::ALL.map(|profile| {
            let selected = profile == current;
            div()
                .id(SharedString::from(format!("bc-profile-{profile:?}")))
                .flex_1()
                .min_w_0()
                .p_3()
                .gap_1()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(if selected { p.primary } else { p.border })
                .bg(if selected {
                    p.primary.opacity(0.09)
                } else {
                    p.card.opacity(0.0)
                })
                .cursor_pointer()
                .hover(|s| s.bg(p.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.edit(cx, |c| c.profile = profile)))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(profile_label(profile)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(profile_summary(profile)),
                )
        });
        section(
            t!("Build.Profile.Title").to_string(),
            t!("Build.Profile.Description").to_string(),
            p,
            h_flex().gap_3().items_stretch().children(cards),
        )
    }

    pub(super) fn render_advanced(&mut self, p: Palette, _cx: &mut Context<Self>) -> AnyElement {
        section(
            t!("Build.Advanced.Title").to_string(),
            t!("Build.Advanced.Description").to_string(),
            p,
            v_flex()
                .gap_3()
                .child(labeled(
                    t!("Build.Advanced.Features").to_string(),
                    p,
                    TextInput::new(&self.features),
                ))
                .child(labeled(
                    t!("Build.Advanced.ExtraArgs").to_string(),
                    p,
                    TextInput::new(&self.extra_args),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(t!("Build.Advanced.QuoteHint").to_string())
                        .px(px(0.)),
                ),
        )
    }
}
