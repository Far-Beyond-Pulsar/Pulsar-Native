/// Comprehensive autocomplete system with closure completion, 
/// tab completion, dictionary support, and language server integration.

use anyhow::Result;
use gpui::{Context, Task, Window};
use lsp_types::{CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTriggerKind, InsertTextFormat};
use ropey::Rope;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::input::{InputState, RopeExt};

/// A comprehensive completion provider that combines multiple sources
pub struct ComprehensiveCompletionProvider {
    /// Dictionary-based word completion
    dictionary: DictionaryProvider,
    /// Language-specific completions (keywords, snippets)
    language_provider: LanguageProvider,
    /// Closure and bracket completion
    closure_provider: ClosureProvider,
    /// Optional LSP completion provider (rust-analyzer, etc.)
    lsp_provider: Option<Rc<dyn super::CompletionProvider>>,
}

impl ComprehensiveCompletionProvider {
    pub fn new() -> Self {
        Self {
            dictionary: DictionaryProvider::new(),
            language_provider: LanguageProvider::new(),
            closure_provider: ClosureProvider::new(),
            lsp_provider: None,
        }
    }

    /// Set the LSP completion provider (e.g., rust-analyzer)
    pub fn with_lsp_provider(mut self, provider: Rc<dyn super::CompletionProvider>) -> Self {
        self.lsp_provider = Some(provider);
        self
    }

    /// Get completions from all sources and merge them
    /// Priority order (highest to lowest):
    /// 1. Closure/bracket completions (auto-close)
    /// 2. LSP completions (rust-analyzer, etc.) - if available
    /// 3. Language keywords and snippets (if no LSP)
    /// 4. Document-learned words
    /// 5. Common words
    /// 6. System dictionary words (lowest priority)
    pub fn get_completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let mut all_completions = Vec::new();

        // 1. Check for closure/bracket completion first (highest priority)
        if let Some(closure_completion) = self.closure_provider.get_closure_completion(text, offset) {
            all_completions.push(closure_completion);
        }

        // 2. If LSP is available, SKIP language keywords and use LSP instead
        // Dictionary-based completions (document, common, system) are always included
        let has_lsp = self.lsp_provider.is_some();

        // 3. Get dictionary-based completions (document + common + system dictionary)
        let current_word = self.get_current_word(text, offset);
        if !current_word.is_empty() {
            all_completions.extend(self.dictionary.get_completions(&current_word));
        }

        // 4. Get LSP completions (if available)
        if let Some(lsp_provider) = &self.lsp_provider {
            let lsp_task = lsp_provider.completions(text, offset, trigger, window, cx);
            
            // Merge LSP completions with our completions
            return cx.spawn_in(window, async move |_, _cx| {
                let mut combined = all_completions;
                
                if let Ok(lsp_response) = lsp_task.await {
                    match lsp_response {
                        CompletionResponse::Array(items) => combined.extend(items),
                        CompletionResponse::List(list) => combined.extend(list.items),
                    }
                }

                // Sort by priority and remove duplicates
                combined.sort_by(|a, b| {
                    a.sort_text.as_ref()
                        .unwrap_or(&a.label)
                        .cmp(b.sort_text.as_ref().unwrap_or(&b.label))
                });
                combined.dedup_by(|a, b| a.label == b.label);

                Ok(CompletionResponse::Array(combined))
            });
        }

        // If no LSP, fallback to language-specific completions
        let language = self.detect_language(text);
        all_completions.extend(self.language_provider.get_completions(&language, text, offset));

        // Return immediately if no LSP provider
        Task::ready(Ok(CompletionResponse::Array(all_completions)))
    }

    /// Detect the language from the rope content
    fn detect_language(&self, text: &Rope) -> String {
        // Simple heuristic: check first few lines for language-specific patterns
        let first_lines: String = text.slice(0..text.len().min(500))
            .to_string();
        
        if first_lines.contains("fn ") || first_lines.contains("impl ") || first_lines.contains("pub struct") {
            "rust".to_string()
        } else if first_lines.contains("function ") || first_lines.contains("const ") || first_lines.contains("let ") {
            "javascript".to_string()
        } else if first_lines.contains("def ") || first_lines.contains("class ") || first_lines.contains("import ") {
            "python".to_string()
        } else {
            "text".to_string()
        }
    }

    /// Get the current word being typed
    fn get_current_word(&self, text: &Rope, offset: usize) -> String {
        let offset = offset.min(text.len());
        let mut start = offset;
        
        // Move backwards to find word start
        while start > 0 {
            let prev_offset = start.saturating_sub(1);
            if prev_offset < text.len() {
                let ch = text.slice(prev_offset..prev_offset+1).to_string().chars().next().unwrap_or(' ');
                if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                start = start.saturating_sub(1);
            } else {
                break;
            }
        }
        
        text.slice(start..offset).to_string()
    }
}

