//! Short, human-readable previews of tool output for the UI.

pub fn format_tool_result_preview(tool_name: &str, raw: &str) -> String {
    if tool_name == "web_search" {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(results) = json.get("results").and_then(|r| r.as_array()) {
                let lines: Vec<String> = results
                    .iter()
                    .take(5)
                    .enumerate()
                    .map(|(i, r)| {
                        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("—");
                        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        let summary = r
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .chars()
                            .take(120)
                            .collect::<String>();
                        format!("[{}] {}\n    {}\n    {}", i + 1, title, summary, url)
                    })
                    .collect();
                return lines.join("\n\n");
            }
        }
    }

    if tool_name == "fetch_url" {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(content) = json.get("content").and_then(|v| v.as_str()) {
                let preview: String = content.chars().take(400).collect();
                return format!("{}…", preview);
            }
        }
    }

    if raw.len() > 300 {
        format!(
            "{}…",
            &raw[..raw
                .char_indices()
                .nth(300)
                .map(|(i, _)| i)
                .unwrap_or(raw.len())]
        )
    } else {
        raw.to_string()
    }
}
