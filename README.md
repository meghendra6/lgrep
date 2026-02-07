# cgrep

Local code search for humans and AI agents.

`cgrep` combines:
- BM25 full-text search (Tantivy)
- AST symbol extraction (tree-sitter)
- optional semantic/hybrid search with embeddings
- deterministic JSON output for tool/agent workflows

Everything runs locally.

## Why cgrep

- Fast search in medium/large codebases
- Better code-aware lookup than plain grep for symbols/definitions
- Agent-friendly output (`json2`) and payload controls
- Works offline after install

## Install

```bash
cargo install --path .

# or build manually
cargo build --release
cp target/release/cgrep ~/.local/bin/
```

## Quick Start (Human)

```bash
# 1) Build index (recommended for repeated searches)
cgrep index

# 2) Basic search
cgrep search "authentication flow"

# 3) Narrow by language/path
cgrep search "token refresh" -t rust -p src/

# 4) Search only changed files
cgrep search "retry logic" --changed

# 5) Symbol/navigation commands
cgrep symbols UserService -T class
cgrep definition handle_auth
cgrep callers validate_token
cgrep references UserService

# 6) Dependency lookup
cgrep dependents src/auth.rs
```

## Quick Start (AI Agent)

Agent workflow is intentionally 2-stage:

1. `locate`: return small candidate set
2. `expand`: fetch richer context only for selected IDs

```bash
# Stage 1: locate (always json2)
cgrep agent locate "where token validation happens" --changed --budget balanced --compact

# Example: pick first ID (requires jq)
ID=$(cgrep agent locate "token validation" --compact | jq -r '.results[0].id')

# Stage 2: expand selected IDs
cgrep agent expand --id "$ID" -C 8 --compact
```

Notes:
- `agent locate/expand` are optimized for low-token loops.
- `agent locate` applies caching + payload minimization defaults.

## Command Overview

| Command | Description |
|---|---|
| `cgrep search <query>` (`s`) | Full-text search |
| `cgrep agent locate <query>` | Agent stage 1 candidate retrieval |
| `cgrep agent expand --id <id>...` | Agent stage 2 context expansion |
| `cgrep symbols <name>` | Symbol search |
| `cgrep definition <name>` (`def`) | Definition lookup |
| `cgrep callers <function>` | Caller lookup |
| `cgrep references <name>` (`refs`) | References lookup |
| `cgrep dependents <file>` (`deps`) | Reverse dependency lookup |
| `cgrep index` | Build/rebuild index |
| `cgrep watch` | Reindex on file changes |
| `cgrep agent install <provider>` | Install agent instructions |
| `cgrep agent uninstall <provider>` | Uninstall agent instructions |
| `cgrep completions <shell>` | Generate shell completions |

## Search Guide

### Core Options (recommended day-to-day)

```bash
cgrep search "<query>" \
  -p <path> \
  -m <limit> \
  -C <context> \
  -t <language> \
  --glob <pattern> \
  --exclude <pattern> \
  --changed [REV] \
  --budget tight|balanced|full|off \
  --profile human|agent|fast
```

Short examples:

```bash
cgrep search "jwt decode" -m 10
cgrep search "retry backoff" --changed
cgrep search "controller middleware" --budget tight
```

### Modes

```bash
cgrep search "token refresh" --mode keyword   # default
cgrep search "token refresh" --mode semantic  # requires embeddings + index
cgrep search "token refresh" --mode hybrid    # requires embeddings + index
```

Mode notes:
- `keyword`: uses index when present, otherwise scan fallback
- `semantic/hybrid`: require index; no scan fallback

Deprecated compatibility aliases:
- `--keyword`, `--semantic`, `--hybrid` (use `--mode` instead)

### Budget Presets

`--budget` reduces option noise by replacing multiple per-field limits:

| Preset | Intent |
|---|---|
| `tight` | Minimal payload / strict token control |
| `balanced` | Default agent-oriented balance |
| `full` | More context, larger payload |
| `off` | No preset budget limits |

### Profiles

| Profile | Typical use |
|---|---|
| `human` | readable interactive output |
| `agent` | structured + token-efficient defaults |
| `fast` | quick exploratory search |

### Advanced Search Options

```bash
cgrep search --help-advanced
```

Examples of advanced flags:
- `--no-index`, `--fuzzy`
- `--agent-cache`, `--cache-ttl`
- `--context-pack`
- `--max-chars-per-snippet`, `--max-context-chars`, `--max-total-chars`

## Output Formats

Global flags:

```bash
--format text|json|json2
--compact
```

Format summary:
- `text`: human-readable
- `json`: simple array/object payload
- `json2`: structured payload for automation/agents

`search --format json2` shape:
- `meta`: query/mode/timing/truncation/cache metadata
- `results`: deterministic list with stable `id`, path/snippet/lines/scores

## Symbols / References Examples

```bash
# Symbols
cgrep symbols AuthService -T class -t typescript

# References in changed files only
cgrep references validate_token --changed

# Limit reference payload
cgrep references validate_token -m 20
```

## Indexing & Watch

```bash
# Rebuild index
cgrep index --force

# Exclude paths while indexing
cgrep index -e vendor/ -e dist/

# Embeddings mode
cgrep index --embeddings auto
cgrep index --embeddings precompute

# Watch mode
cgrep watch --debounce 2
```

Behavior notes:
- Index lives under `.cgrep/`
- Search from subdirectories reuses nearest parent index
- Indexing ignores `.gitignore`; scan mode respects `.gitignore`

## Agent Integration Install

```bash
cgrep agent install claude-code
cgrep agent install codex
cgrep agent install copilot
cgrep agent install opencode
```

Uninstall:

```bash
cgrep agent uninstall claude-code
cgrep agent uninstall codex
cgrep agent uninstall copilot
cgrep agent uninstall opencode
```

Legacy commands (`install-...`, `uninstall-...`) still work as deprecated compatibility paths.

## Configuration

Config precedence:
1. `<repo>/.cgreprc.toml`
2. `~/.config/cgrep/config.toml`

Example:

```toml
max_results = 20
exclude_patterns = ["target/**", "node_modules/**"]

[search]
default_mode = "keyword"

[cache]
ttl_ms = 600000

[index]
exclude_paths = ["vendor/", "dist/"]

[profile.agent]
format = "json2"
max_results = 50
context = 6
context_pack = 8
mode = "keyword"
agent_cache = true

[embeddings]
provider = "builtin" # builtin|command|dummy
# command = "embedder"
# model = "local-model-id"
```

## Embeddings

Embeddings are optional and used by `--mode semantic|hybrid`.

```bash
cgrep index --embeddings auto
cgrep search "natural language query" --mode hybrid
```

If embeddings DB/provider is unavailable, search falls back to BM25-only with a warning.

## Supported Languages

AST symbol extraction:
- typescript, tsx, javascript, python, rust, go, c, cpp, java, ruby

Index/scan extensions:
- rs, ts, tsx, js, jsx, py, go, java, c, cpp, h, hpp, cs, rb, php, swift
- kt, kts, scala, lua, md, txt, json, yaml, toml

## Troubleshooting

- `semantic/hybrid` returns error or weak results: run `cgrep index` and verify embeddings config.
- Running from subdirectory misses files: set explicit scope with `-p`.
- Too much output for agents: use `--budget tight` or `--profile agent`.
- No index present: keyword mode auto-falls back to scan; semantic/hybrid do not.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```
