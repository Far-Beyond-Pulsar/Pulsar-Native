use gpui::*;

/// Floating toolbar for toggling rendering features
#[derive(Default)]
pub struct FeatureToolbar {
    pub lighting_enabled: bool,
    pub shadows_enabled: bool,
    pub bloom_enabled: bool,
    pub materials_enabled: bool,
    pub base_geometry_enabled: bool,
}

impl FeatureToolbar {
    pub fn new() -> Self {
        Self {
            lighting_enabled: true,
            shadows_enabled: true,
            bloom_enabled: true,
            materials_enabled: true,
            base_geometry_enabled: true,
        }
    }
    
    pub fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(60.0))
            .right(px(10.0))
            .p_2()
            .bg(rgb(0x1e1e1e))
            .rounded_md()
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("🎨 Rendering Features")
            )
            .child(div().h_px().bg(rgb(0x404040)))
            .child(self.checkbox_row("Base Geometry", self.base_geometry_enabled))
            .child(self.checkbox_row("Materials", self.materials_enabled))
            .child(self.checkbox_row("Lighting", self.lighting_enabled))
            .child(self.checkbox_row("Shadows", self.shadows_enabled))
            .child(self.checkbox_row("Bloom", self.bloom_enabled))
    }
    
    fn checkbox_row(&self, label: &str, checked: bool) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .border_1()
                    .border_color(rgb(0x808080))
                    .rounded_sm()
                    .when(checked, |this| {
                        this.bg(rgb(0x4080ff))
                    })
            )
            .child(
                div()
                    .text_xs()
                    .child(label)
            )
    }
}
