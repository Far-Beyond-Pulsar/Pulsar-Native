//! System Info panel — static hardware and OS information.

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::system_info::SharedSystemInfo;

pub struct SystemInfoPanel {
    focus_handle: FocusHandle,
    system_info: SharedSystemInfo,
}

impl SystemInfoPanel {
    pub fn new(system_info: SharedSystemInfo, cx: &mut Context<Self>) -> Self {
        Self { focus_handle: cx.focus_handle(), system_info }
    }

    fn info_row(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
        use ui::h_flex;
        let theme = cx.theme();
        h_flex()
            .w_full()
            .justify_between()
            .gap_2()
            .child(div().text_size(px(11.0)).text_color(theme.muted_foreground).child(label.into()))
            .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.foreground).child(value.into()))
    }

    fn section_header(title: impl Into<SharedString>, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        div().w_full().text_size(px(12.0)).font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.accent).pb_2().child(title.into())
    }
}

impl EventEmitter<PanelEvent> for SystemInfoPanel {}

impl Render for SystemInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();
        let info = self.system_info.read();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_3()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                h_flex().items_center().gap_2()
                    .child(div().text_size(px(14.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground).child("System Information"))
            )
            .child(
                v_flex().w_full().gap_1()
                    .child(Self::section_header("Operating System", cx))
                    .child(Self::info_row("OS", &info.os_name, cx))
                    .child(Self::info_row("Version", &info.os_version, cx))
                    .child(Self::info_row("Kernel", &info.kernel_version, cx))
                    .child(Self::info_row("Hostname", &info.host_name, cx))
                    .child(Self::info_row("Uptime", info.uptime_formatted(), cx))
            )
            .child(
                v_flex().w_full().gap_1()
                    .child(Self::section_header("Processor", cx))
                    .child(Self::info_row("Model", &info.cpu_brand, cx))
                    .child(Self::info_row("Vendor", &info.cpu_vendor, cx))
                    .child(Self::info_row("Cores", format!("{} cores", info.cpu_cores), cx))
                    .child(Self::info_row("Frequency", format!("{} MHz", info.cpu_frequency), cx))
            )
            .child(
                v_flex().w_full().gap_1()
                    .child(Self::section_header("Memory", cx))
                    .child(Self::info_row("Total RAM", format!("{:.2} GB", info.total_memory_gb()), cx))
                    .child(Self::info_row("Total Swap", format!("{:.2} GB", info.total_swap_gb()), cx))
            )
            .child(
                v_flex().w_full().gap_1()
                    .child(Self::section_header("Graphics", cx))
                    .child(Self::info_row("GPU", &info.gpu_name, cx))
                    .child(Self::info_row("Vendor", &info.gpu_vendor, cx))
                    .child(Self::info_row("Driver", &info.gpu_driver_version, cx))
                    .child(Self::info_row("VRAM", info.gpu_vram_formatted(), cx))
            )
            .child(
                v_flex().w_full().gap_1()
                    .child(Self::section_header("Engine", cx))
                    .child(Self::info_row("Renderer", "Helio (D3D12)", cx))
                    .child(Self::info_row("Backend", "Blade", cx))
            )
    }
}

impl Focusable for SystemInfoPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for SystemInfoPanel {
    fn panel_name(&self) -> &'static str { "system_info" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "System Info".into_any_element() }
}
