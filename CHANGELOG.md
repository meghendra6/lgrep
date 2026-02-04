# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Symbol-level embeddings for semantic/hybrid search (AST-derived symbols).
- Embedding generation during `cgrep index` with `--embeddings auto|precompute|off` and `--embeddings-force`.
- Hybrid and semantic search modes with `--hybrid`, `--semantic`, and `--keyword`, plus `--format json2` for agent-friendly output.
- Embedding provider configuration (builtin/command/dummy) with SQLite storage for vectors.
- Agent session cache with `--agent-cache` and `--cache-ttl`.
- Parent index lookup and index-time exclude paths.

### Changed
- FastEmbed MiniLM batching/truncation for faster embedding generation.
- Search results are scoped to the current directory by default.
- Indexing performance and correctness improvements.
- Faster definition/callers/references lookups.
- Improved context output readability.
- Indexing now includes gitignored paths.
- Documentation updates for indexing, watch mode, and agent install instructions.

### Removed
- `cg` shortcut binary. Use `cgrep search <query>` directly.

## [1.1.0] - 2026-02-01

### Added
- Scan mode fallback when index doesn't exist
- Regex search support (`--regex` flag)
- Case-sensitive search option (`--case-sensitive` flag)
- Context lines display (`-C, --context` flag)

### Changed
- Improved indexing performance with parallel parsing
- Better file type detection and filtering
- Enhanced output formatting with colors

### Fixed
- Binary file detection improvements
- Large file handling with chunked indexing

## [1.0.0] - 2026-02-01

### Added
- BM25 full-text search (Tantivy)
- AST-based symbol extraction (tree-sitter)
- Multi-language: TS, JS, Python, Rust, Go, C, C++, Java, Ruby
- AI agent integrations: Copilot, Claude Code, Codex, OpenCode
- JSON output format
- Incremental indexing with parallel parsing
- Shell completions
