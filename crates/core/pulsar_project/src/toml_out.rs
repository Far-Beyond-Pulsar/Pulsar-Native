//! A small, deterministic TOML writer for the manifests people read: one
//! `[section]` per table, `key = value` lines with dependency specs as
//! inline tables (`x = { path = "..." }`, the way Cargo manifests are
//! written by hand). Key order is the caller's; nothing depends on a map's
//! iteration order.

use std::fmt::Write;

/// A document under construction.
#[derive(Default)]
pub struct Doc {
    out: String,
}

impl Doc {
    pub fn new() -> Self {
        Self::default()
    }

    /// `# line` for each line of `text`.
    pub fn comment(&mut self, text: &str) -> &mut Self {
        for line in text.lines() {
            if line.is_empty() {
                self.out.push_str("#\n");
            } else {
                let _ = writeln!(self.out, "# {line}");
            }
        }
        self
    }

    /// A `[header]` (path segments are quoted as needed), preceded by a blank
    /// line unless it opens the document.
    pub fn section(&mut self, path: &[&str]) -> &mut Self {
        self.blank();
        let header: Vec<String> = path.iter().map(|segment| key(segment)).collect();
        let _ = writeln!(self.out, "[{}]", header.join("."));
        self
    }

    /// A `[[header]]` array-of-tables entry.
    pub fn array_section(&mut self, name: &str) -> &mut Self {
        self.blank();
        let _ = writeln!(self.out, "[[{}]]", key(name));
        self
    }

    pub fn entry(&mut self, name: &str, value: &toml::Value) -> &mut Self {
        let _ = writeln!(self.out, "{} = {}", key(name), inline(value));
        self
    }

    pub fn str(&mut self, name: &str, value: &str) -> &mut Self {
        self.entry(name, &toml::Value::String(value.to_owned()))
    }

    pub fn finish(mut self) -> String {
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }
}

/// A key, bare when TOML allows it, quoted otherwise.
pub fn key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        name.to_owned()
    } else {
        quoted(name)
    }
}

/// `value` on one line. Tables are inline, their keys sorted.
pub fn inline(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => quoted(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => {
            let text = f.to_string();
            if text.contains(['.', 'e', 'E', 'n', 'i']) {
                text
            } else {
                format!("{text}.0")
            }
        }
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(inline).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(table) => {
            if table.is_empty() {
                return "{}".to_owned();
            }
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{} = {}", key(k), inline(&table[k])))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
    }
}

fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_parses_back_to_the_same_values() {
        let mut spec = toml::Table::new();
        spec.insert("path".into(), "a/b \"c\"\\d".into());
        spec.insert(
            "features".into(),
            toml::Value::Array(vec!["x".into(), "y".into()]),
        );
        spec.insert("optional".into(), true.into());
        let mut doc = Doc::new();
        doc.comment("Generated.\n\nTwo lines.")
            .section(&["package"])
            .str("name", "game")
            .section(&["target", "cfg(windows)", "dependencies"])
            .entry("dep", &toml::Value::Table(spec.clone()))
            .entry("tracing-subscriber", &toml::Value::Float(1.0));
        let text = doc.finish();
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(parsed["package"]["name"].as_str(), Some("game"));
        assert_eq!(
            parsed["target"]["cfg(windows)"]["dependencies"]["dep"],
            toml::Value::Table(spec)
        );
        assert!(text.contains("[target.\"cfg(windows)\".dependencies]"));
        assert!(text.contains("dep = { features = [\"x\", \"y\"], optional = true,"));
    }
}
