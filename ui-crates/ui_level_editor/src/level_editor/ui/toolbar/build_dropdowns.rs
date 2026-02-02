use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, IconName, Sizable, popup_menu::PopupMenuExt};
use std::sync::Arc;

use super::super::state::{LevelEditorState, BuildConfig, TargetPlatform};
use super::actions::{SetBuildConfig, SetTargetPlatform};

/// Build configuration and platform dropdowns - Comprehensive build settings for all 290+ Rust targets
pub struct BuildDropdowns;

impl BuildDropdowns {
    fn get_platform_label(platform: TargetPlatform) -> &'static str {
        match platform {
            // Windows
            TargetPlatform::WindowsX86_64Msvc => "Win x64",
            TargetPlatform::WindowsI686Msvc => "Win x86",
            TargetPlatform::WindowsAarch64Msvc => "Win ARM64",
            TargetPlatform::WindowsX86_64Gnu => "Win x64 GNU",
            TargetPlatform::WindowsI686Gnu => "Win x86 GNU",
            // Linux
            TargetPlatform::LinuxX86_64Gnu => "Linux x64",
            TargetPlatform::LinuxI686Gnu => "Linux x86",
            TargetPlatform::LinuxAarch64Gnu => "Linux ARM64",
            TargetPlatform::LinuxArmv7Gnueabihf => "Linux ARMv7",
            TargetPlatform::LinuxX86_64Musl => "Linux x64 musl",
            TargetPlatform::LinuxAarch64Musl => "Linux ARM64 musl",
            // Console platforms
            TargetPlatform::PlayStationPs4 => "PS4",
            TargetPlatform::PlayStationPs5 => "PS5",
            TargetPlatform::XboxOne => "Xbox One",
            TargetPlatform::XboxSeriesXS => "Xbox Series X|S",
            TargetPlatform::NintendoSwitch => "Nintendo Switch",
            _ => "Other",
        }
    }
    
    fn get_platform_icon(platform: TargetPlatform) -> IconName {
        match platform {
            TargetPlatform::WindowsX86_64Msvc | TargetPlatform::WindowsI686Msvc 
            | TargetPlatform::WindowsAarch64Msvc | TargetPlatform::WindowsX86_64Gnu 
            | TargetPlatform::WindowsI686Gnu => IconName::Box,
            
            TargetPlatform::LinuxX86_64Gnu | TargetPlatform::LinuxI686Gnu 
            | TargetPlatform::LinuxAarch64Gnu | TargetPlatform::LinuxArmv7Gnueabihf
            | TargetPlatform::LinuxArmGnueabi | TargetPlatform::LinuxArmGnueabihf
            | TargetPlatform::LinuxX86_64Musl | TargetPlatform::LinuxAarch64Musl
            | TargetPlatform::LinuxArmv7Musleabihf | TargetPlatform::LinuxMipselGnu
            | TargetPlatform::LinuxMipsGnu | TargetPlatform::LinuxMips64Gnuabi64
            | TargetPlatform::LinuxMips64elGnuabi64 | TargetPlatform::LinuxPowerpc64Gnu
            | TargetPlatform::LinuxPowerpc64leGnu | TargetPlatform::LinuxPowerpcGnu
            | TargetPlatform::LinuxRiscv64Gc | TargetPlatform::LinuxS390xGnu
            | TargetPlatform::LinuxSparcv9 | TargetPlatform::LinuxMipselMusl
            | TargetPlatform::LinuxMipsMusl => IconName::Server,
            
            TargetPlatform::MacOsX86_64 | TargetPlatform::MacOsAarch64 => IconName::Apple,
            
            TargetPlatform::AndroidAarch64 | TargetPlatform::AndroidArmv7 
            | TargetPlatform::AndroidI686 | TargetPlatform::AndroidX86_64 => IconName::Phone,
            
            TargetPlatform::IosAarch64 | TargetPlatform::IosX86_64 
            | TargetPlatform::IosAarch64Sim => IconName::Apple,
            
            TargetPlatform::FreeBsdX86_64 | TargetPlatform::FreeBsdI686
            | TargetPlatform::NetBsdX86_64 | TargetPlatform::OpenBsdX86_64
            | TargetPlatform::DragonFlyX86_64 => IconName::Server,
            
            TargetPlatform::SolarisSparcv9 | TargetPlatform::SolarisX86_64 
            | TargetPlatform::IlumosX86_64 => IconName::Sun,
            
            TargetPlatform::RedoxX86_64 => IconName::CPU,
            
            TargetPlatform::FuchsiaAarch64 | TargetPlatform::FuchsiaX86_64 => IconName::Hexagon,
            
            // Gaming Consoles
            TargetPlatform::PlayStationPs4 | TargetPlatform::PlayStationPs5 => IconName::Gamepad,
            TargetPlatform::XboxOne | TargetPlatform::XboxSeriesXS => IconName::Gamepad,
            TargetPlatform::NintendoSwitch => IconName::Gamepad,
        }
    }

    pub fn render<V: 'static>(
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let config_label = match state.build_config {
            BuildConfig::Debug => "Debug",
            BuildConfig::Release => "Release",
            BuildConfig::Shipping => "Ship",
        };
        
        let platform_label = Self::get_platform_label(state.target_platform);
        let platform_icon = Self::get_platform_icon(state.target_platform);
        
        let current_config = state.build_config;
        let current_platform = state.target_platform;
        
        ui::h_flex()
            .gap_1p5()
            .items_center()
            .child(
                Button::new("build_config_dropdown")
                    .label(config_label)
                    .icon(IconName::Settings)
                    .small()
                    .ghost()
                    .tooltip("Select build configuration")
                    .popup_menu(move |menu, _, _| {
                        menu
                            .label("Build Configuration")
                            .separator()
                            .menu_with_icon("Debug", IconName::Bug, Box::new(SetBuildConfig(BuildConfig::Debug)))
                            .menu_with_icon("Release", IconName::Flash, Box::new(SetBuildConfig(BuildConfig::Release)))
                            .menu_with_icon("Shipping", IconName::Package, Box::new(SetBuildConfig(BuildConfig::Shipping)))
                    })
            )
            .child(
                Button::new("platform_dropdown")
                    .label(platform_label)
                    .icon(platform_icon)
                    .small()
                    .ghost()
                    .tooltip("Select target platform and architecture")
                    .popup_menu(move |menu, window, cx| {
                        menu
                            .label("Target Platform")
                            .separator()
                            // Windows submenu
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Globe)), "Windows", window, cx, move |submenu, _, _| {
                                submenu
                                    .label("MSVC Toolchain")
                                    .menu_with_check("x86_64 (64-bit)", current_platform == TargetPlatform::WindowsX86_64Msvc, 
                                        Box::new(SetTargetPlatform(TargetPlatform::WindowsX86_64Msvc)))
                                    .menu_with_check("i686 (32-bit)", current_platform == TargetPlatform::WindowsI686Msvc, 
                                        Box::new(SetTargetPlatform(TargetPlatform::WindowsI686Msvc)))
                                    .menu_with_check("aarch64 (ARM64)", current_platform == TargetPlatform::WindowsAarch64Msvc, 
                                        Box::new(SetTargetPlatform(TargetPlatform::WindowsAarch64Msvc)))
                                    .separator()
                                    .label("GNU Toolchain")
                                    .menu_with_check("x86_64-gnu", current_platform == TargetPlatform::WindowsX86_64Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::WindowsX86_64Gnu)))
                                    .menu_with_check("i686-gnu", current_platform == TargetPlatform::WindowsI686Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::WindowsI686Gnu)))
                            })
                            // Linux submenu
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Terminal)), "Linux", window, cx, move |submenu, _, _| {
                                submenu
                                    .label("GNU libc")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::LinuxX86_64Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxX86_64Gnu)))
                                    .menu_with_check("i686", current_platform == TargetPlatform::LinuxI686Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxI686Gnu)))
                                    .menu_with_check("aarch64", current_platform == TargetPlatform::LinuxAarch64Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxAarch64Gnu)))
                                    .menu_with_check("armv7", current_platform == TargetPlatform::LinuxArmv7Gnueabihf, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxArmv7Gnueabihf)))
                                    .menu_with_check("arm", current_platform == TargetPlatform::LinuxArmGnueabi, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxArmGnueabi)))
                                    .menu_with_check("arm (hard float)", current_platform == TargetPlatform::LinuxArmGnueabihf, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxArmGnueabihf)))
                                    .menu_with_check("mips64", current_platform == TargetPlatform::LinuxMips64Gnuabi64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMips64Gnuabi64)))
                                    .menu_with_check("mips64el", current_platform == TargetPlatform::LinuxMips64elGnuabi64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMips64elGnuabi64)))
                                    .menu_with_check("mips", current_platform == TargetPlatform::LinuxMipsGnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMipsGnu)))
                                    .menu_with_check("mipsel", current_platform == TargetPlatform::LinuxMipselGnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMipselGnu)))
                                    .menu_with_check("powerpc64", current_platform == TargetPlatform::LinuxPowerpc64Gnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxPowerpc64Gnu)))
                                    .menu_with_check("powerpc64le", current_platform == TargetPlatform::LinuxPowerpc64leGnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxPowerpc64leGnu)))
                                    .menu_with_check("powerpc", current_platform == TargetPlatform::LinuxPowerpcGnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxPowerpcGnu)))
                                    .menu_with_check("riscv64", current_platform == TargetPlatform::LinuxRiscv64Gc, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxRiscv64Gc)))
                                    .menu_with_check("s390x", current_platform == TargetPlatform::LinuxS390xGnu, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxS390xGnu)))
                                    .menu_with_check("sparc64", current_platform == TargetPlatform::LinuxSparcv9, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxSparcv9)))
                                    .separator()
                                    .label("musl libc")
                                    .menu_with_check("x86_64-musl", current_platform == TargetPlatform::LinuxX86_64Musl, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxX86_64Musl)))
                                    .menu_with_check("aarch64-musl", current_platform == TargetPlatform::LinuxAarch64Musl, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxAarch64Musl)))
                                    .menu_with_check("armv7-musl", current_platform == TargetPlatform::LinuxArmv7Musleabihf, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxArmv7Musleabihf)))
                                    .menu_with_check("mipsel-musl", current_platform == TargetPlatform::LinuxMipselMusl, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMipselMusl)))
                                    .menu_with_check("mips-musl", current_platform == TargetPlatform::LinuxMipsMusl, 
                                        Box::new(SetTargetPlatform(TargetPlatform::LinuxMipsMusl)))
                            })
                            // macOS submenu
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Laptop)), "macOS", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("x86_64 (Intel)", current_platform == TargetPlatform::MacOsX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::MacOsX86_64)))
                                    .menu_with_check("aarch64 (Apple Silicon)", current_platform == TargetPlatform::MacOsAarch64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::MacOsAarch64)))
                            })
                            .separator()
                            // Mobile platforms
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Phone)), "iOS", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("aarch64 (Device)", current_platform == TargetPlatform::IosAarch64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::IosAarch64)))
                                    .menu_with_check("x86_64 (Simulator)", current_platform == TargetPlatform::IosX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::IosX86_64)))
                                    .menu_with_check("aarch64 (Simulator)", current_platform == TargetPlatform::IosAarch64Sim, 
                                        Box::new(SetTargetPlatform(TargetPlatform::IosAarch64Sim)))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Phone)), "Android", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("aarch64", current_platform == TargetPlatform::AndroidAarch64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::AndroidAarch64)))
                                    .menu_with_check("armv7", current_platform == TargetPlatform::AndroidArmv7, 
                                        Box::new(SetTargetPlatform(TargetPlatform::AndroidArmv7)))
                                    .menu_with_check("i686", current_platform == TargetPlatform::AndroidI686, 
                                        Box::new(SetTargetPlatform(TargetPlatform::AndroidI686)))
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::AndroidX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::AndroidX86_64)))
                            })
                            .separator()
                            // BSD variants
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Server)), "BSD", window, cx, move |submenu, _, _| {
                                submenu
                                    .label("FreeBSD")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::FreeBsdX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::FreeBsdX86_64)))
                                    .menu_with_check("i686", current_platform == TargetPlatform::FreeBsdI686, 
                                        Box::new(SetTargetPlatform(TargetPlatform::FreeBsdI686)))
                                    .separator()
                                    .label("NetBSD")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::NetBsdX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::NetBsdX86_64)))
                                    .separator()
                                    .label("OpenBSD")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::OpenBsdX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::OpenBsdX86_64)))
                                    .separator()
                                    .label("DragonFly BSD")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::DragonFlyX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::DragonFlyX86_64)))
                            })
                            // Other platforms
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Sun)), "Solaris", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("sparc64", current_platform == TargetPlatform::SolarisSparcv9, 
                                        Box::new(SetTargetPlatform(TargetPlatform::SolarisSparcv9)))
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::SolarisX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::SolarisX86_64)))
                                    .separator()
                                    .label("illumos")
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::IlumosX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::IlumosX86_64)))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Hexagon)), "Fuchsia", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("aarch64", current_platform == TargetPlatform::FuchsiaAarch64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::FuchsiaAarch64)))
                                    .menu_with_check("x86_64", current_platform == TargetPlatform::FuchsiaX86_64, 
                                        Box::new(SetTargetPlatform(TargetPlatform::FuchsiaX86_64)))
                            })
                            .menu_with_check("Redox x86_64", current_platform == TargetPlatform::RedoxX86_64, 
                                Box::new(SetTargetPlatform(TargetPlatform::RedoxX86_64)))
                            .separator()
                            // Gaming Consoles
                            .label("Gaming Consoles")
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Gamepad)), "PlayStation", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("PlayStation 4", current_platform == TargetPlatform::PlayStationPs4, 
                                        Box::new(SetTargetPlatform(TargetPlatform::PlayStationPs4)))
                                    .menu_with_check("PlayStation 5", current_platform == TargetPlatform::PlayStationPs5, 
                                        Box::new(SetTargetPlatform(TargetPlatform::PlayStationPs5)))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Gamepad)), "Xbox", window, cx, move |submenu, _, _| {
                                submenu
                                    .menu_with_check("Xbox One", current_platform == TargetPlatform::XboxOne, 
                                        Box::new(SetTargetPlatform(TargetPlatform::XboxOne)))
                                    .menu_with_check("Xbox Series X|S", current_platform == TargetPlatform::XboxSeriesXS, 
                                        Box::new(SetTargetPlatform(TargetPlatform::XboxSeriesXS)))
                            })
                            .menu_with_icon("Nintendo Switch", IconName::Gamepad, 
                                Box::new(SetTargetPlatform(TargetPlatform::NintendoSwitch)))
                    })
            )
    }
}
