//! Display text for the build model. `engine_state` has no translations, so
//! its profiles, platforms and steps are named here.

use engine_state::build_config::{
    BuildConfiguration, BuildProfile, BuildSteps, PlatformFamily, TargetPlatform,
};
use rust_i18n::t;

pub fn profile_label(profile: BuildProfile) -> String {
    match profile {
        BuildProfile::Debug => t!("Build.Profile.Debug.Label"),
        BuildProfile::Release => t!("Build.Profile.Release.Label"),
        BuildProfile::Shipping => t!("Build.Profile.Shipping.Label"),
    }
    .to_string()
}

pub fn profile_summary(profile: BuildProfile) -> String {
    match profile {
        BuildProfile::Debug => t!("Build.Profile.Debug.Summary"),
        BuildProfile::Release => t!("Build.Profile.Release.Summary"),
        BuildProfile::Shipping => t!("Build.Profile.Shipping.Summary"),
    }
    .to_string()
}

pub fn family_label(family: PlatformFamily) -> String {
    match family {
        PlatformFamily::Windows => t!("Build.Family.Windows"),
        PlatformFamily::Linux => t!("Build.Family.Linux"),
        PlatformFamily::MacOs => t!("Build.Family.MacOs"),
        PlatformFamily::Ios => t!("Build.Family.Ios"),
        PlatformFamily::Android => t!("Build.Family.Android"),
        PlatformFamily::Bsd => t!("Build.Family.Bsd"),
        PlatformFamily::Unix => t!("Build.Family.Unix"),
        PlatformFamily::Other => t!("Build.Family.Other"),
        PlatformFamily::Console => t!("Build.Family.Console"),
    }
    .to_string()
}

/// Keyed by the platform's saved id: `Build.Target.WindowsX86_64Msvc`.
pub fn platform_label(platform: TargetPlatform) -> String {
    t!(format!("Build.Target.{}", platform.id())).to_string()
}

/// `Update › Clean › Build › Run`, or `No steps` when empty.
pub fn steps_summary(steps: BuildSteps) -> String {
    let parts: Vec<String> = [
        (steps.update, t!("Build.Steps.Summary.Update")),
        (steps.clean, t!("Build.Steps.Summary.Clean")),
        (steps.check, t!("Build.Steps.Summary.Check")),
        (steps.build, t!("Build.Steps.Summary.Build")),
        (steps.run, t!("Build.Steps.Summary.Run")),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, part)| part.to_string())
    .collect();
    if parts.is_empty() {
        t!("Build.Steps.Summary.None").to_string()
    } else {
        parts.join(" › ")
    }
}

/// `Windows x64`, `This machine`, or `Windows x64 +2`.
pub fn platforms_summary(config: &BuildConfiguration) -> String {
    match config.platforms.as_slice() {
        [] => t!("Build.Platforms.ThisMachine").to_string(),
        [one] => platform_label(*one),
        [first, rest @ ..] => format!("{} +{}", platform_label(*first), rest.len()),
    }
}

/// One line describing a configuration: `Release · Windows x64 · Build › Run`.
pub fn subtitle(config: &BuildConfiguration) -> String {
    format!(
        "{} · {} · {}",
        profile_label(config.profile),
        platforms_summary(config),
        steps_summary(config.steps)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_has_a_label() {
        for &platform in TargetPlatform::ALL {
            let key = format!("Build.Target.{}", platform.id());
            assert_ne!(
                platform_label(platform),
                key,
                "missing {key} in locales/en.yml"
            );
        }
    }

    #[test]
    fn subtitles_read_naturally() {
        let mut c = BuildConfiguration::new("x");
        c.steps = BuildSteps {
            build: true,
            run: true,
            ..Default::default()
        };
        assert_eq!(subtitle(&c), "Release · This machine · Build › Run");
        c.platforms = vec![
            TargetPlatform::WindowsX86_64Msvc,
            TargetPlatform::LinuxX86_64Gnu,
        ];
        assert_eq!(platforms_summary(&c), "Windows x64 +1");
        c.steps = BuildSteps::default();
        assert_eq!(steps_summary(c.steps), "No steps");
    }
}
