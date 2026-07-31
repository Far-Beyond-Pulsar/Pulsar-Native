//! Global Git hook configuration and safe project hook installation.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use engine_fs::{FsProvider, LocalFsProvider, virtual_fs};
use git2::{ErrorCode, Repository};
use serde::{Deserialize, Serialize};

/// Current on-disk schema version for [`GitHooksConfig`].
pub const GIT_HOOKS_CONFIG_VERSION: u32 = 1;

/// Application-wide Git hooks that should be installed into local projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHooksConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub hooks: BTreeMap<String, GitHookDefinition>,
}

impl Default for GitHooksConfig {
    fn default() -> Self {
        Self {
            version: GIT_HOOKS_CONFIG_VERSION,
            hooks: BTreeMap::new(),
        }
    }
}

impl GitHooksConfig {
    /// Validate the supported schema version and every enabled hook.
    ///
    /// Disabled hooks are retained as drafts and are not required to have
    /// executable content until they are enabled.
    pub fn validate(&self) -> Result<()> {
        if self.version != GIT_HOOKS_CONFIG_VERSION {
            bail!(
                "Unsupported Git hooks config version {}; expected {}",
                self.version,
                GIT_HOOKS_CONFIG_VERSION
            );
        }

        for (name, definition) in &self.hooks {
            validate_git_hook_definition(name, definition)?;
        }
        Ok(())
    }
}

const fn default_config_version() -> u32 {
    GIT_HOOKS_CONFIG_VERSION
}

/// One globally configured Git hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHookDefinition {
    pub enabled: bool,
    pub content: String,
}

/// Outcome of syncing a hook configuration into a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHookSyncReport {
    pub status: GitHookSyncStatus,
    /// Hook files created by this sync, sorted by name.
    pub created: Vec<String>,
    /// Existing hook files left untouched, sorted by name.
    pub preserved: Vec<String>,
}

/// Whether hook synchronization ran or was intentionally skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHookSyncStatus {
    Synced,
    Skipped(GitHookSyncSkipReason),
}

/// Non-error reasons why a project was not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHookSyncSkipReason {
    /// The active provider did not opt in to locally configured executable code.
    RemoteProvider {
        label: String,
    },
    MissingGitDirectory,
    /// Git is configured to use a hook directory managed outside this feature.
    CustomHooksPath {
        path: PathBuf,
    },
}

impl GitHookSyncReport {
    fn synced() -> Self {
        Self {
            status: GitHookSyncStatus::Synced,
            created: Vec::new(),
            preserved: Vec::new(),
        }
    }

    fn skipped(reason: GitHookSyncSkipReason) -> Self {
        Self {
            status: GitHookSyncStatus::Skipped(reason),
            created: Vec::new(),
            preserved: Vec::new(),
        }
    }
}

/// Return the canonical global Git hooks configuration path.
pub fn git_hooks_config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .context("Unable to determine the Pulsar application data directory")?;
    Ok(dirs.data_dir().join("configs").join("git_hooks.json"))
}

/// Load the canonical global Git hooks configuration.
///
/// A missing file returns [`GitHooksConfig::default`].
pub fn load_git_hooks_config() -> Result<GitHooksConfig> {
    load_git_hooks_config_from(&git_hooks_config_path()?)
}

/// Save the canonical global Git hooks configuration.
pub fn save_git_hooks_config(config: &GitHooksConfig) -> Result<()> {
    save_git_hooks_config_to(&git_hooks_config_path()?, config)
}

/// Load a Git hooks configuration from an explicit path.
///
/// A missing file returns [`GitHooksConfig::default`].
pub fn load_git_hooks_config_from(path: &Path) -> Result<GitHooksConfig> {
    let provider = LocalFsProvider::new();
    let bytes = match provider.read_file(path) {
        Ok(bytes) => bytes,
        Err(error) if has_io_kind(&error, ErrorKind::NotFound) => {
            return Ok(GitHooksConfig::default());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read Git hooks config '{}'", path.display()));
        }
    };

    let config: GitHooksConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse Git hooks config '{}'", path.display()))?;
    config
        .validate()
        .with_context(|| format!("Invalid Git hooks config '{}'", path.display()))?;
    Ok(config)
}

