# cgrep

Local code search with BM25 ranking and AST-aware symbols. Fully local, single
binary. Includes scan mode fallback and experimental hybrid/semantic search.

## Why cgrep
- Fast BM25 index (Tantivy)
- AST-aware symbol lookup (tree-sitter)
- Respects .gitignore (ignore crate)
- Single binary, no cloud dependencies
- JSON output for automation and agents
- Scan mode with regex when you do not want to index

## Install

### From source
```bash
cargo build --release
cp target/release/cgrep ~/.local/bin/
cp target/release/cg ~/.local/bin/
```

### With cargo
```bash
cargo install --path .
```

## Quick start
```bash
# Build the search index
cgrep index

# Full-text search (BM25)
cgrep search "authentication flow"

# Search with context and file type filter
cgrep search "auth middleware" -C 2 -t typescript

# Symbol lookups
cgrep symbols UserService -T class
cgrep definition handleAuth
cgrep callers validateToken
cgrep references MyClass

# File dependency search
cgrep dependents src/auth.rs
```

Shortcut:
```bash
# cg is a thin wrapper around cgrep
cg "authentication flow"
```

## Commands

| Command | Description |
|--------|-------------|
| `cgrep search <query>` | Full-text search (BM25), or hybrid/semantic if enabled |
| `cgrep symbols <name>` | Search symbols by name |
| `cgrep definition <name>` | Find symbol definition location |
| `cgrep callers <function>` | Find callers of a function |
| `cgrep references <name>` | Find symbol references |
| `cgrep dependents <file>` | Find files that depend on a file |
| `cgrep index` | Build or rebuild the search index |
| `cgrep watch` | Watch for file changes and update index |
| `cgrep completions <shell>` | Generate shell completions |
| `cgrep install-*` | Install agent instructions |
| `cgrep uninstall-*` | Uninstall agent instructions |

## Search modes

`cgrep search` supports three modes:

- `keyword` (default): BM25 only
- `semantic`: embeddings only (experimental)
- `hybrid`: BM25 + embeddings (experimental)

Examples:
```bash
cgrep search "token refresh" --mode keyword
cgrep search "token refresh" --mode semantic
cgrep search "token refresh" --mode hybrid

# Shorthand aliases
cgrep search "token refresh" --keyword
cgrep search "token refresh" --semantic
cgrep search "token refresh" --hybrid
```

Important notes:
- Hybrid/semantic require a BM25 index. If the index is missing, cgrep returns
  an error (no scan fallback for these modes).
- If no embedding database exists, cgrep prints a warning and falls back to BM25.

## Search flags

Common flags:
```
-p, --path <path>        Path to search in (default: current directory)
-m, --max-results <n>    Maximum results (default: 20)
-C, --context <n>        Context lines before/after matches
-t, --type <type>        File type filter (rust, ts, python, ...)
-g, --glob <pattern>     Glob filter (e.g. "src/**/*.rs")
    --exclude <pattern>  Exclude pattern
-q, --quiet              Suppress statistics output
-f, --fuzzy              Fuzzy BM25 matching (index mode only)
```

Scan mode:
```
    --no-index           Force scan mode (no index)
    --regex              Regex search (scan mode only)
    --case-sensitive     Case-sensitive (scan mode only)
```
Note: In keyword mode, if no index exists, cgrep falls back to scan mode.

Agent/experimental:
```
    --agent-cache        Cache hybrid/semantic results
    --cache-ttl <ms>     Cache TTL in milliseconds (default: 600000)
    --context-pack <n>   Accepted but not implemented yet
    --profile <name>     Accepted but not implemented yet
```

## Command-specific flags

Symbols:
```
-T, --type <type>        Symbol kind (function, class, ...)
-l, --lang <lang>        Language filter
-t, --file-type <type>   File type filter
-g, --glob <pattern>     Glob filter
    --exclude <pattern>  Exclude pattern
```

References:
```
-p, --path <path>        Path to search in
-m, --max-results <n>    Maximum results (default: 50)
```

Index:
```
-p, --path <path>        Path to index
-f, --force              Force full reindex
    --embeddings <mode>  auto|precompute|off (accepted, not wired yet)
    --embeddings-force   Force regenerate embeddings (accepted, not wired yet)
```

## Output formats

Global flag:
```
--format text|json|json2
```

- `text`: human-readable output
- `json`: array of results
- `json2`: currently identical to `json` (reserved for structured output)

Search result JSON fields:
```
path, score, snippet, line, context_before, context_after
text_score, vector_score, hybrid_score, result_id, chunk_start, chunk_end
```
Optional fields appear only in hybrid/semantic mode.

## Embeddings (current state)

The repository includes:
- Chunking logic (default: 80 lines per chunk, 20 lines overlap)
- SQLite storage at `.cgrep/embeddings.sqlite`
- Provider interface for generating embeddings

Current limitations:
- `cgrep index` does NOT generate embeddings yet. The `--embeddings` flags are
  accepted by the CLI but not wired to the indexer.
- `cgrep search --semantic/--hybrid` uses a dummy embedding provider for query
  embeddings (placeholder). Without a real provider and stored embeddings,
  semantic scoring is not meaningful.

If you want real semantic search today, you must:
1) Generate embeddings externally.
2) Store them in `.cgrep/embeddings.sqlite` using the schema in
   `src/embedding/storage.rs`.
3) Wire a real embedding provider into the search path (library has a command
   provider interface, but the CLI does not use it yet).

## Indexing behavior

- Index is stored under `.cgrep/`.
- Incremental indexing uses BLAKE3 hashes to skip unchanged files.
- Binary files are skipped.
- Large files are chunked (~1MB per document) to limit memory use.
- `.gitignore` is respected.

## Configuration

Config file locations (first wins):
1) `.cgreprc.toml` in the project root
2) `~/.config/cgrep/config.toml`

Currently used fields:
```toml
max_results = 20
exclude_patterns = ["target/**", "node_modules/**"]
```
Note: `max_results` is read but the CLI always supplies a default value, so the
config value currently has no effect unless the CLI defaults change.

Parsed but not applied yet (reserved):
- `[search]` (mode, weights, candidate_k)
- `[embeddings]` (provider, model, chunking)
- `[cache]` (enabled, ttl)
- `[profile.*]` presets
- `default_format` (only text/json supported in code path)

## Supported languages

AST symbol extraction (tree-sitter):
- typescript, tsx, javascript, python, rust, go, c, cpp, java, ruby

Full-text indexing/scanning (file extensions):
- rs, ts, tsx, js, jsx, py, go, java, c, cpp, h, hpp, cs, rb, php, swift
- kt, kts, scala, lua, md, txt, json, yaml, toml

Files outside these extensions are ignored.

## Agent integrations

These commands install local instruction files so your agent uses cgrep
for code search:
```
cgrep install-claude-code
cgrep install-codex
cgrep install-copilot
cgrep install-opencode
```

Uninstall:
```
cgrep uninstall-claude-code
cgrep uninstall-codex
cgrep uninstall-copilot
cgrep uninstall-opencode
```

## Troubleshooting

- Index not found:
  - Run `cgrep index`
- Hybrid/semantic search returns BM25-only results:
  - Ensure `.cgrep/embeddings.sqlite` exists and contains embeddings
- Symbols not found for a language:
  - Only the AST-supported languages above provide symbol extraction

## Known limitations

- `cg` with no arguments shows `cgrep --help` (TUI is not implemented yet).
- `--profile` and `--context-pack` are accepted by the CLI but not applied.
- `--format json2` currently outputs the same structure as `json`.

## Development

```bash
cargo build
cargo test
```