impl super::CompletionProvider for ComprehensiveCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        self.get_completions(text, offset, trigger, window, cx)
    }

    fn is_completion_trigger(
        &self,
        offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // Trigger on:
        // 1. Alphanumeric characters (word completion)
        // 2. Dot (method completion)
        // 3. Double colon (path completion)
        // 4. Opening brackets/braces (closure completion)
        let triggers = vec!['.', ':', '{', '(', '[', '<'];
        
        new_text.chars().any(|ch| {
            ch.is_alphanumeric() || ch == '_' || triggers.contains(&ch)
        })
    }
}

/// Dictionary-based completion provider that learns from the document
pub struct DictionaryProvider {
    /// Words collected from the current document
    learned_words: HashSet<String>,
    /// Common English words for general text editing
    common_words: HashSet<String>,
    /// System dictionary words (loaded lazily)
    system_words: HashSet<String>,
}

impl DictionaryProvider {
    pub fn new() -> Self {
        let mut common_words = HashSet::new();
        
        // Add common English words
        for word in &[
            "the", "be", "to", "of", "and", "a", "in", "that", "have", "I",
            "it", "for", "not", "on", "with", "he", "as", "you", "do", "at",
            "this", "but", "his", "by", "from", "they", "we", "say", "her", "she",
            "or", "an", "will", "my", "one", "all", "would", "there", "their", "what",
            "function", "return", "public", "private", "class", "interface", "implements",
            "extends", "import", "export", "const", "let", "var", "async", "await",
        ] {
            common_words.insert(word.to_string());
        }
        
        let system_words = Self::load_system_dictionary();
        
        Self {
            learned_words: HashSet::new(),
            common_words,
            system_words,
        }
    }
    
    /// Load system dictionary from common locations
    fn load_system_dictionary() -> HashSet<String> {
        let mut words = HashSet::new();
        
        // Try common dictionary file locations
        let dict_paths = vec![
            "/usr/share/dict/words",           // Unix/Linux
            "/usr/dict/words",                  // Alternative Unix location
            "C:\\Windows\\System32\\en-US.dic", // Windows (hypothetical)
        ];
        
        for path in dict_paths {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let word = line.trim().to_lowercase();
                    // Only include words between 3 and 15 characters
                    if word.len() >= 3 && word.len() <= 15 && word.chars().all(|c| c.is_alphabetic()) {
                        words.insert(word);
                    }
                }
                tracing::info!("✓ Loaded {} words from system dictionary: {}", words.len(), path);
                break; // Use first available dictionary
            }
        }
        
        if words.is_empty() {
            tracing::warn!("⚠ No system dictionary found, using built-in word list only");
        }
        
        words
    }

    /// Learn words from the text
    pub fn learn_from_text(&mut self, text: &str) {
        for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if word.len() >= 3 {
                self.learned_words.insert(word.to_lowercase());
            }
        }
    }

    /// Get completions matching the prefix
    pub fn get_completions(&self, prefix: &str) -> Vec<CompletionItem> {
        if prefix.len() < 2 {
            return vec![];
        }

        let prefix_lower = prefix.to_lowercase();
        let mut completions = Vec::new();

        // Search learned words (medium-high priority)
        for word in &self.learned_words {
            if word.starts_with(&prefix_lower) && word != &prefix_lower {
                completions.push(CompletionItem {
                    label: word.clone(),
                    kind: Some(CompletionItemKind::TEXT),
                    detail: Some("Document".to_string()),
                    sort_text: Some(format!("y_{}", word)), // Medium-high priority
                    ..Default::default()
                });
            }
        }

        // Search common words (medium priority)
        for word in &self.common_words {
            if word.starts_with(&prefix_lower) && word != &prefix_lower {
                completions.push(CompletionItem {
                    label: word.clone(),
                    kind: Some(CompletionItemKind::TEXT),
                    detail: Some("Common".to_string()),
                    sort_text: Some(format!("z_{}", word)), // Medium priority
                    ..Default::default()
                });
            }
        }
        
        // Search system dictionary words (lowest priority)
        // Limit to 20 matches to avoid overwhelming the completion list
        let mut system_matches = 0;
        for word in &self.system_words {
            if system_matches >= 20 {
                break;
            }
            if word.starts_with(&prefix_lower) && word != &prefix_lower {
                // Skip if already in learned or common words
                if !self.learned_words.contains(word) && !self.common_words.contains(word) {
                    completions.push(CompletionItem {
                        label: word.clone(),
                        kind: Some(CompletionItemKind::TEXT),
                        detail: Some("Dictionary".to_string()),
                        sort_text: Some(format!("zz_{}", word)), // Lowest priority
                        ..Default::default()
                    });
                    system_matches += 1;
                }
            }
        }

        completions
    }
}

