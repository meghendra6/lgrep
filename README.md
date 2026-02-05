# cgrep

Local code search with BM25 ranking and AST-aware symbols. Fully local, single
binary. Includes scan mode fallback and optional hybrid/semantic search.

## Why cgrep
- Fast BM25 index (Tantivy)
- AST-aware symbol lookup (tree-sitter)
- Single binary, no cloud dependencies
- JSON output for automation and agents
- Scan mode with regex when you do not want to index

## Quick install
```bash
cargo install --path .

# or build from source:
cargo build --release
cp target/release/cgrep ~/.local/bin/
```

## Quick start
```bash
# Build the BM25 index (embeddings are off by default)
cgrep index

# Full-text search
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

## When to use what
- `cgrep index` then `cgrep search` for repeated searches or large repos.
- `cgrep search --no-index` for one-off searches or when you do not want an index.
- `cgrep search --regex` for regex-only scans.
- `cgrep symbols`, `definition`, `callers`, `references` when you know the symbol name.
- `cgrep dependents` to find files importing a given file.
- `cgrep watch` to keep the index fresh while you code.
- `cgrep search --semantic` or `cgrep search --hybrid` when embeddings are configured and you want semantic ranking.

## Commands

| Command | Description |
|--------|-------------|
| `cgrep search <query>` (`s`) | Full-text search (BM25), or hybrid/semantic if enabled |
| `cgrep symbols <name>` | Search symbols by name |
| `cgrep definition <name>` (`def`) | Find symbol definition location |
| `cgrep callers <function>` | Find callers of a function |
| `cgrep references <name>` (`refs`) | Find symbol references |
| `cgrep dependents <file>` (`deps`) | Find files that depend on a file |
| `cgrep index` | Build or rebuild the search index |
| `cgrep watch` | Watch for file changes and update index |
| `cgrep completions <shell>` | Generate shell completions |
| `cgrep install-*` | Install agent instructions |
| `cgrep uninstall-*` | Uninstall agent instructions |

## Search modes

`cgrep search` supports three modes:

- `keyword` (default): BM25. Uses the index when available and falls back to scan mode.
- `semantic`: embeddings-based ranking (experimental; requires an index).
- `hybrid`: BM25 + embeddings (experimental; requires an index).

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

Notes:
- Hybrid/semantic require a BM25 index; there is no scan fallback.
- If the embedding database or provider is unavailable, hybrid/semantic return BM25-only results with a warning.

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

Mode selection:
```
    --mode <mode>        keyword|semantic|hybrid
    --keyword            Alias for --mode keyword
    --semantic           Alias for --mode semantic
    --hybrid             Alias for --mode hybrid
```

Scan mode:
```
    --no-index           Force scan mode (no index)
    --regex              Regex search (scan mode only)
    --case-sensitive     Case-sensitive (scan mode only)
```
Note: In keyword mode, if no index exists, cgrep falls back to scan mode.

Agent/cache:
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
-q, --quiet              Suppress statistics output
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
path, snippet, line
context_before, context_after
```

## Embeddings (optional)

Embeddings are only used for semantic/hybrid search. By default, `cgrep index`
runs with embeddings disabled (`--embeddings off`).

Enable during indexing:
- `cgrep index --embeddings auto`: best-effort (continues without embeddings if the provider is unavailable).
- `cgrep index --embeddings precompute`: generate embeddings for all indexed files (fails if the provider is unavailable).
- `cgrep index --embeddings-force`: clear and regenerate embeddings (useful when changing the model/provider).

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

Note: the builtin FastEmbed provider is not available on `x86_64-apple-darwin` builds.
Use `provider = "command"` or `provider = "dummy"` on that target.

Linux note: the builtin provider uses a dynamically loaded ONNX Runtime library.
Install a compatible `libonnxruntime` and set `ORT_DYLIB_PATH` (or ensure it is
discoverable via system library paths/`LD_LIBRARY_PATH`), or use
`provider = "command"` / `provider = "dummy"`.

For the builtin provider, you can tune FastEmbed via environment variables:
```
FASTEMBED_MODEL=minilm
FASTEMBED_BATCH_SIZE=512
FASTEMBED_MAX_CHARS=2000
FASTEMBED_NORMALIZE=true
```

### Using embeddings in search

If `.cgrep/embeddings.sqlite` exists, `cgrep search --semantic/--hybrid` uses it
for embedding-based scoring. Query embeddings are generated using the
configured provider. If the DB or provider is unavailable, it falls back to
BM25-only results with a warning.

## Indexing behavior

- Index is stored under `.cgrep/`.
- Search looks for the nearest parent `.cgrep` directory and uses that index
  when you run it from a subdirectory (it prints a notice when this happens).
- Results are scoped to the current directory by default; use `-p` to change the scope.
- Incremental indexing uses BLAKE3 hashes to skip unchanged files.
- Binary files are skipped.
- Large files are chunked (~1MB per document) to limit memory use.
- Indexing does not respect `.gitignore`; use `--exclude` or `[index].exclude_paths`
  to skip paths. Scan mode respects `.gitignore`.
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
enabled = "auto" # off|auto|on
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
Notes:
- `max_results` is read but the CLI always supplies a default value, so the
  config value has no effect unless the CLI defaults change.
- `[index].exclude_paths` is applied during indexing and combined with
  `cgrep index --exclude` (CLI flags are applied first).

Parsed but not applied yet:
- `index.max_file_size`
- `[search]` (default_mode, weights, candidate_k)
- `[cache]` (enabled, ttl_ms)
- `[profile.*]` presets
- `default_format` (only text/json supported in parser)

## Supported languages

AST symbol extraction (tree-sitter):
- typescript, tsx, javascript, python, rust, go, c, cpp, java, ruby

Full-text indexing/scanning (file extensions):
- rs, ts, tsx, js, jsx, py, go, java, c, cpp, h, hpp, cs, rb, php, swift
- kt, kts, scala, lua, md, txt, json, yaml, toml

Files outside these extensions are ignored.

## Agent integrations

Install local instruction files so your agent uses cgrep for code search:
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

What gets updated:
- Claude Code: `~/.claude/CLAUDE.md`
- Codex: `~/.codex/AGENTS.md`
- GitHub Copilot: `.github/instructions/cgrep.instructions.md` and optional append to `.github/copilot-instructions.md`
- OpenCode: `~/.config/opencode/tool/cgrep.ts` (you may need to add it to OpenCode config)

Agent usage tips:
- Use `--format json` or `--format json2` for structured output.
- Add `-C` for context lines.
- For hybrid/semantic sessions, enable caching with `--agent-cache` and adjust `--cache-ttl`.

## Troubleshooting

- Index not found or hybrid/semantic error: run `cgrep index` (required for `--semantic/--hybrid`).
- Results missing when running from a subdirectory: use `-p` to change the search scope.
- Hybrid/semantic returns BM25-only results: ensure `.cgrep/embeddings.sqlite` exists and your embedding provider is configured.
- Symbols not found for a language: only the AST-supported languages above provide symbol extraction.

## Known limitations

- `--profile` and `--context-pack` are accepted by the CLI but not applied.
- `--format json2` currently outputs the same structure as `json`.
- `[search]`, `[cache]`, `[profile.*]`, `default_format`, and `index.max_file_size` are parsed but not applied yet.

## Development

```bash
cargo build
cargo test
```
