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
 -e, --exclude <pattern>  Exclude path/pattern (repeatable)
    --high-memory        Use a 1GiB writer budget for faster indexing
    --embeddings <mode>  auto|precompute|off (default: off)
    --embeddings-force   Force regenerate embeddings
```

Watch:
```
    --debounce <seconds>  Debounce interval for reindex (default: 2)
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
For symbol results, `result_id` is the symbol ID and `chunk_start`/`chunk_end` are the symbol start/end line numbers.

## Embeddings

The repository includes:
- Symbol-level embedding generation (AST symbols)
- SQLite storage at `.cgrep/embeddings.sqlite`
- Provider interface for generating embeddings

### Generating embeddings during indexing

By default, `cgrep index` runs with embeddings disabled (`--embeddings off`).

- `cgrep index --embeddings precompute`: generate embeddings for all indexed files (fails if the embedding provider is unavailable).
- `cgrep index --embeddings auto`: best-effort (if the provider is unavailable, indexing continues without embeddings).
- `cgrep index --embeddings off`: disable embedding generation.
- `cgrep index --embeddings-force`: clear and regenerate all embeddings (useful when changing the embedding model/provider).

Embeddings are stored at `.cgrep/embeddings.sqlite` under the indexed root.

### Embedding provider configuration

Embeddings are generated using the provider configured in `.cgreprc.toml`:

```toml
[embeddings]
provider = "builtin"  # builtin|command|dummy

# command provider
command = "embedder"
model = "local-model-id"

# symbol-level tuning (optional)
max_symbols_per_file = 500
symbol_preview_lines = 12
symbol_max_chars = 1200
# symbol_kinds = ["function", "class", "method"]
```

`provider = "dummy"` is intended for tests/dev only (returns zero vectors).

`chunk_lines` and `chunk_overlap` are deprecated and ignored (embeddings are symbol-level).

For the builtin provider, you can tune FastEmbed via environment variables:
```
FASTEMBED_MODEL=minilm
FASTEMBED_BATCH_SIZE=512
FASTEMBED_MAX_CHARS=2000
FASTEMBED_NORMALIZE=true
```

### Using embeddings in search

If `.cgrep/embeddings.sqlite` exists, `cgrep search --semantic/--hybrid` will use it for embedding-based reranking.
Query embeddings are generated using the configured embedding provider.
If the embedding DB or provider is unavailable, it falls back to BM25-only search.

## Indexing behavior

- Index is stored under `.cgrep/`.
- Search looks for the nearest parent `.cgrep` directory and uses that index
  when you run it from a subdirectory (it prints a notice when this happens).
- Incremental indexing uses BLAKE3 hashes to skip unchanged files.
- Binary files are skipped.
- Large files are chunked (~1MB per document) to limit memory use.
- `.gitignore` is respected.
- `cgrep watch` debounces file events (default: 2s) and rate-limits reindexing
  (minimum 5s between reindexes).
- `cgrep symbols` uses the index (when available) to narrow candidate files,
  falling back to a full scan if no index exists.

## Configuration

Config file locations (first wins):
1) `.cgreprc.toml` in the project root
2) `~/.config/cgrep/config.toml`

Currently used fields:
```toml
max_results = 20
exclude_patterns = ["target/**", "node_modules/**"]

[index]
exclude_paths = ["vendor/", "dist/"]

[embeddings]
provider = "builtin"
# provider = "command"
# command = "embedder"
# model = "local-model-id"
max_file_bytes = 2000000
max_symbols_per_file = 500
symbol_preview_lines = 12
symbol_max_chars = 1200
# symbol_kinds = ["function", "class", "method"]
```
Note: `max_results` is read but the CLI always supplies a default value, so the
config value currently has no effect unless the CLI defaults change.
`[index].exclude_paths` is applied during indexing and is combined with any
`cgrep index --exclude` flags (CLI flags take precedence by being applied first).

Parsed but not applied yet (reserved):
- `index.max_file_size`
- `[search]` (mode, weights, candidate_k)
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

- `--profile` and `--context-pack` are accepted by the CLI but not applied.
- `--format json2` currently outputs the same structure as `json`.

## Development

```bash
cargo build
cargo test
```
