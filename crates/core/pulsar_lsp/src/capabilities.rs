//! The client capabilities the host announces to every server.

use serde_json::{json, Value};

use pulsar_language::spec::merge_json;

/// The base set: what the editor can do with a server's answers, for every
/// language. A spec adds its own (`LanguageServerSpec::client_capabilities`).
pub fn base() -> Value {
    json!({
        "workspace": {
            "configuration": true,
            "workspaceFolders": true,
            "applyEdit": true,
            "workspaceEdit": {
                "documentChanges": true,
                "resourceOperations": ["create", "rename", "delete"],
                "failureHandling": "abort"
            },
            "symbol": {
                "symbolKind": { "valueSet": (1..=26).collect::<Vec<u32>>() }
            },
            "didChangeConfiguration": { "dynamicRegistration": false },
            "didChangeWatchedFiles": { "dynamicRegistration": false }
        },
        "textDocument": {
            "synchronization": {
                "dynamicRegistration": false,
                "willSave": false,
                "willSaveWaitUntil": false,
                "didSave": true
            },
            "completion": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": true,
                    "commitCharactersSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "preselectSupport": true,
                    "tagSupport": { "valueSet": [1] },
                    "insertReplaceSupport": true,
                    "resolveSupport": {
                        "properties": ["documentation", "detail", "additionalTextEdits"]
                    },
                    "insertTextModeSupport": { "valueSet": [1, 2] },
                    "labelDetailsSupport": true
                },
                "completionItemKind": { "valueSet": (1..=25).collect::<Vec<u32>>() }
            },
            "hover": {
                "dynamicRegistration": false,
                "contentFormat": ["markdown", "plaintext"]
            },
            "signatureHelp": {
                "dynamicRegistration": false,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true
                },
                "contextSupport": true
            },
            "definition": { "dynamicRegistration": false, "linkSupport": true },
            "references": { "dynamicRegistration": false },
            "documentHighlight": { "dynamicRegistration": false },
            "documentSymbol": {
                "dynamicRegistration": false,
                "symbolKind": { "valueSet": (1..=26).collect::<Vec<u32>>() },
                "hierarchicalDocumentSymbolSupport": true
            },
            "codeAction": {
                "dynamicRegistration": false,
                "codeActionLiteralSupport": {
                    "codeActionKind": {
                        "valueSet": ["", "quickfix", "refactor", "refactor.extract",
                            "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"]
                    }
                },
                "resolveSupport": { "properties": ["edit"] },
                "dataSupport": true
            },
            "rename": { "dynamicRegistration": false, "prepareSupport": true },
            "publishDiagnostics": {
                "relatedInformation": true,
                "versionSupport": true,
                "tagSupport": { "valueSet": [1, 2] },
                "codeDescriptionSupport": true,
                "dataSupport": true
            },
            "foldingRange": {
                "dynamicRegistration": false,
                "rangeLimit": 5000,
                "lineFoldingOnly": true
            },
            "semanticTokens": {
                "dynamicRegistration": false,
                "tokenTypes": [
                    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter",
                    "parameter", "variable", "property", "enumMember", "event", "function",
                    "method", "macro", "keyword", "modifier", "comment", "string", "number",
                    "regexp", "operator", "decorator"
                ],
                "tokenModifiers": [
                    "declaration", "definition", "readonly", "static", "deprecated",
                    "abstract", "async", "modification", "documentation", "defaultLibrary"
                ],
                "formats": ["relative"],
                "requests": { "range": true, "full": { "delta": true } },
                "multilineTokenSupport": false,
                "overlappingTokenSupport": false
            },
            "inlayHint": {
                "dynamicRegistration": false,
                "resolveSupport": {
                    "properties": ["tooltip", "textEdits", "label.tooltip", "label.location", "label.command"]
                }
            }
        },
        "window": {
            "workDoneProgress": true,
            "showMessage": { "messageActionItem": { "additionalPropertiesSupport": true } },
            "showDocument": { "support": false }
        },
        "general": {
            "staleRequestSupport": {
                "cancel": true,
                "retryOnContentModified": [
                    "textDocument/semanticTokens/full",
                    "textDocument/semanticTokens/full/delta",
                    "textDocument/semanticTokens/range"
                ]
            },
            "regularExpressions": { "engine": "ECMAScript", "version": "ES2020" },
            "markdown": { "parser": "marked", "version": "1.1.0" },
            "positionEncodings": ["utf-16"]
        }
    })
}

/// The base set with a spec's additions merged on top.
pub fn with_spec(extra: Value) -> Value {
    let mut capabilities = base();
    if !extra.is_null() {
        merge_json(&mut capabilities, &extra);
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_adds_to_the_base_without_replacing_it() {
        let merged = with_spec(json!({ "experimental": { "serverStatusNotification": true } }));
        assert_eq!(merged["experimental"]["serverStatusNotification"], true);
        assert_eq!(merged["workspace"]["applyEdit"], true);
        assert_eq!(with_spec(Value::Null), base());
    }
}
