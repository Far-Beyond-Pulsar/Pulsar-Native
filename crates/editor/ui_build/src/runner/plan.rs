//! Turning a [`BuildConfiguration`] into the ordered cargo commands it means.
//!
//! This is pure: no processes, no UI. The runner executes a [`Plan`]; the
//! configurator shows one as a live preview, so what you see is what runs.

use engine_state::build_config::{BuildConfiguration, TargetPlatform};

/// One cargo command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// `build`, `check`, `run`, `update`, `clean`.
    pub subcommand: &'static str,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

impl Invocation {
    fn new(subcommand: &'static str) -> Self {
        Self { subcommand, args: Vec::new(), envs: Vec::new() }
    }

    /// The command line, for display.
    pub fn display(&self) -> String {
        let mut line = format!("cargo {}", self.subcommand);
        for arg in &self.args {
            line.push(' ');
            if arg.contains(char::is_whitespace) {
                line.push_str(&format!("{arg:?}"));
            } else {
                line.push_str(arg);
            }
        }
        line
    }
}

/// One step of a build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make sure the project's generated scaffolding exists and is current.
    Bootstrap,
    Update(Invocation),
    Clean(Invocation),
    Check { platform: Option<TargetPlatform>, invocation: Invocation },
    Build { platform: Option<TargetPlatform>, invocation: Invocation },
    Run { platform: Option<TargetPlatform>, invocation: Invocation },
}

impl Step {
    /// What it is doing, for status text.
    pub fn title(&self) -> String {
        let on = |platform: &Option<TargetPlatform>| {
            platform.map(|p| format!(" for {}", p.label())).unwrap_or_default()
        };
        match self {
            Self::Bootstrap => "Preparing project".into(),
            Self::Update(_) => "Updating dependencies".into(),
            Self::Clean(_) => "Cleaning build artifacts".into(),
            Self::Check { platform, .. } => format!("Checking{}", on(platform)),
            Self::Build { platform, .. } => format!("Building{}", on(platform)),
            Self::Run { platform, .. } => format!("Launching{}", on(platform)),
        }
    }

    pub fn invocation(&self) -> Option<&Invocation> {
        match self {
            Self::Bootstrap => None,
            Self::Update(i) | Self::Clean(i) => Some(i),
            Self::Check { invocation, .. }
            | Self::Build { invocation, .. }
            | Self::Run { invocation, .. } => Some(invocation),
        }
    }

    /// Relative share of the progress bar. The game launch is not part of it:
    /// the bar is full when the build ends.
    fn weight(&self) -> u32 {
        match self {
            Self::Bootstrap => 2,
            Self::Update(_) => 12,
            Self::Clean(_) => 8,
            Self::Check { .. } => 20,
            Self::Build { .. } => 60,
            Self::Run { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Things the user should know that do not stop the build.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Every step is switched off.
    NoSteps,
    /// A selected platform cannot be built with cargo here.
    UnbuildablePlatform(TargetPlatform),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSteps => write!(f, "This configuration has no steps enabled."),
            Self::UnbuildablePlatform(p) => write!(
                f,
                "{} cannot be built with cargo; it needs its platform SDK.",
                p.label()
            ),
        }
    }
}

/// Split a command-line fragment into arguments, honouring single and double
/// quotes. An unterminated quote runs to the end.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in text.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => current.push(ch),
        }
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    args
}

/// Feature names from a comma / space separated list, without repeats.
pub fn parse_features(text: &str) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();
    for feature in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let feature = feature.trim();
        if !feature.is_empty() && !features.iter().any(|f| f == feature) {
            features.push(feature.to_owned());
        }
    }
    features
}

/// The arguments shared by check / build / run for one platform.
fn common_args(config: &BuildConfiguration, platform: Option<TargetPlatform>) -> Vec<String> {
    let mut args: Vec<String> = config.profile.cargo_args().iter().map(|s| s.to_string()).collect();
    if let Some(triple) = platform.and_then(TargetPlatform::triple) {
        args.push("--target".into());
        args.push(triple.into());
    }
    let features = parse_features(&config.features);
    if !features.is_empty() {
        args.push("--features".into());
        args.push(features.join(","));
    }
    args.extend(split_args(&config.extra_args));
    args
}

fn cargo(
    subcommand: &'static str,
    config: &BuildConfiguration,
    platform: Option<TargetPlatform>,
) -> Invocation {
    let mut invocation = Invocation::new(subcommand);
    invocation.args = common_args(config, platform);
    invocation.envs = config
        .profile
        .cargo_env()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    invocation
}

