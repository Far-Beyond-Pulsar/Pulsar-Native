# pulsar_language

Languages as plugin contributions: the `Language` registration and the registry that routes files and classes to it. It is GPUI-free.

- Owns: `Language` (identity, file extensions, class marker files, an optional script compiler, an optional language server spec), `LanguageRegistry`, `ScriptLanguage` and `LanguageServerSpec`. The host names no language; a language arrives with its plugin.
- Entry points: `LanguageRegistry::current`, `for_path`, `class_languages`, `compilers`, `with_servers`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar-config`, `pulsar_plugins`, `pulsar_script_vm`, `pulsar_settings`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI; any specific language or server.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md), [`.agents/LANGUAGE_PLUGINS_PLAN.md`](../../../.agents/LANGUAGE_PLUGINS_PLAN.md).
