use super::*;

impl BuildDropdowns {
    pub(super) fn get_platform_label(platform: TargetPlatform) -> &'static str {
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

    pub(super) fn get_platform_icon(platform: TargetPlatform) -> IconName {
        match platform {
            TargetPlatform::WindowsX86_64Msvc
            | TargetPlatform::WindowsI686Msvc
            | TargetPlatform::WindowsAarch64Msvc
            | TargetPlatform::WindowsX86_64Gnu
            | TargetPlatform::WindowsI686Gnu => IconName::Box,

            TargetPlatform::LinuxX86_64Gnu
            | TargetPlatform::LinuxI686Gnu
            | TargetPlatform::LinuxAarch64Gnu
            | TargetPlatform::LinuxArmv7Gnueabihf
            | TargetPlatform::LinuxArmGnueabi
            | TargetPlatform::LinuxArmGnueabihf
            | TargetPlatform::LinuxX86_64Musl
            | TargetPlatform::LinuxAarch64Musl
            | TargetPlatform::LinuxArmv7Musleabihf
            | TargetPlatform::LinuxMipselGnu
            | TargetPlatform::LinuxMipsGnu
            | TargetPlatform::LinuxMips64Gnuabi64
            | TargetPlatform::LinuxMips64elGnuabi64
            | TargetPlatform::LinuxPowerpc64Gnu
            | TargetPlatform::LinuxPowerpc64leGnu
            | TargetPlatform::LinuxPowerpcGnu
            | TargetPlatform::LinuxRiscv64Gc
            | TargetPlatform::LinuxS390xGnu
            | TargetPlatform::LinuxSparcv9
            | TargetPlatform::LinuxMipselMusl
            | TargetPlatform::LinuxMipsMusl => IconName::Server,

            TargetPlatform::MacOsX86_64 | TargetPlatform::MacOsAarch64 => IconName::Apple,

            TargetPlatform::AndroidAarch64
            | TargetPlatform::AndroidArmv7
            | TargetPlatform::AndroidI686
            | TargetPlatform::AndroidX86_64 => IconName::Phone,

            TargetPlatform::IosAarch64
            | TargetPlatform::IosX86_64
            | TargetPlatform::IosAarch64Sim => IconName::Apple,

            TargetPlatform::FreeBsdX86_64
            | TargetPlatform::FreeBsdI686
            | TargetPlatform::NetBsdX86_64
            | TargetPlatform::OpenBsdX86_64
            | TargetPlatform::DragonFlyX86_64 => IconName::Server,

            TargetPlatform::SolarisSparcv9
            | TargetPlatform::SolarisX86_64
            | TargetPlatform::IlumosX86_64 => IconName::Sun,

            TargetPlatform::RedoxX86_64 => IconName::Cpu,

            TargetPlatform::FuchsiaAarch64 | TargetPlatform::FuchsiaX86_64 => IconName::Hexagon,

            // Gaming Consoles
            TargetPlatform::PlayStationPs4 | TargetPlatform::PlayStationPs5 => IconName::Gamepad,
            TargetPlatform::XboxOne | TargetPlatform::XboxSeriesXS => IconName::Gamepad,
            TargetPlatform::NintendoSwitch => IconName::Gamepad,
        }
    }

}
