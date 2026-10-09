//! The configurator's general, Rust build mode and advanced sections, and the
//! card every section sits in.

use engine_state::build_config::{BuildConfiguration, BuildProfile};
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use ui::input::TextInput;
use ui::{h_flex, v_flex};

use super::{BuildConfiguratorWindow, Palette};

/// A titled card. `description` explains the section in one line.
pub(super) fn section(
    title: &'static str,
    description: &'static str,
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
pub(super) fn labeled(
    label: &'static str,
    p: Palette,
    control: impl IntoElement,
) -> impl IntoElement {
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
            "General",
            "How this configuration appears in the Build menu.",
            p,
            v_flex()
                .gap_3()
                .child(labeled(
                    "Name",
                    p,
                    v_flex()
                        .gap_1()
                        .child(TextInput::new(&self.name))
                        .children(error.map(|e| div().text_xs().text_color(p.danger).child(e))),
                ))
                .child(labeled("Description", p, TextInput::new(&self.description))),
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
                .id(SharedString::from(format!(
                    "bc-profile-{}",
                    profile.label()
                )))
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
                        .child(profile.label()),
                )
                .child(div().text_xs().text_color(p.muted).child(profile.summary()))
        });
        section(
            "Rust build mode",
            "The cargo profile every compile step uses.",
            p,
            h_flex().gap_3().items_stretch().children(cards),
        )
    }

    pub(super) fn render_advanced(&mut self, p: Palette, _cx: &mut Context<Self>) -> AnyElement {
        section(
            "Advanced",
            "Passed to every cargo check, build and run.",
            p,
            v_flex()
                .gap_3()
                .child(labeled(
                    "Cargo features (comma or space separated)",
                    p,
                    TextInput::new(&self.features),
                ))
                .child(labeled(
                    "Extra cargo arguments",
                    p,
                    TextInput::new(&self.extra_args),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child("Quote an argument that contains spaces: --config \"build.jobs=4\"")
                        .px(px(0.)),
                ),
        )
    }
}
