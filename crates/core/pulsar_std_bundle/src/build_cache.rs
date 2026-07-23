use std::ffi::OsString;
use std::path::{Path, PathBuf};

const CACHE_DIR: &str = "pulsar-std-native";

pub(crate) fn native_target_dir(
    workspace_root: &Path,
    explicit_native_target: Option<OsString>,
    cargo_target_dir: Option<OsString>,
) -> PathBuf {
    if let Some(path) = explicit_native_target {
        return absolute_from(workspace_root, PathBuf::from(path));
    }

    let outer_target = cargo_target_dir
        .map(PathBuf::from)
        .map(|path| absolute_from(workspace_root, path))
        .unwrap_or_else(|| workspace_root.join("target"));
    outer_target.join(CACHE_DIR)
}

pub(crate) fn native_artifact_path(
    target_dir: &Path,
    cross_target: Option<&str>,
    profile: &str,
    prefix: &str,
    extension: &str,
) -> PathBuf {
    let mut path = target_dir.to_path_buf();
    if let Some(target) = cross_target {
        path.push(target);
    }
    path.join(profile)
        .join(format!("{prefix}pulsar_std.{extension}"))
}

fn absolute_from(workspace_root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cache_is_stable_below_the_workspace_target() {
        let root = Path::new("workspace");
        assert_eq!(
            native_target_dir(root, None, None),
            root.join("target").join(CACHE_DIR)
        );
    }

    #[test]
    fn configured_outer_target_gets_an_isolated_stable_child() {
        let root = Path::new("workspace");
        assert_eq!(
            native_target_dir(root, None, Some(OsString::from("shared-target"))),
            root.join("shared-target").join(CACHE_DIR)
        );
    }

    #[test]
    fn explicit_native_target_is_used_without_an_extra_suffix() {
        let root = Path::new("workspace");
        assert_eq!(
            native_target_dir(
                root,
                Some(OsString::from("native-cache")),
                Some(OsString::from("ignored")),
            ),
            root.join("native-cache")
        );
    }

    #[test]
    fn cross_compiled_artifact_includes_the_target_triple() {
        assert_eq!(
            native_artifact_path(
                Path::new("cache"),
                Some("aarch64-apple-darwin"),
                "release",
                "lib",
                "dylib",
            ),
            Path::new("cache")
                .join("aarch64-apple-darwin")
                .join("release")
                .join("libpulsar_std.dylib")
        );
    }
}
