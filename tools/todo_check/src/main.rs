use std::error::Error;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy)]
enum Syntax {
    Slash,
    Hash,
}

fn syntax(path: &Path) -> Option<Syntax> {
    match path.extension()?.to_str()? {
        "rs" | "wgsl" | "c" | "h" | "cc" | "cpp" | "hpp" | "js" | "jsx" | "ts" | "tsx" | "java"
        | "go" | "css" => Some(Syntax::Slash),
        "py" | "sh" | "bash" | "ps1" | "toml" | "yaml" | "yml" | "nix" => Some(Syntax::Hash),
        _ => None,
    }
}

fn has_issue_reference(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(i, &byte)| {
        byte == b'#'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(i + 1).is_some_and(|digit| *digit != b'0')
    }) || text.split("github.com/").skip(1).any(|rest| {
        let parts: Vec<_> = rest.split('/').collect();
        parts.len() >= 4
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts[2] == "issues"
            && parts[3]
                .as_bytes()
                .first()
                .is_some_and(|b| (b'1'..=b'9').contains(b))
    })
}

fn has_unreferenced_todo(comment: &str) -> bool {
    comment.match_indices("TODO").any(|(index, _)| {
        let before = comment[..index].chars().next_back();
        let after = comment[index + 4..].chars().next();
        let is_marker = comment[..index]
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '*' | '!' | '/'))
            || matches!(after, Some(':' | '('));
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && is_marker
            && !has_issue_reference(&comment[index + 4..])
    })
}

#[derive(Default)]
struct Scanner {
    block_depth: usize,
    raw_hashes: Option<usize>,
    quote: Option<u8>,
    triple_quote: bool,
}

impl Scanner {
    fn line_has_unreferenced_todo(&mut self, line: &str, syntax: Syntax, rust: bool) -> bool {
        let bytes = line.as_bytes();
        let mut i = 0;
        let mut comment = String::new();
        while i < bytes.len() {
            if let Some(hashes) = self.raw_hashes {
                if bytes[i] == b'"'
                    && bytes
                        .get(i + 1..i + 1 + hashes)
                        .is_some_and(|end| end.iter().all(|&b| b == b'#'))
                {
                    self.raw_hashes = None;
                    i += 1 + hashes;
                } else {
                    i += 1;
                }
            } else if let Some(quote) = self.quote {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote
                    && (!self.triple_quote || bytes.get(i..i + 3) == Some(&[quote, quote, quote]))
                {
                    i += if self.triple_quote { 3 } else { 1 };
                    self.quote = None;
                    self.triple_quote = false;
                } else {
                    i += 1;
                }
            } else if self.block_depth > 0 {
                if bytes.get(i..i + 2) == Some(b"/*") {
                    self.block_depth += 1;
                    i += 2;
                } else if bytes.get(i..i + 2) == Some(b"*/") {
                    self.block_depth -= 1;
                    i += 2;
                } else {
                    comment.push(bytes[i] as char);
                    i += 1;
                }
            } else if matches!(syntax, Syntax::Slash) && bytes.get(i..i + 2) == Some(b"//") {
                comment.push_str(&line[i + 2..]);
                break;
            } else if matches!(syntax, Syntax::Slash) && bytes.get(i..i + 2) == Some(b"/*") {
                self.block_depth = 1;
                i += 2;
            } else if matches!(syntax, Syntax::Hash) && bytes[i] == b'#' {
                comment.push_str(&line[i + 1..]);
                break;
            } else if rust && bytes[i] == b'r' {
                let start = i + 1;
                let mut end = start;
                while bytes.get(end) == Some(&b'#') {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'"') {
                    self.raw_hashes = Some(end - start);
                    i = end + 1;
                } else {
                    i += 1;
                }
            } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                let quote = bytes[i];
                // Rust lifetimes start with an apostrophe but are not character literals.
                if rust && quote == b'\'' && !is_rust_character_literal(&bytes[i..]) {
                    i += 1;
                    continue;
                }
                self.triple_quote = matches!(syntax, Syntax::Hash)
                    && bytes.get(i..i + 3) == Some(&[quote, quote, quote]);
                self.quote = Some(quote);
                i += if self.triple_quote { 3 } else { 1 };
            } else {
                i += 1;
            }
        }
        has_unreferenced_todo(&comment)
    }
}

