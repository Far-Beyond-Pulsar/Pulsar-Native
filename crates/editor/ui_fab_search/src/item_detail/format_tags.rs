use gpui::{prelude::*, *};
use ui::{ActiveTheme, Sizable as _, StyledExt, tag::Tag, v_flex};

/// Engine/file format codes mapped to a friendly display name.
const FORMAT_DISPLAY: &[(&str, &str)] = &[
    ("unreal-engine", "Unreal Engine"),
    ("unity", "Unity"),
    ("blender", "Blender"),
    ("fbx", "FBX"),
    ("gltf", "glTF"),
    ("obj", "OBJ"),
    ("usd", "USD"),
    ("maya", "Maya"),
    ("3ds-max", "3ds Max"),
];

fn display_name(code: &str, _raw_name: &str) -> &'static str {
    FORMAT_DISPLAY
        .iter()
        .find(|(k, _)| *k == code)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| {
            // Fall through to the raw_name at the call-site; we return a &'static placeholder
            // and let the caller use raw_name when this returns the sentinel.
            ""
        })
}

/// Displays asset format badges (Unreal, Unity, Blender, …) and keyword tags.
#[derive(IntoElement)]
pub struct FormatTagsSection {
    pub formats: Vec<(String, String)>, // (code, display_name)
    pub tags: Vec<SharedString>,
}

impl FormatTagsSection {
    pub fn new(formats: Vec<(String, String)>, tags: Vec<SharedString>) -> Self {
        Self { formats, tags }
    }
}

impl RenderOnce for FormatTagsSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(border)
            // ── engine formats ───────────────────────────────────────────
            .when(!self.formats.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_bold()
                                .text_color(muted)
                                .child("Compatible Formats"),
                        )
                        .child(div().flex().flex_row().flex_wrap().gap_2().children(
                            self.formats.iter().map(|(code, name)| {
                                let label = {
                                    let mapped = display_name(code.as_str(), name.as_str());
                                    if mapped.is_empty() {
                                        name.clone()
                                    } else {
                                        mapped.to_string()
                                    }
                                };
                                Tag::primary().rounded_full().small().child(label)
                            }),
                        )),
                )
            })
            // ── keyword tags ─────────────────────────────────────────────
            .when(!self.tags.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_xs().font_bold().text_color(muted).child("Tags"))
                        .child(
                            div().flex().flex_row().flex_wrap().gap_2().children(
                                self.tags.iter().map(|t| {
                                    Tag::secondary().rounded_full().small().child(t.clone())
                                }),
                            ),
                        ),
                )
            })
    }
}
