//! The configurator's platform section: what the build targets.
//!
//! Nothing selected means "this machine". Otherwise every selected platform is
//! built in turn. The long list is grouped by family, collapsible, and filtered
//! by the search box.

use engine_state::build_config::{BuildConfiguration, PlatformFamily, TargetPlatform};
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use ui::button::Button;
use ui::checkbox::Checkbox;
use ui::input::TextInput;
use ui::scroll::Scrollbar;
use ui::{Icon, IconName, Sizable as _, h_flex, v_flex};

use super::form::section;
use super::{BuildConfiguratorWindow, Palette};

/// The "Desktop" preset: the three platforms most games ship on.
const DESKTOP: [TargetPlatform; 3] = [
    TargetPlatform::WindowsX86_64Msvc,
    TargetPlatform::LinuxX86_64Gnu,
    TargetPlatform::MacOsAarch64,
];

/// Whether `platform` passes the search `query` (lowercase, may be empty).
fn matches_query(platform: TargetPlatform, query: &str) -> bool {
    query.split_whitespace().all(|word| {
        platform.label().to_lowercase().contains(word)
            || platform.triple().is_some_and(|t| t.contains(word))
            || platform.family().label().to_lowercase().contains(word)
    })
}

impl BuildConfiguratorWindow {
    fn toggle_platform(&mut self, platform: TargetPlatform, cx: &mut Context<Self>) {
        self.edit(cx, |c| {
            match c.platforms.iter().position(|p| *p == platform) {
                Some(ix) => {
                    c.platforms.remove(ix);
                }
                None => c.platforms.push(platform),
            }
        });
    }

    fn toggle_family(&mut self, family: PlatformFamily, cx: &mut Context<Self>) {
        if !self.expanded.remove(&family) {
            self.expanded.insert(family);
        }
        cx.notify();
    }

    pub(super) fn render_platforms(
        &mut self,
        config: &BuildConfiguration,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = config.platforms.clone();
        let query = self.platform_search.read(cx).value().to_lowercase();

        // What is selected, as removable chips.
        let chips: Vec<AnyElement> = if selected.is_empty() {
            vec![chip("This machine (default)", None, p, cx)]
        } else {
            selected.iter().map(|&platform| chip(platform.label(), Some(platform), p, cx)).collect()
        };

        let presets = h_flex()
            .gap_2()
            .child(
                Button::new("bc-plat-host")
                    .small()
                    .label("This machine")
                    .tooltip("Build for the platform this editor runs on")
                    .on_click(cx.listener(|this, _, _, cx| this.edit(cx, |c| c.platforms.clear()))),
            )
            .child(
                Button::new("bc-plat-desktop")
                    .small()
                    .label("Desktop")
                    .tooltip("Windows x64, Linux x64 and macOS Apple Silicon")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit(cx, |c| c.platforms = DESKTOP.to_vec())
                    })),
            );

        // The grouped list.
        let mut groups: Vec<AnyElement> = Vec::new();
        for family in PlatformFamily::ALL {
            let members: Vec<TargetPlatform> = TargetPlatform::ALL
                .iter()
                .copied()
                .filter(|p| p.family() == family && matches_query(*p, &query))
                .collect();
            if members.is_empty() {
                continue;
            }
            // Searching opens every group that has a hit.
            let open = !query.is_empty() || self.expanded.contains(&family);
            let chosen = members.iter().filter(|m| selected.contains(m)).count();

            let header = h_flex()
                .id(SharedString::from(format!("bc-fam-{family:?}")))
                .w_full()
                .px_2()
                .py(px(5.))
                .gap_2()
                .items_center()
                .cursor_pointer()
                .rounded_md()
                .hover(|s| s.bg(p.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_family(family, cx)))
                .child(
                    Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                        .size(px(14.))
                        .text_color(p.muted),
                )
                .child(div().flex_1().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(family.label()))
                .child(
                    div()
                        .text_xs()
                        .text_color(if chosen > 0 { p.primary } else { p.muted })
                        .child(format!("{chosen} of {}", members.len())),
                );

            let rows = open.then(|| {
                v_flex().pl_6().children(members.into_iter().map(|platform| {
                    platform_row(platform, selected.contains(&platform), p, cx)
                }))
            });
            groups.push(v_flex().child(header).children(rows).into_any_element());
        }
        let nothing_found = groups.is_empty();

        section(
            "Platforms",
            "Each selected platform is built in turn. Leave empty to build for this machine.",
            p,
            v_flex()
                .gap_3()
                .child(h_flex().gap_2().flex_wrap().children(chips))
                .child(presets)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Search).size(px(14.)).text_color(p.muted))
                        .child(div().flex_1().child(TextInput::new(&self.platform_search).small())),
                )
                .child(
                    div()
                        .id("bc-plat")
                        .relative()
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .id("bc-plat-scroll")
                                .max_h(px(300.))
                                .overflow_y_scroll()
                                .track_scroll(&self.platform_scroll)
                                .p_1()
                                .when(nothing_found, |el| {
                                    el.child(
                                        div()
                                            .p_3()
                                            .text_sm()
                                            .text_color(p.muted)
                                            .child("No platforms match your search."),
                                    )
                                })
                                .children(groups),
                        )
                        .child(Scrollbar::vertical(
                                    &self.platform_scroll_state,
                                    &self.platform_scroll,
                                )),
                ),
        )
    }
}

/// A selected platform, with a button to remove it (`None` is the default chip).
fn chip(
    label: &'static str,
    platform: Option<TargetPlatform>,
    p: Palette,
    cx: &mut Context<BuildConfiguratorWindow>,
) -> AnyElement {
    h_flex()
        .gap_1()
        .pl_3()
        .pr_2()
        .py(px(3.))
        .items_center()
        .rounded_full()
        .bg(p.primary.opacity(0.14))
        .text_xs()
        .child(label)
        .children(platform.map(|platform| {
            div()
                .id(SharedString::from(format!("bc-chip-{}", platform.id())))
                .cursor_pointer()
                .rounded_full()
                .hover(|s| s.bg(p.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_platform(platform, cx)))
                .child(Icon::new(IconName::Close).size(px(12.)))
        }))
        .into_any_element()
}

fn platform_row(
    platform: TargetPlatform,
    selected: bool,
    p: Palette,
    cx: &mut Context<BuildConfiguratorWindow>,
) -> AnyElement {
    let detail = platform.triple().unwrap_or("Needs the console SDK; cargo cannot build it");
    let row = h_flex()
        .w_full()
        .px_2()
        .py(px(3.))
        .gap_3()
        .items_center();
    if platform.is_buildable() {
        row.child(
            Checkbox::new(SharedString::from(format!("bc-plat-{}", platform.id())))
                .label(platform.label())
                .checked(selected)
                .on_click(cx.listener(move |this, _: &bool, _, cx| this.toggle_platform(platform, cx))),
        )
        .child(div().flex_1())
        .child(div().text_xs().text_color(p.muted).child(detail))
        .into_any_element()
    } else {
        row.opacity(0.55)
            .child(div().text_sm().child(platform.label()))
            .child(div().flex_1())
            .child(div().text_xs().text_color(p.muted).child(detail))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_label_triple_and_family() {
        let win = TargetPlatform::WindowsX86_64Msvc;
        assert!(matches_query(win, ""));
        assert!(matches_query(win, "windows"));
        assert!(matches_query(win, "x64"));
        assert!(matches_query(win, "pc-windows-msvc"));
        assert!(matches_query(win, "win msvc"), "every word, any field");
        assert!(!matches_query(win, "linux"));
    }
}