/// Save a Git hooks configuration to an explicit path.
///
/// The serialized data is written to a uniquely named sibling first and then
/// renamed over the destination, keeping replacement atomic where the platform
/// supports it.
pub fn save_git_hooks_config_to(path: &Path, config: &GitHooksConfig) -> Result<()> {
    config.validate().context("Invalid Git hooks config")?;
    let mut bytes =
        serde_json::to_vec_pretty(config).context("Failed to serialize Git hooks config")?;
    bytes.push(b'\n');
    LocalFsProvider::new()
        .write_file_atomically(path, &bytes)
        .with_context(|| format!("Failed to replace Git hooks config '{}'", path.display()))
}

/// Load the global hook configuration and synchronize it into a project.
pub fn sync_configured_project_hooks(project_root: &Path) -> Result<GitHookSyncReport> {
    let config = load_git_hooks_config()?;
    sync_project_git_hooks(project_root, &config)
}

/// Install all enabled global hooks that are missing from a local project's
/// repository-wide default hooks directory.
///
/// Existing hook files are always preserved. Cloud paths and providers that do
/// not explicitly permit local executable writes are skipped because
/// propagating executable content crosses a code-execution trust boundary.
pub fn sync_project_git_hooks(
    project_root: &Path,
    config: &GitHooksConfig,
) -> Result<GitHookSyncReport> {
    let active_provider = virtual_fs::provider();
    sync_project_git_hooks_with_provider(project_root, config, active_provider.as_ref())
}

fn sync_project_git_hooks_with_provider(
    project_root: &Path,
    config: &GitHooksConfig,
    active_provider: &dyn FsProvider,
) -> Result<GitHookSyncReport> {
    if virtual_fs::is_cloud_path(project_root) || !active_provider.permits_local_executable_writes()
    {
        return Ok(GitHookSyncReport::skipped(
            GitHookSyncSkipReason::RemoteProvider {
                label: active_provider.label().to_string(),
            },
        ));
    }

    let enabled_hooks = validate_enabled_hooks(config)?;
    let repository = match Repository::discover(project_root) {
        Ok(repository) => repository,
        Err(error) if error.code() == ErrorCode::NotFound => {
            return Ok(GitHookSyncReport::skipped(
                GitHookSyncSkipReason::MissingGitDirectory,
            ));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to discover a Git repository from '{}'",
                    project_root.display()
                )
            });
        }
    };

    let local_provider = LocalFsProvider::new();
    validate_repository_worktree(project_root, &repository, &local_provider)?;

    let repository_config = repository
        .config()
        .context("Failed to read the project Git configuration")?;
    match repository_config.get_path("core.hooksPath") {
        Ok(path) => {
            return Ok(GitHookSyncReport::skipped(
                GitHookSyncSkipReason::CustomHooksPath { path },
            ));
        }
        Err(error) if error.code() == ErrorCode::NotFound => {}
        Err(error) => return Err(error).context("Failed to read Git core.hooksPath"),
    }

    let common_dir = local_provider
        .canonicalize(repository.commondir())
        .context("Failed to resolve the repository common directory")?;
    let provider = LocalFsProvider::with_root(common_dir.clone())?;

    let mut report = GitHookSyncReport::synced();
    if enabled_hooks.is_empty() {
        return Ok(report);
    }

    let hooks_dir = common_dir.join("hooks");
    match provider.metadata(&hooks_dir) {
        Ok(metadata) if !metadata.is_dir => {
            bail!(
                "Git hooks path '{}' is not a directory",
                hooks_dir.display()
            )
        }
        Ok(_) => {}
        Err(error) if has_io_kind(&error, ErrorKind::NotFound) => {
            provider
                .create_dir_all(&hooks_dir)
                .context("Failed to create the project Git hooks directory")?;
        }
        Err(error) => return Err(error).context("Failed to inspect the Git hooks directory"),
    }

    for (name, definition) in enabled_hooks {
        let hook_path = hooks_dir.join(name);
        match provider.metadata(&hook_path) {
            Ok(metadata) if metadata.is_dir => {
                bail!("Git hook path '{}' is a directory", hook_path.display())
            }
            Ok(_) => {
                report.preserved.push(name.to_string());
                continue;
            }
            Err(error) if has_io_kind(&error, ErrorKind::NotFound) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect Git hook '{}'", name));
            }
        }

        match provider.create_executable_file(&hook_path, definition.content.as_bytes()) {
            Ok(()) => report.created.push(name.to_string()),
            Err(error) if has_io_kind(&error, ErrorKind::AlreadyExists) => {
                report.preserved.push(name.to_string());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to create Git hook '{}'", name));
            }
        }
    }

    Ok(report)
}