/// Language-specific completion provider
pub struct LanguageProvider {
    rust_keywords: Vec<String>,
    rust_snippets: HashMap<String, (String, String)>, // (trigger, (replacement, description))
    js_keywords: Vec<String>,
    js_snippets: HashMap<String, (String, String)>,
    python_keywords: Vec<String>,
    python_snippets: HashMap<String, (String, String)>,
}

impl LanguageProvider {
    pub fn new() -> Self {
        let mut rust_snippets = HashMap::new();
        rust_snippets.insert("fn".to_string(), ("fn ${1:name}(${2}) ${3:-> ${4:ReturnType}} {\n    ${5}\n}".to_string(), "Function".to_string()));
        rust_snippets.insert("impl".to_string(), ("impl ${1:Type} {\n    ${2}\n}".to_string(), "Implementation".to_string()));
        rust_snippets.insert("struct".to_string(), ("struct ${1:Name} {\n    ${2}\n}".to_string(), "Struct".to_string()));
        rust_snippets.insert("enum".to_string(), ("enum ${1:Name} {\n    ${2}\n}".to_string(), "Enum".to_string()));
        rust_snippets.insert("match".to_string(), ("match ${1:expression} {\n    ${2:pattern} => ${3},\n}".to_string(), "Match expression".to_string()));
        rust_snippets.insert("if".to_string(), ("if ${1:condition} {\n    ${2}\n}".to_string(), "If statement".to_string()));
        rust_snippets.insert("for".to_string(), ("for ${1:item} in ${2:iterator} {\n    ${3}\n}".to_string(), "For loop".to_string()));
        rust_snippets.insert("while".to_string(), ("while ${1:condition} {\n    ${2}\n}".to_string(), "While loop".to_string()));

        let mut js_snippets = HashMap::new();
        js_snippets.insert("fn".to_string(), ("function ${1:name}(${2:params}) {\n    ${3}\n}".to_string(), "Function".to_string()));
        js_snippets.insert("arrow".to_string(), ("(${1:params}) => {\n    ${2}\n}".to_string(), "Arrow function".to_string()));
        js_snippets.insert("class".to_string(), ("class ${1:Name} {\n    constructor(${2}) {\n        ${3}\n    }\n}".to_string(), "Class".to_string()));
        js_snippets.insert("if".to_string(), ("if (${1:condition}) {\n    ${2}\n}".to_string(), "If statement".to_string()));
        js_snippets.insert("for".to_string(), ("for (let ${1:i} = 0; ${1:i} < ${2:length}; ${1:i}++) {\n    ${3}\n}".to_string(), "For loop".to_string()));

        let mut python_snippets = HashMap::new();
        python_snippets.insert("def".to_string(), ("def ${1:name}(${2:params}):\n    ${3:pass}".to_string(), "Function".to_string()));
        python_snippets.insert("class".to_string(), ("class ${1:Name}:\n    def __init__(self, ${2}):\n        ${3:pass}".to_string(), "Class".to_string()));
        python_snippets.insert("if".to_string(), ("if ${1:condition}:\n    ${2:pass}".to_string(), "If statement".to_string()));
        python_snippets.insert("for".to_string(), ("for ${1:item} in ${2:iterable}:\n    ${3:pass}".to_string(), "For loop".to_string()));

        Self {
            rust_keywords: vec![
                "as", "break", "const", "continue", "crate", "else", "enum", "extern",
                "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
                "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
                "async", "await", "dyn",
            ].iter().map(|s| s.to_string()).collect(),
            rust_snippets,
            js_keywords: vec![
                "break", "case", "catch", "class", "const", "continue", "debugger", "default",
                "delete", "do", "else", "export", "extends", "finally", "for", "function",
                "if", "import", "in", "instanceof", "let", "new", "return", "super", "switch",
                "this", "throw", "try", "typeof", "var", "void", "while", "with", "yield",
                "async", "await",
            ].iter().map(|s| s.to_string()).collect(),
            js_snippets,
            python_keywords: vec![
                "False", "None", "True", "and", "as", "assert", "async", "await", "break",
                "class", "continue", "def", "del", "elif", "else", "except", "finally",
                "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
                "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
            ].iter().map(|s| s.to_string()).collect(),
            python_snippets,
        }
    }

