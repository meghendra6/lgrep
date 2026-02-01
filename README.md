# cgrep

> Local semantic code search tool with AST and BM25 support

**cgrep** is a high-performance, fully local code search tool that combines:
- **tree-sitter** for AST-aware symbol extraction
- **tantivy** for BM25-ranked full-text search
- **ripgrep's ignore crate** for respecting .gitignore

## Features

- **Zero cloud dependencies** - All processing is local, no data leaves your machine
- **Single binary** - No runtime or dependencies required (~5MB)
- **Fast** - ripgrep-level file scanning performance
- **AST-aware** - Understands code structure (functions, classes, etc.)
- **BM25 ranking** - Relevant results, not just pattern matches
- **Incremental indexing** - Hash-based change detection with symbol cache; binary files skipped
- **Multi-language** - TypeScript, JavaScript, Python, Rust, Go, C, C++, Java, Ruby
- **Shell completions** - Tab completion for Bash, Zsh, Fish, PowerShell
- **Agent integrations** - Works with Claude Code, Codex, Copilot, OpenCode

## Installation

### From Source

```bash
cd cgrep
cargo build --release
cp target/release/cgrep ~/.local/bin/
```

### Using Cargo

```bash
cargo install --path .
```

## Quick Start

```bash
# Build the search index (run once)
cgrep index

# Search for code
cgrep search "authentication flow"

# Find a symbol definition
cgrep definition handleAuth

# Find all callers of a function
cgrep callers validateToken

# Find all references to a symbol
cgrep references MyClass

# Search for symbols by type
cgrep symbols UserService --type class
```

## Commands

| Command | Description |
|---------|-------------|
| `cgrep search <query>` | Full-text search with BM25 ranking |
| `cgrep symbols <name>` | Search for symbols by name |
| `cgrep definition <name>` | Find symbol definition location |
| `cgrep callers <function>` | Find all callers of a function |
| `cgrep references <name>` | Find all references to a symbol |
| `cgrep dependents <file>` | Find files that depend on a file |
| `cgrep index` | Build or rebuild the search index |
| `cgrep watch` | Watch for file changes and update index |
| `cgrep completions <shell>` | Generate shell completions |

## Search Command Flags

```bash
cgrep search <query> [options]
```

| Flag | Description |
|------|-------------|
| `-p, --path <path>` | Path to search in (defaults to current directory) |
| `-m, --max-results <n>` | Maximum number of results (default: 20) |
| `-C, --context <n>` | Show N lines before and after each match |
| `-t, --type <type>` | Filter by file type/language (e.g., rust, ts, python) |
| `-g, --glob <pattern>` | Filter files matching glob pattern (e.g., "*.rs") |
| `--exclude <pattern>` | Exclude files matching pattern |
| `-q, --quiet` | Suppress statistics output |
| `-f, --fuzzy` | Enable fuzzy matching (allows 1-2 character differences) |
| `--no-index` | Scan files directly without using the index |
| `--regex` | Treat query as a regular expression (scan mode) |
| `--case-sensitive` | Case-sensitive search (scan mode) |
| `--format <text\|json>` | Output format |

If the index does not exist, `cgrep search` automatically falls back to scan mode.

## Symbols Command Flags

```bash
cgrep symbols <name> [options]
```

| Flag | Description |
|------|-------------|
| `-T, --type <type>` | Filter by symbol type (function, class, variable, etc.) |
| `-l, --lang <lang>` | Filter by language (typescript, python, rust, etc.) |
| `-t, --file-type <type>` | Filter by file type |
| `-g, --glob <pattern>` | Filter files matching glob pattern |
| `--exclude <pattern>` | Exclude files matching pattern |
| `-q, --quiet` | Suppress statistics output |

## References Command Flags

```bash
cgrep references <name> [options]
```

| Flag | Description |
|------|-------------|
| `-p, --path <path>` | Path to search in (defaults to current directory) |
| `-m, --max-results <n>` | Maximum number of results (default: 50) |
| `--format <text\|json>` | Output format |

## Configuration

### Config File

cgrep supports configuration via `.cgreprc.toml` in your project directory or `~/.config/cgrep/config.toml` for global settings:

```toml
# .cgreprc.toml
max_results = 20
default_format = "text"  # or "json"
```

### Index Location

cgrep stores its index in `.cgrep/` directory in your project root. Add this to your `.gitignore`:

```
.cgrep/
```

