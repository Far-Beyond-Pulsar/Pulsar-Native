pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

fn parse_iso_timestamp(s: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_local());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Some(dt);
    }
    None
}

pub fn format_timestamp(timestamp: &str) -> String {
    if timestamp.is_empty() {
        return "Never".to_string();
    }
    if let Some(parsed) = parse_iso_timestamp(timestamp) {
        let now = chrono::Local::now().naive_local();
        let duration = now - parsed;
        if duration.num_minutes() < 1 {
            "Just now".to_string()
        } else if duration.num_hours() < 1 {
            format!("{}m ago", duration.num_minutes())
        } else if duration.num_days() < 1 {
            format!("{}h ago", duration.num_hours())
        } else if duration.num_days() < 7 {
            format!("{}d ago", duration.num_days())
        } else {
            parsed.format("%b %d").to_string()
        }
    } else {
        timestamp.to_string()
    }
}

pub fn sanitize_repo_name(repo_url: &str) -> String {
    let trimmed = repo_url.trim_end_matches(".git").trim_end_matches('/');
    let parts: Vec<&str> = trimmed.rsplit('/').take(2).collect();
    let joined: String = parts.into_iter().rev().collect::<Vec<_>>().join("_");
    if joined.is_empty() {
        "template".to_string()
    } else {
        joined
    }
}
