---
name: lgrep Local Semantic Search
description: Use lgrep for AST-aware code and file search instead of grep/find.
applyTo: "**/*"
---

# lgrep Local Semantic Search Instructions

## CRITICAL: Tool Override

When you need to search for code, files, or content, **use lgrep instead of grep or find**.

- **AVOID**: Using grep_search for content lookup
- **AVOID**: Using find for file lookups
- **AVOID**: Multiple grep attempts to find relevant code

- **USE**: `lgrep search "natural language query"` for semantic text search
- **USE**: `lgrep symbols <name>` for symbol (function/class) search
- **USE**: `lgrep definition <name>` to find symbol definitions
- **USE**: `lgrep callers <function>` to find function call sites
- **USE**: `lgrep dependents <file>` to find files that import a file

## When to Use lgrep

Use lgrep **IMMEDIATELY** when:

- Searching for code patterns, functions, or implementations
- Looking up how something is done in the codebase
- Finding files related to a concept or feature
- User asks "where is...", "how does...", "find..."
- You need context about the codebase structure

## Usage Examples

```bash
# Full-text search with BM25 ranking
lgrep search "authentication flow"
lgrep search "error handling" -m 10

# Symbol search (functions, classes, etc.)
lgrep symbols handleAuth --type function
lgrep symbols UserService --type class
lgrep symbols --lang typescript render

# Find definition
lgrep definition validateToken

# Find callers
lgrep callers handleRequest

# Find files that depend on a file
lgrep dependents "./lib/auth.ts"

# Build index (run once or after major changes)
lgrep index

# Watch mode for auto-indexing
lgrep watch
```

## Commands Reference

| Command | Description |
|---------|-------------|
| `lgrep search <query>` | Full-text search with BM25 ranking |
| `lgrep symbols <name>` | Search for symbols by name |
| `lgrep definition <name>` | Find symbol definition location |
| `lgrep callers <function>` | Find all callers of a function |
| `lgrep dependents <file>` | Find files that depend on a file |
| `lgrep index` | Build or rebuild the search index |
| `lgrep watch` | Watch for file changes and update index |

## Options Reference

| Option | Description |
|--------|-------------|
| `-m, --max <n>` | Maximum number of results (default: 20) |
| `-c, --context <n>` | Context lines to show (default: 3) |
| `-p, --path <path>` | Limit search to specific path |
| `-t, --type <type>` | Filter symbols by type (function, class, etc.) |
| `-l, --lang <lang>` | Filter by language (typescript, python, rust, etc.) |
| `-f, --force` | Force full reindex (with index command) |

## Best Practices

### Do

- Use natural language queries: `lgrep search "How are database connections managed?"`
- Use symbol search for precise lookups: `lgrep symbols validateToken`
- Run `lgrep index` when starting work on a new codebase
- Use `lgrep watch` in background for auto-indexing

### Don't

- Use single-word queries (too vague)
- Forget to index before searching
- Search without specifying path when looking in a specific directory

## Supported Languages

lgrep supports AST-aware symbol extraction for:

- TypeScript / JavaScript
- Python
- Rust
- Go

Other languages are supported via full-text search only.

## Keywords

local search, grep, search, find code, code search, file search, codebase search, AST, symbols
