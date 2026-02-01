# lgrep

> Local semantic code search tool with AST and BM25 support

**lgrep** is a high-performance, fully local code search tool that combines:
- **tree-sitter** for AST-aware symbol extraction
- **tantivy** for BM25-ranked full-text search
- **ripgrep's ignore crate** for respecting .gitignore

## Features

- **Zero cloud dependencies** - All processing is local, no data leaves your machine
- **Single binary** - No runtime or dependencies required (~5MB)
- **Fast** - ripgrep-level file scanning performance
- **AST-aware** - Understands code structure (functions, classes, etc.)
- **BM25 ranking** - Relevant results, not just pattern matches
- **Multi-language** - TypeScript, Python, Rust, Go, and more

## Installation

### From Source

```bash
cd lgrep
cargo build --release
cp target/release/lgrep ~/.local/bin/
```

### Using Cargo

```bash
cargo install --path .
```

## Quick Start

```bash
# Build the search index (run once)
lgrep index

# Search for code
lgrep search "authentication flow"

# Find a symbol definition
lgrep definition handleAuth

# Find all callers of a function
lgrep callers validateToken

# Search for symbols by type
lgrep symbols UserService --type class
```

## Commands

| Command | Description |
|---------|-------------|
| `lgrep search <query>` | Full-text search with BM25 ranking |
| `lgrep symbols <name>` | Search for symbols by name |
| `lgrep definition <name>` | Find symbol definition location |
| `lgrep callers <function>` | Find all callers of a function |
| `lgrep dependents <file>` | Find files that depend on a file |
| `lgrep index` | Build or rebuild the search index |
| `lgrep watch` | Watch for file changes and update index |

## Examples

### Full-text Search

```bash
$ lgrep search "error handling"

✓ Found 15 results for: error handling

➜ src/lib/auth.ts (score: 8.59)
    throw new Error("Authentication failed");

➜ src/commands/search.ts (score: 7.23)
    } catch (error) {
```

### Symbol Search

```bash
$ lgrep symbols handleAuth --type function

🔍 Searching for symbol: handleAuth

  [function] handleAuth src/lib/auth.ts:45
```

### Find Definition

```bash
$ lgrep definition FileScanner

🔍 Finding definition of: FileScanner

  [struct] FileScanner lgrep/src/indexer/scanner.rs:20:1

  ➜   20 pub struct FileScanner {
      21     root: PathBuf,
      22     extensions: Vec<String>,
```

### Find Callers

```bash
$ lgrep callers validateToken

🔍 Finding callers of: validateToken

  src/api/routes.ts:45 const result = validateToken(token);
  src/middleware/auth.ts:23 if (!validateToken(req.token)) {
```

## Supported Languages

| Language | AST Support | Full-text |
|----------|------------|-----------|
| TypeScript | ✅ | ✅ |
| JavaScript | ✅ | ✅ |
| Python | ✅ | ✅ |
| Rust | ✅ | ✅ |
| Go | ✅ | ✅ |
| Other | ❌ | ✅ |

## Configuration

lgrep stores its index in `.lgrep/` directory in your project root. Add this to your `.gitignore`:

```
.lgrep/
```

## Performance

Compared to traditional tools:

| Metric | grep | ripgrep | lgrep |
|--------|------|---------|-------|
| File scan | 1x | 10-50x | 10-50x |
| Code understanding | ❌ | ❌ | ✅ |
| Ranking | ❌ | ❌ | ✅ (BM25) |
| Symbol search | ❌ | ❌ | ✅ |
| Dependency tracking | ❌ | ❌ | ✅ |

## Copilot Integration

lgrep integrates with GitHub Copilot via instruction files. See `.github/instructions/lgrep.instructions.md` for Copilot configuration.

## License

MIT