Indexing uses file hashes to detect real content changes, caches extracted symbols, and skips binary files by content while still indexing large text/code files.

## Shell Completions

Generate shell completions for your preferred shell:

```bash
# Bash
cgrep completions bash > ~/.local/share/bash-completion/completions/cgrep

# Zsh
cgrep completions zsh > ~/.zfunc/_cgrep

# Fish
cgrep completions fish > ~/.config/fish/completions/cgrep.fish

# PowerShell
cgrep completions powershell > $PROFILE.CurrentUserAllHosts
```

## Agent Integrations

cgrep integrates with AI coding agents for enhanced code understanding:

### Claude Code

```bash
cgrep install-claude-code    # Install integration
cgrep uninstall-claude-code  # Uninstall
```

### OpenAI Codex

```bash
cgrep install-codex    # Install integration
cgrep uninstall-codex  # Uninstall
```

### GitHub Copilot

```bash
cgrep install-copilot    # Install integration
cgrep uninstall-copilot  # Uninstall
```

### OpenCode

```bash
cgrep install-opencode    # Install integration
cgrep uninstall-opencode  # Uninstall
```

## Supported Languages

| Language | File Extensions | AST Support | Full-text |
|----------|----------------|-------------|-----------|
| TypeScript | .ts, .tsx | ✅ | ✅ |
| JavaScript | .js, .jsx | ✅ | ✅ |
| Python | .py | ✅ | ✅ |
| Rust | .rs | ✅ | ✅ |
| Go | .go | ✅ | ✅ |
| C | .c, .h | ✅ | ✅ |
| C++ | .cpp, .cc, .hpp | ✅ | ✅ |
| Java | .java | ✅ | ✅ |
| Ruby | .rb | ✅ | ✅ |
| Other | * | ❌ | ✅ |

## Examples

### Full-text Search

```bash
$ cgrep search "error handling"

✓ Found 15 results for: error handling

➜ src/lib/auth.ts (score: 8.59)
    throw new Error("Authentication failed");

➜ src/commands/search.ts (score: 7.23)
    } catch (error) {
```

### Full-text Search with Context

```bash
$ cgrep search "auth middleware" -C 2 -t typescript

✓ Found 5 results for: auth middleware

➜ src/middleware/auth.ts (score: 9.12)
    // Previous line
    export const authMiddleware = async (req, res, next) => {
    // Next line
```

### Fuzzy Search

```bash
$ cgrep search "authentcation" --fuzzy  # Note typo

✓ Found 12 results (fuzzy matching)
```

### Symbol Search

```bash
$ cgrep symbols handleAuth --type function

🔍 Searching for symbol: handleAuth

  [function] handleAuth src/lib/auth.ts:45
```

### Find Definition

```bash
$ cgrep definition FileScanner

🔍 Finding definition of: FileScanner

  [struct] FileScanner cgrep/src/indexer/scanner.rs:20:1

  ➜   20 pub struct FileScanner {
      21     root: PathBuf,
      22     extensions: Vec<String>,
```

### Find Callers

```bash
$ cgrep callers validateToken

🔍 Finding callers of: validateToken

  src/api/routes.ts:45 const result = validateToken(token);
  src/middleware/auth.ts:23 if (!validateToken(req.token)) {
```

### Find References

```bash
$ cgrep references UserService

🔍 Finding references of: UserService

  src/services/user.ts:5:14 export class UserService {
  src/api/routes.ts:12:22 const service = new UserService();
  src/tests/user.test.ts:8:10 describe('UserService', () => {

✓ Found 3 references
```

### JSON Output

```bash
$ cgrep search "config" --format json
[
  {
    "path": "src/config.ts",
    "line": 10,
    "score": 8.5,
    "content": "export const config = { ... }"
  }
]
```

## Performance

Compared to traditional tools:

| Metric | grep | ripgrep | cgrep |
|--------|------|---------|-------|
| File scan | 1x | 10-50x | 10-50x |
| Code understanding | ❌ | ❌ | ✅ |
| Ranking | ❌ | ❌ | ✅ (BM25) |
| Symbol search | ❌ | ❌ | ✅ |
| Dependency tracking | ❌ | ❌ | ✅ |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CGREP_LOG` | Set log level (e.g., `debug`, `info`, `warn`) |
| `NO_COLOR` | Disable colored output |

## License

Dual-licensed under the MIT and Apache License, Version 2.0.

SPDX-License-Identifier: MIT OR Apache-2.0