fn validate_repository_worktree(
    project_root: &Path,
    repository: &Repository,
    provider: &LocalFsProvider,
) -> Result<()> {
    let workdir = repository
        .workdir()
        .context("Git hook synchronization requires a non-bare repository")?;
    let canonical_project_root = provider.canonicalize(project_root).with_context(|| {
        format!(
            "Failed to resolve project path '{}'",
            project_root.display()
        )
    })?;
    let canonical_workdir = provider.canonicalize(workdir).with_context(|| {
        format!(
            "Failed to resolve repository worktree '{}'",
            workdir.display()
        )
    })?;
    if !canonical_project_root.starts_with(&canonical_workdir) {
        bail!(
            "Project path '{}' is outside repository worktree '{}'",
            canonical_project_root.display(),
            canonical_workdir.display()
        );
    }

    let git_marker = canonical_workdir.join(".git");
    if provider
        .is_symlink(&git_marker)
        .with_context(|| format!("Failed to inspect Git marker '{}'", git_marker.display()))?
    {
        bail!(
            "Git marker '{}' must not be a symbolic link or junction",
            git_marker.display()
        );
    }

    Ok(())
}

fn validate_enabled_hooks(config: &GitHooksConfig) -> Result<Vec<(&str, &GitHookDefinition)>> {
    config.validate()?;

    let mut enabled = Vec::new();
    for (name, definition) in &config.hooks {
        if !definition.enabled {
            continue;
        }
        enabled.push((name.as_str(), definition));
    }
    Ok(enabled)
}

/// Validate that a hook name is one plain filename on every supported platform.
pub fn validate_git_hook_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name.is_ascii()
        || name.bytes().any(|byte| byte.is_ascii_uppercase())
        || name == "."
        || name == ".."
        || name.contains('\0')
        || name.contains('/')
        || name.contains('\\')
        || name.ends_with('.')
        || name.ends_with(' ')
        || name
            .chars()
            .any(|character| character.is_control() || r#"<>:"|?*"#.contains(character))
        || is_windows_reserved_file_name(name)
    {
        bail!("Invalid Git hook name: {name:?}");
    }

    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if component == name => Ok(()),
        _ => bail!("Invalid Git hook name: {name:?}"),
    }
}

fn is_windows_reserved_file_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(|character| character == '.' || character == ' ');
    let stem = stem.to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }

    let suffix = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    matches!(
        suffix,
        Some(
            "1" | "2"
                | "3"
                | "4"
                | "5"
                | "6"
                | "7"
                | "8"
                | "9"
                | "\u{00B9}"
                | "\u{00B2}"
                | "\u{00B3}"
        )
    )
}

/// Validate one hook definition using the same rules as project synchronization.
///
/// Disabled definitions may contain draft or empty content, but their names are
/// still validated before they can be stored under a project hook path.
pub fn validate_git_hook_definition(name: &str, definition: &GitHookDefinition) -> Result<()> {
    validate_git_hook_name(name)?;
    if !definition.enabled {
        return Ok(());
    }

    let content = &definition.content;
    if content.trim().is_empty() {
        bail!("Enabled Git hook '{name}' has empty content");
    }
    if content.contains('\0') {
        bail!("Enabled Git hook '{name}' contains a NUL byte");
    }
    Ok(())
}