    pub fn get_completions(&self, language: &str, text: &Rope, offset: usize) -> Vec<CompletionItem> {
        let mut completions = Vec::new();

        let (keywords, snippets) = match language {
            "rust" => (&self.rust_keywords, &self.rust_snippets),
            "javascript" | "typescript" => (&self.js_keywords, &self.js_snippets),
            "python" => (&self.python_keywords, &self.python_snippets),
            _ => return completions,
        };

        // Get current word
        let current_word = self.get_word_at_offset(text, offset);
        
        if current_word.is_empty() {
            return completions;
        }

        // Add keyword completions
        for keyword in keywords {
            if keyword.starts_with(&current_word) {
                completions.push(CompletionItem {
                    label: keyword.clone(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("Keyword".to_string()),
                    sort_text: Some(format!("a_{}", keyword)), // High priority
                    ..Default::default()
                });
            }
        }

        // Add snippet completions
        for (trigger, (snippet, description)) in snippets {
            if trigger.starts_with(&current_word) {
                completions.push(CompletionItem {
                    label: trigger.clone(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(description.clone()),
                    insert_text: Some(snippet.clone()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    sort_text: Some(format!("b_{}", trigger)), // High priority
                    ..Default::default()
                });
            }
        }

        completions
    }

    fn get_word_at_offset(&self, text: &Rope, offset: usize) -> String {
        let offset = offset.min(text.len());
        let mut start = offset;
        
        while start > 0 {
            let prev_offset = start.saturating_sub(1);
            if prev_offset < text.len() {
                let ch = text.slice(prev_offset..prev_offset+1).to_string().chars().next().unwrap_or(' ');
                if !ch.is_alphanumeric() && ch != '_' {
                    break;
                }
                start = start.saturating_sub(1);
            } else {
                break;
            }
        }
        
        text.slice(start..offset).to_string()
    }
}

/// Closure and bracket completion provider
pub struct ClosureProvider {
    bracket_pairs: HashMap<char, char>,
}

impl ClosureProvider {
    pub fn new() -> Self {
        let mut bracket_pairs = HashMap::new();
        bracket_pairs.insert('(', ')');
        bracket_pairs.insert('{', '}');
        bracket_pairs.insert('[', ']');
        bracket_pairs.insert('<', '>');
        bracket_pairs.insert('"', '"');
        bracket_pairs.insert('\'', '\'');
        
        Self { bracket_pairs }
    }

    /// Get closure completion for the given offset
    /// Returns a completion item that inserts the closing bracket/quote
    pub fn get_closure_completion(&self, text: &Rope, offset: usize) -> Option<CompletionItem> {
        if offset == 0 {
            return None;
        }

        // Get the character just before the cursor
        let prev_offset = offset.saturating_sub(1);
        if prev_offset >= text.len() {
            return None;
        }

        let ch = text.slice(prev_offset..offset).to_string().chars().next()?;
        
        // Check if it's an opening bracket/quote
        if let Some(&closing) = self.bracket_pairs.get(&ch) {
            // Check if we should auto-close (don't auto-close if closing bracket already exists)
            if offset < text.len() {
                let next_ch = text.slice(offset..offset+1).to_string().chars().next().unwrap_or(' ');
                // Don't auto-close if the next character is the closing bracket
                if next_ch == closing {
                    return None;
                }
            }

            // Create a completion item for the closing bracket
            Some(CompletionItem {
                label: format!("{}{}", ch, closing),
                kind: Some(CompletionItemKind::TEXT),
                detail: Some("Auto-close".to_string()),
                insert_text: Some(closing.to_string()),
                sort_text: Some("aaa_closure".to_string()), // Highest priority
                preselect: Some(true),
                ..Default::default()
            })
        } else {
            None
        }
    }

    /// Check if a character should trigger closure completion
    pub fn is_closure_trigger(&self, ch: char) -> bool {
        self.bracket_pairs.contains_key(&ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictionary_completions() {
        let mut dict = DictionaryProvider::new();
        dict.learn_from_text("hello world wonderful weather");
        
        let completions = dict.get_completions("wor");
        assert!(completions.iter().any(|c| c.label == "world"));
    }

    #[test]
    fn test_closure_completion() {
        let closure = ClosureProvider::new();
        let text = Rope::from("test(");
        let completion = closure.get_closure_completion(&text, 5);
        
        assert!(completion.is_some());
        assert_eq!(completion.unwrap().insert_text, Some(")".to_string()));
    }

    #[test]
    fn test_language_completions() {
        let lang = LanguageProvider::new();
        let text = Rope::from("f");
        let completions = lang.get_completions("rust", &text, 1);
        
        assert!(completions.iter().any(|c| c.label == "fn"));
    }
}
