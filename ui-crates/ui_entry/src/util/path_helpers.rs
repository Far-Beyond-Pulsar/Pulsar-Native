pub fn normalize_project_path(path: &str) -> String {
    let buf = std::path::PathBuf::from(path);
    if let (Some(file_name), Some(parent)) = (buf.file_name(), buf.parent()) {
        if let Some(parent_name) = parent.file_name() {
            if file_name == parent_name {
                return parent.to_string_lossy().to_string();
            }
        }
    }
    path.to_string()
}

pub fn appdata_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub fn recent_projects_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|d| d.data_dir().join("recent_projects.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("recent_projects.json"))
}

pub fn cloud_servers_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|d| d.data_dir().join("cloud_servers.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("cloud_servers.json"))
}

pub fn plugins_dir() -> std::path::PathBuf {
    appdata_dir().join("plugins")
}

pub fn registries_dir() -> std::path::PathBuf {
    appdata_dir().join("registries")
}

pub fn thumbnail_cache_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|d| d.cache_dir().join("template_thumbnails"))
        .unwrap_or_else(|| std::path::PathBuf::from("template_thumbnails"))
}