fn is_rust_character_literal(bytes: &[u8]) -> bool {
    let Some(end) = bytes.iter().skip(1).position(|&b| b == b'\'') else {
        return false;
    };
    let content = &bytes[1..end + 1];
    if content.first() == Some(&b'\\') {
        !content.is_empty()
    } else {
        std::str::from_utf8(content).is_ok_and(|s| s.chars().count() == 1)
    }
}

fn run() -> Result<bool, Box<dyn Error>> {
    let output = Command::new("git").args(["ls-files", "-z"]).output()?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut failed = false;
    for path in output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|p| !p.is_empty())
    {
        let path = std::str::from_utf8(path)?;
        let path = Path::new(path);
        let Some(syntax) = syntax(path) else {
            continue;
        };
        let contents = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let rust = path.extension().is_some_and(|ext| ext == "rs");
        let mut scanner = Scanner::default();
        for (index, line) in contents.lines().enumerate() {
            if scanner.line_has_unreferenced_todo(line, syntax, rust) {
                eprintln!(
                    "{}:{}: TODO without an issue reference",
                    path.display(),
                    index + 1
                );
                failed = true;
            }
        }
    }
    Ok(!failed)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("TODO check failed: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_references_are_required() {
        assert!(has_unreferenced_todo(" TODO: fix this"));
        assert!(!has_unreferenced_todo(" TODO(#123): fix this"));
        assert!(!has_unreferenced_todo(" TODO(Pulsar-Native#924): fix this"));
        assert!(!has_unreferenced_todo(
            " TODO https://github.com/org/repo/issues/42"
        ));
        assert!(has_unreferenced_todo(" TODO(#0): fix this"));
        assert!(!has_unreferenced_todo(" placeholder TODOs"));
        assert!(!has_unreferenced_todo(" check TODO comments"));
        assert!(!has_unreferenced_todo(" with `TODO` placeholders"));
        assert!(has_unreferenced_todo(" note: TODO: fix this"));
    }

    #[test]
    fn scans_comments_not_strings() {
        let mut scanner = Scanner::default();
        assert!(!scanner.line_has_unreferenced_todo(r#"let s = "// TODO";"#, Syntax::Slash, true));
        assert!(scanner.line_has_unreferenced_todo("let x = 1; // TODO: fix", Syntax::Slash, true));
        assert!(!scanner.line_has_unreferenced_todo("/* TODO #42 */", Syntax::Slash, true));
        assert!(!scanner.line_has_unreferenced_todo("/*", Syntax::Slash, true));
        assert!(scanner.line_has_unreferenced_todo(" * TODO: fix", Syntax::Slash, true));
        assert!(!scanner.line_has_unreferenced_todo(" */", Syntax::Slash, true));
        assert!(scanner.line_has_unreferenced_todo(
            "fn f<'a>() {} // TODO: fix",
            Syntax::Slash,
            true
        ));
    }

    #[test]
    fn scans_multiline_strings_and_hash_comments() {
        let mut scanner = Scanner::default();
        assert!(!scanner.line_has_unreferenced_todo("let s = r#\"", Syntax::Slash, true));
        assert!(!scanner.line_has_unreferenced_todo("// TODO", Syntax::Slash, true));
        assert!(!scanner.line_has_unreferenced_todo("\"#;", Syntax::Slash, true));
        assert!(scanner.line_has_unreferenced_todo("// TODO", Syntax::Slash, true));
        let mut scanner = Scanner::default();
        assert!(!scanner.line_has_unreferenced_todo("x = '# TODO'", Syntax::Hash, false));
        assert!(scanner.line_has_unreferenced_todo("x = 1 # TODO", Syntax::Hash, false));
    }
}