/// Expand `config` into steps. `host` is the platform this machine runs, used
/// to decide which build can be launched.
///
/// Order: bootstrap, update, clean (once each), then for every platform check
/// and/or build, then run once for the first platform this machine can run.
pub fn plan(config: &BuildConfiguration, host: Option<TargetPlatform>) -> Result<Plan, PlanError> {
    let steps = config.steps.normalized();
    if !steps.any() {
        return Err(PlanError::NoSteps);
    }

    let needs_cargo_target = steps.check || steps.build;
    if needs_cargo_target {
        if let Some(&bad) = config.platforms.iter().find(|p| !p.is_buildable()) {
            return Err(PlanError::UnbuildablePlatform(bad));
        }
    }

    // No platforms selected means "this machine": cargo's default target.
    let targets: Vec<Option<TargetPlatform>> = if config.platforms.is_empty() {
        vec![None]
    } else {
        config.platforms.iter().copied().map(Some).collect()
    };

    let mut plan = Plan { steps: vec![Step::Bootstrap], warnings: Vec::new() };

    if steps.update {
        plan.steps.push(Step::Update(Invocation::new("update")));
    }
    if steps.clean {
        plan.steps.push(Step::Clean(Invocation::new("clean")));
    }
    for &platform in &targets {
        if steps.check {
            plan.steps.push(Step::Check {
                platform,
                invocation: cargo("check", config, platform),
            });
        }
        if steps.build {
            plan.steps.push(Step::Build {
                platform,
                invocation: cargo("build", config, platform),
            });
        }
    }
    if steps.run {
        let runnable = targets
            .iter()
            .copied()
            .find(|p| p.is_none() || p.is_some() && *p == host);
        match runnable {
            Some(platform) => plan.steps.push(Step::Run {
                platform,
                invocation: cargo("run", config, platform),
            }),
            None => plan.warnings.push(
                "Run was skipped: none of the selected platforms can run on this machine.".into(),
            ),
        }
    }
    Ok(plan)
}

