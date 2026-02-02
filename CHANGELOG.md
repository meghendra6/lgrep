# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] - 2026-02-03

### Added
- **Hybrid Search**: Combine BM25 keyword search with vector embeddings for semantic code search
- **Embedding Storage**: SQLite-based persistent storage for embedding vectors with incremental updates
- **Code Chunker**: Configurable chunking for code embedding (80 lines default, 20 lines overlap)
- **Embedding Provider**: Trait-based interface with CommandProvider and DummyProvider implementations
- **`cg` Shortcut**: Convenience binary where `cg <query>` equals `cgrep search <query>`
- **Agent Session Cache**: File-based cache with TTL support for repeated queries
- **New CLI flags**:
  - `--hybrid` - Use hybrid BM25 + vector search
  - `--semantic` - Use semantic (vector) search only
  - `--keyword` - Use keyword (BM25) search only
  - `--agent-cache` - Enable result caching for agents
  - `--cache-ttl <ms>` - Cache TTL in milliseconds
  - `--format json2` - Structured JSON output for AI agents
- Stable result IDs using BLAKE3 hashing for agent session continuity
- Updated AI agent installation files with new features documentation

### Changed
- Search module refactored to support multiple search modes
- Improved agent integration documentation in install commands

## [1.1.0] - 2026-02-02

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