fn has_io_kind(error: &anyhow::Error, kind: ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io_error| io_error.kind() == kind)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{RepositoryInitOptions, Signature};
    use std::fs;

    fn hook(content: &str) -> GitHookDefinition {
        GitHookDefinition {
            enabled: true,
            content: content.to_string(),
        }
    }

    fn config_with(name: &str, content: &str) -> GitHooksConfig {
        let mut config = GitHooksConfig::default();
        config.hooks.insert(name.to_string(), hook(content));
        config
    }

    fn initialize_repository(root: &Path) -> Repository {
        let mut options = RepositoryInitOptions::new();
        options.external_template(false).initial_head("main");
        Repository::init_opts(root, &options).unwrap()
    }

    fn create_initial_commit(repository: &Repository) {
        let tree_id = repository.index().unwrap().write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = Signature::now("Pulsar Test", "pulsar@example.com").unwrap();
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Initial commit",
                &tree,
                &[],
            )
            .unwrap();
    }

    struct NonLocalProvider;

    impl FsProvider for NonLocalProvider {
        fn read_file(&self, _path: &Path) -> Result<Vec<u8>> {
            unreachable!()
        }

        fn write_file(&self, _path: &Path, _content: &[u8]) -> Result<()> {
            unreachable!()
        }

        fn create_file(&self, _path: &Path, _content: &[u8]) -> Result<()> {
            unreachable!()
        }

        fn delete_path(&self, _path: &Path) -> Result<()> {
            unreachable!()
        }

        fn rename(&self, _from: &Path, _to: &Path) -> Result<()> {
            unreachable!()
        }

        fn list_dir(&self, _path: &Path) -> Result<Vec<engine_fs::FsEntry>> {
            unreachable!()
        }

        fn create_dir_all(&self, _path: &Path) -> Result<()> {
            unreachable!()
        }

        fn exists(&self, _path: &Path) -> Result<bool> {
            unreachable!()
        }

        fn metadata(&self, _path: &Path) -> Result<engine_fs::FsMetadata> {
            unreachable!()
        }

        fn is_remote(&self) -> bool {
            true
        }

        fn label(&self) -> &str {
            "Remote"
        }
    }

    #[test]
    fn missing_config_loads_default() {
        let temp = tempfile::tempdir().unwrap();
        let loaded = load_git_hooks_config_from(&temp.path().join("missing.json")).unwrap();
        assert_eq!(loaded, GitHooksConfig::default());
        assert_eq!(loaded.version, GIT_HOOKS_CONFIG_VERSION);
    }

    #[test]
    fn config_round_trips_and_replaces_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("git_hooks.json");
        let mut config = config_with("pre-commit", "#!/bin/sh\nexit 0\n");

        save_git_hooks_config_to(&path, &config).unwrap();
        assert_eq!(load_git_hooks_config_from(&path).unwrap(), config);

        config
            .hooks
            .insert("pre-push".to_string(), hook("#!/bin/sh\nexit 1\n"));
        save_git_hooks_config_to(&path, &config).unwrap();
        assert_eq!(load_git_hooks_config_from(&path).unwrap(), config);

        let files = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(files, 1, "temporary config files should be cleaned up");
    }

    #[test]
    fn corrupt_config_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("git_hooks.json");
        fs::write(&path, b"{not-json").unwrap();

        let error = load_git_hooks_config_from(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to parse Git hooks config")
        );
    }

    #[test]
    fn invalid_config_is_rejected_on_load_and_save() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("git_hooks.json");
        fs::write(&path, br#"{"version":2,"hooks":{}}"#).unwrap();
        assert!(load_git_hooks_config_from(&path).is_err());

        let mut invalid = GitHooksConfig::default();
        invalid.hooks.insert(
            "../escape".to_string(),
            GitHookDefinition {
                enabled: false,
                content: String::new(),
            },
        );
        assert!(save_git_hooks_config_to(&path, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"version":2,"hooks":{}}"#);
    }

    #[test]
    fn missing_git_directory_is_skipped_without_creating_it() {
        let temp = tempfile::tempdir().unwrap();
        let report = sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        assert_eq!(
            report.status,
            GitHookSyncStatus::Skipped(GitHookSyncSkipReason::MissingGitDirectory)
        );
        assert!(!temp.path().join(".git").exists());
    }

    #[test]
    fn existing_local_repository_honors_provider_denial() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());

        let report = sync_project_git_hooks_with_provider(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
            &NonLocalProvider,
        )
        .unwrap();

        assert_eq!(
            report.status,
            GitHookSyncStatus::Skipped(GitHookSyncSkipReason::RemoteProvider {
                label: "Remote".to_string(),
            })
        );
        assert!(report.created.is_empty());
        assert!(!temp.path().join(".git/hooks/pre-commit").exists());
    }

    #[test]
    fn creates_hooks_directory_and_missing_hooks() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let config = config_with("pre-commit", "#!/bin/sh\nexit 0\n");

        let report = sync_project_git_hooks(temp.path(), &config).unwrap();

        assert_eq!(report.status, GitHookSyncStatus::Synced);
        assert_eq!(report.created, vec!["pre-commit"]);
        assert!(report.preserved.is_empty());
        assert_eq!(
            fs::read(temp.path().join(".git/hooks/pre-commit")).unwrap(),
            b"#!/bin/sh\nexit 0\n"
        );
    }

    #[test]
    fn linked_worktree_uses_repository_common_hooks_directory() {
        let temp = tempfile::tempdir().unwrap();
        let main_path = temp.path().join("main");
        let linked_path = temp.path().join("linked");
        let main_repository = initialize_repository(&main_path);
        create_initial_commit(&main_repository);
        main_repository
            .worktree("linked", &linked_path, None)
            .unwrap();

        let linked_repository = Repository::discover(&linked_path).unwrap();
        assert!(linked_repository.is_worktree());
        assert_eq!(
            linked_repository.commondir().canonicalize().unwrap(),
            main_repository.commondir().canonicalize().unwrap()
        );

        let report = sync_project_git_hooks(
            &linked_path,
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        assert_eq!(report.status, GitHookSyncStatus::Synced);
        assert!(
            main_repository
                .commondir()
                .join("hooks/pre-commit")
                .is_file()
        );
        assert!(!linked_repository.path().join("hooks/pre-commit").exists());
    }

    #[test]
    fn submodule_uses_its_repository_common_hooks_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source");
        let parent_path = temp.path().join("parent");
        let source_repository = initialize_repository(&source_path);
        create_initial_commit(&source_repository);
        let parent_repository = initialize_repository(&parent_path);
        create_initial_commit(&parent_repository);

        let mut submodule = parent_repository
            .submodule(source_path.to_str().unwrap(), Path::new("child"), true)
            .unwrap();
        submodule.clone(None).unwrap();
        submodule.add_to_index(true).unwrap();
        submodule.add_finalize().unwrap();

        let child_path = parent_path.join("child");
        assert!(child_path.join(".git").is_file());
        let child_repository = Repository::discover(&child_path).unwrap();
        let expected_common_dir = parent_repository.commondir().join("modules/child");
        assert_eq!(
            child_repository.commondir().canonicalize().unwrap(),
            expected_common_dir.canonicalize().unwrap()
        );

        let report =
            sync_project_git_hooks(&child_path, &config_with("pre-push", "#!/bin/sh\nexit 0\n"))
                .unwrap();

        assert_eq!(report.status, GitHookSyncStatus::Synced);
        assert!(expected_common_dir.join("hooks/pre-push").is_file());
        assert!(!source_path.join(".git/hooks/pre-push").exists());
    }

    #[test]
    fn custom_hooks_path_is_skipped_without_writing_either_directory() {
        let temp = tempfile::tempdir().unwrap();
        let repository = initialize_repository(temp.path());
        let custom_hooks_path = temp.path().join("custom-hooks");
        repository
            .config()
            .unwrap()
            .set_str("core.hooksPath", custom_hooks_path.to_str().unwrap())
            .unwrap();

        let report = sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        assert_eq!(
            report.status,
            GitHookSyncStatus::Skipped(GitHookSyncSkipReason::CustomHooksPath {
                path: custom_hooks_path.clone(),
            })
        );
        assert!(!custom_hooks_path.join("pre-commit").exists());
        assert!(!repository.commondir().join("hooks/pre-commit").exists());
    }

    #[test]
    fn empty_custom_hooks_path_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let repository = initialize_repository(temp.path());
        repository
            .config()
            .unwrap()
            .set_str("core.hooksPath", "")
            .unwrap();

        let report = sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        assert_eq!(
            report.status,
            GitHookSyncStatus::Skipped(GitHookSyncSkipReason::CustomHooksPath {
                path: PathBuf::new(),
            })
        );
        assert!(!repository.commondir().join("hooks/pre-commit").exists());
    }

    #[test]
    fn cloud_project_remains_skipped_for_nonlocal_provider() {
        let report = sync_project_git_hooks_with_provider(
            Path::new("cloud+pulsar://host/workspace/environment"),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
            &NonLocalProvider,
        )
        .unwrap();

        assert_eq!(
            report.status,
            GitHookSyncStatus::Skipped(GitHookSyncSkipReason::RemoteProvider {
                label: "Remote".to_string(),
            })
        );
    }

    #[test]
    fn preserves_existing_hook_content() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let hooks_dir = temp.path().join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        fs::write(&hook_path, b"existing hook\n").unwrap();

        let report = sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "replacement hook\n"),
        )
        .unwrap();

        assert!(report.created.is_empty());
        assert_eq!(report.preserved, vec!["pre-commit"]);
        assert_eq!(fs::read(hook_path).unwrap(), b"existing hook\n");
    }

    #[test]
    fn repeated_sync_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let config = config_with("pre-push", "#!/bin/sh\nexit 0\n");

        let first = sync_project_git_hooks(temp.path(), &config).unwrap();
        let second = sync_project_git_hooks(temp.path(), &config).unwrap();

        assert_eq!(first.created, vec!["pre-push"]);
        assert!(first.preserved.is_empty());
        assert!(second.created.is_empty());
        assert_eq!(second.preserved, vec!["pre-push"]);
    }

    #[test]
    fn invalid_hook_is_rejected_before_any_write() {
        let invalid_names = [
            "",
            ".",
            "..",
            "../escape",
            "nested/hook",
            "nested\\hook",
            "pre-commit:stream",
            "NUL",
            "con.txt",
            "COM1",
            "lpt9.txt",
            "COM\u{00B9}.log",
            "hook.",
            "hook ",
            "hook\nname",
            "bad?hook",
            "PRE-COMMIT",
            "pre-comm\u{00ED}t",
        ];

        for invalid_name in invalid_names {
            let temp = tempfile::tempdir().unwrap();
            initialize_repository(temp.path());
            let mut config = config_with("a-valid", "valid content\n");
            config
                .hooks
                .insert(invalid_name.to_string(), hook("invalid content\n"));

            assert!(sync_project_git_hooks(temp.path(), &config).is_err());
            assert!(
                !temp.path().join(".git/hooks/a-valid").exists(),
                "invalid name {invalid_name:?} must be rejected before creating any hook"
            );
        }

        for invalid_content in ["", "   \n", "echo before\0echo after"] {
            let temp = tempfile::tempdir().unwrap();
            initialize_repository(temp.path());
            let mut config = config_with("a-valid", "valid content\n");
            config
                .hooks
                .insert("z-invalid".to_string(), hook(invalid_content));

            assert!(sync_project_git_hooks(temp.path(), &config).is_err());
            assert!(!temp.path().join(".git/hooks/a-valid").exists());
            assert!(!temp.path().join(".git/hooks/z-invalid").exists());
        }
    }

    #[test]
    fn unsupported_config_version_is_rejected_before_any_write() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let mut config = config_with("pre-commit", "valid content\n");
        config.version = GIT_HOOKS_CONFIG_VERSION + 1;

        assert!(sync_project_git_hooks(temp.path(), &config).is_err());
        assert!(!temp.path().join(".git/hooks/pre-commit").exists());
    }

    #[test]
    fn disabled_invalid_hooks_are_not_installed() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let mut config = GitHooksConfig::default();
        config.hooks.insert(
            "disabled-draft".to_string(),
            GitHookDefinition {
                enabled: false,
                content: String::new(),
            },
        );

        let report = sync_project_git_hooks(temp.path(), &config).unwrap();
        assert_eq!(report.status, GitHookSyncStatus::Synced);
        assert!(!temp.path().join(".git/hooks/disabled-draft").exists());
    }

    #[test]
    fn executable_creation_leaves_no_staging_files() {
        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        let hooks_dir = temp.path().join(".git/hooks");

        sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        let entries = fs::read_dir(hooks_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(entries.iter().any(|entry| entry == "pre-commit"));
        assert!(
            !entries
                .iter()
                .any(|entry| { entry.to_string_lossy().starts_with(".pulsar-executable.") })
        );
    }

    #[cfg(unix)]
    #[test]
    fn newly_installed_hook_is_executable() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        initialize_repository(temp.path());
        sync_project_git_hooks(
            temp.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        )
        .unwrap();

        let mode = fs::metadata(temp.path().join(".git/hooks/pre-commit"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn git_symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        initialize_repository(outside.path());
        symlink(outside.path().join(".git"), project.path().join(".git")).unwrap();

        let result = sync_project_git_hooks(
            project.path(),
            &config_with("pre-commit", "#!/bin/sh\nexit 0\n"),
        );

        assert!(result.is_err());
        assert!(!outside.path().join(".git/hooks/pre-commit").exists());
    }
}