/// The progress-bar range `(from, to)` of each step, in percent, in step order.
/// Ranges are contiguous and end at 100; a step with no weight gets an empty
/// range at the end.
pub fn progress_ranges(plan: &Plan) -> Vec<(u32, u32)> {
    let total: u32 = plan.steps.iter().map(Step::weight).sum();
    let mut done = 0;
    let mut ranges = Vec::with_capacity(plan.steps.len());
    for step in &plan.steps {
        let from = if total == 0 { 100 } else { done * 100 / total };
        done += step.weight();
        let to = if total == 0 { 100 } else { done * 100 / total };
        ranges.push((from, to));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::build_config::{BuildProfile, BuildSteps};

    fn config(steps: BuildSteps) -> BuildConfiguration {
        let mut c = BuildConfiguration::new("t");
        c.steps = steps;
        c
    }

    fn kinds(plan: &Plan) -> Vec<&'static str> {
        plan.steps
            .iter()
            .map(|s| match s {
                Step::Bootstrap => "bootstrap",
                Step::Update(_) => "update",
                Step::Clean(_) => "clean",
                Step::Check { .. } => "check",
                Step::Build { .. } => "build",
                Step::Run { .. } => "run",
            })
            .collect()
    }

    const WIN: Option<TargetPlatform> = Some(TargetPlatform::WindowsX86_64Msvc);

    #[test]
    fn steps_run_in_a_fixed_order() {
        let c = config(BuildSteps { update: true, clean: true, check: true, build: true, run: true });
        let p = plan(&c, WIN).unwrap();
        assert_eq!(kinds(&p), ["bootstrap", "update", "clean", "check", "build", "run"]);
    }

    #[test]
    fn nothing_enabled_is_an_error() {
        assert_eq!(plan(&config(BuildSteps::default()), WIN), Err(PlanError::NoSteps));
    }

    #[test]
    fn run_alone_still_builds() {
        let p = plan(&config(BuildSteps { run: true, ..Default::default() }), WIN).unwrap();
        assert_eq!(kinds(&p), ["bootstrap", "build", "run"]);
    }

    #[test]
    fn profile_target_features_and_extras_reach_cargo() {
        let mut c = config(BuildSteps::just_build());
        c.profile = BuildProfile::Release;
        c.platforms = vec![TargetPlatform::LinuxX86_64Gnu];
        c.features = "a, b  a".into();
        c.extra_args = r#"--locked --config "build.jobs=4""#.into();
        let p = plan(&c, WIN).unwrap();
        let Step::Build { invocation, .. } = &p.steps[1] else { panic!("not a build") };
        assert_eq!(
            invocation.display(),
            r#"cargo build --release --target x86_64-unknown-linux-gnu --features a,b --locked --config build.jobs=4"#
        );
    }

    #[test]
    fn debug_has_no_profile_flag_and_shipping_sets_the_release_overrides() {
        let mut c = config(BuildSteps::just_build());
        c.profile = BuildProfile::Debug;
        let Step::Build { invocation, .. } = &plan(&c, WIN).unwrap().steps[1] else { panic!() };
        assert_eq!(invocation.display(), "cargo build");
        assert!(invocation.envs.is_empty());

        c.profile = BuildProfile::Shipping;
        let Step::Build { invocation, .. } = &plan(&c, WIN).unwrap().steps[1] else { panic!() };
        assert!(invocation.args.contains(&"--release".to_string()));
        assert!(invocation.envs.iter().any(|(k, v)| k == "CARGO_PROFILE_RELEASE_LTO" && v == "fat"));
    }

    #[test]
    fn each_platform_is_built_in_turn_and_clean_happens_once() {
        let mut c = config(BuildSteps { clean: true, check: true, build: true, ..Default::default() });
        c.platforms = vec![TargetPlatform::WindowsX86_64Msvc, TargetPlatform::LinuxX86_64Gnu];
        let p = plan(&c, WIN).unwrap();
        assert_eq!(kinds(&p), ["bootstrap", "clean", "check", "build", "check", "build"]);
    }

    #[test]
    fn run_launches_the_first_platform_this_machine_can_run() {
        let mut c = config(BuildSteps { build: true, run: true, ..Default::default() });
        c.platforms = vec![TargetPlatform::LinuxX86_64Gnu, TargetPlatform::WindowsX86_64Msvc];
        let p = plan(&c, WIN).unwrap();
        let Some(Step::Run { platform, .. }) = p.steps.last() else { panic!("no run step") };
        assert_eq!(*platform, WIN);
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn run_is_skipped_with_a_warning_when_nothing_runs_here() {
        let mut c = config(BuildSteps { build: true, run: true, ..Default::default() });
        c.platforms = vec![TargetPlatform::LinuxX86_64Gnu];
        let p = plan(&c, WIN).unwrap();
        assert_eq!(kinds(&p), ["bootstrap", "build"]);
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn the_machine_default_always_runs() {
        let c = config(BuildSteps { build: true, run: true, ..Default::default() });
        // Even on a host we do not recognise.
        let p = plan(&c, None).unwrap();
        assert!(matches!(p.steps.last(), Some(Step::Run { platform: None, .. })));
    }

    #[test]
    fn a_console_cannot_be_built_but_update_and_clean_do_not_care() {
        let mut c = config(BuildSteps::just_build());
        c.platforms = vec![TargetPlatform::PlayStationPs5];
        assert_eq!(plan(&c, WIN), Err(PlanError::UnbuildablePlatform(TargetPlatform::PlayStationPs5)));

        let c2 = {
            let mut c2 = config(BuildSteps { update: true, ..Default::default() });
            c2.platforms = vec![TargetPlatform::PlayStationPs5];
            c2
        };
        assert!(plan(&c2, WIN).is_ok());
    }

    #[test]
    fn quoted_arguments_stay_together() {
        assert_eq!(split_args(r#"a "b c" 'd e' f"#), ["a", "b c", "d e", "f"]);
        assert_eq!(split_args("  "), Vec::<String>::new());
        assert_eq!(split_args(r#"x "unterminated y"#), ["x", "unterminated y"]);
        assert_eq!(split_args(r#"empty "" end"#), ["empty", "", "end"]);
    }

    #[test]
    fn features_split_on_commas_and_spaces_without_repeats() {
        assert_eq!(parse_features("a,b c ,, a"), ["a", "b", "c"]);
        assert!(parse_features("  ").is_empty());
    }

    #[test]
    fn progress_ranges_are_contiguous_and_end_at_100() {
        let c = config(BuildSteps { update: true, clean: true, check: true, build: true, run: true });
        let p = plan(&c, WIN).unwrap();
        let ranges = progress_ranges(&p);
        assert_eq!(ranges.len(), p.steps.len());
        assert_eq!(ranges.first().unwrap().0, 0);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "gap between steps");
        }
        // The launch has no share: it starts and ends at 100.
        assert_eq!(*ranges.last().unwrap(), (100, 100));
        assert_eq!(ranges[ranges.len() - 2].1, 100);
    }
}
