// SPDX-License-Identifier: MIT OR Apache-2.0

//! GitHub Copilot installation for cgrep
//!
//! Installs cgrep instructions in the project's .github/instructions directory.

use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

use super::{print_install_success, print_uninstall_success, write_file_if_changed};

const LGREP_INSTRUCTIONS: &str = r#"---
name: cgrep Local Code Search
description: Use cgrep for local code search instead of built-in grep/search tools.
applyTo: "**/*"
---

# cgrep Local Code Search Instructions

## CRITICAL: Tool Override

When you need to search for code or files locally, **use cgrep instead of built-in search tools**.

- **AVOID**: Using grep_search for content lookup
- **AVOID**: Multiple grep attempts to find relevant code

- **USE**: `cgrep search "natural language query"` for keyword search (BM25)
- **USE**: `cgrep symbols <name>` for symbol search
- **USE**: `cgrep definition <name>` for finding definitions
- **USE**: `--format json` for JSON output (json2 is reserved and currently the same as json)

## When to Use cgrep

Use cgrep **IMMEDIATELY** when:

- Searching for code patterns, functions, or implementations
- Looking up how something is done in the codebase
- Finding files related to a concept or feature
- User asks "where is...", "how does...", "find..."
- You need context about the codebase structure

## Usage Examples

```bash
# Keyword search (BM25)
cgrep search "authentication flow implementation"
cgrep search "error handling patterns" -p src/

# Hybrid search (experimental; requires embeddings)
cgrep search "user validation" --hybrid

# Symbol search  
cgrep symbols handleAuth -t function
cgrep definition UserService

# Dependency tracking
cgrep callers process_request
cgrep dependents src/auth.rs
```

## Options Reference

| Option | Description |
|--------|-------------|
| `-m, --max-results <n>` | Limit number of results (default: 20) |
| `-C, --context <n>` | Context lines (default: 0) |
| `-p, --path <path>` | Search in specific directory |

## Best Practices

### Do

- Use natural language queries: `cgrep search "How are database connections managed?"`
- Specify subdirectories for focused search: `cgrep search "validation logic" -p src/lib/`

### Don't

- Use single-word queries: `cgrep search "parser"` (too vague)
- Indexing: `cgrep index` gives BM25 results, but keyword search can fall back to scan mode

## Keywords

code search, grep, search, find code, file search, codebase search, hybrid search
"#;

const COPILOT_INSTRUCTIONS_APPEND: &str = r#"

## cgrep Local Code Search

This project uses **cgrep** for local code search. When searching for code or content:

1. Use `cgrep search "natural language query"` instead of grep-based searches
2. For symbol lookup: `cgrep symbols <name>`
3. For definitions: `cgrep definition <name>`
4. Use `--hybrid` for combined keyword + semantic search (experimental; requires embeddings)
5. Use `--format json` for machine-readable output (json2 is reserved)

cgrep uses tantivy + tree-sitter for fast offline code search.
"#;

fn get_project_root() -> Result<PathBuf> {
    env::current_dir().context("Failed to get current directory")
}

pub fn install() -> Result<()> {
    let project_root = get_project_root()?;
    let github_dir = project_root.join(".github");
    let instructions_dir = github_dir.join("instructions");
    let cgrep_instructions_path = instructions_dir.join("cgrep.instructions.md");
    let copilot_instructions_path = github_dir.join("copilot-instructions.md");

    // Create cgrep.instructions.md
    let created = write_file_if_changed(&cgrep_instructions_path, LGREP_INSTRUCTIONS.trim_start())
        .context("Failed to write cgrep instructions")?;

    if created {
        println!("Created cgrep instructions at {:?}", cgrep_instructions_path);
    } else {
        println!("cgrep instructions already up to date");
    }

    // Append to copilot-instructions.md if it exists
    if copilot_instructions_path.exists() {
        let existing = std::fs::read_to_string(&copilot_instructions_path)?;
        if !existing.contains("## cgrep Local Code Search")
            && !existing.contains("## cgrep Local Semantic Search")
            && !existing.contains("cgrep")
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&copilot_instructions_path)?;
            use std::io::Write;
            write!(file, "{}", COPILOT_INSTRUCTIONS_APPEND)?;
            println!("Added cgrep section to {:?}", copilot_instructions_path);
        }
    }

    print_install_success("GitHub Copilot");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let project_root = get_project_root()?;
    let instructions_path = project_root
        .join(".github")
        .join("instructions")
        .join("cgrep.instructions.md");
    let copilot_instructions_path = project_root.join(".github").join("copilot-instructions.md");

    if instructions_path.exists() {
        std::fs::remove_file(&instructions_path)?;
        println!("Removed {:?}", instructions_path);
    } else {
        println!("cgrep instructions file not found");
    }

    if copilot_instructions_path.exists() {
        let content = std::fs::read_to_string(&copilot_instructions_path)?;
        if content.contains(COPILOT_INSTRUCTIONS_APPEND.trim()) {
            let updated = content.replace(COPILOT_INSTRUCTIONS_APPEND, "");
            std::fs::write(&copilot_instructions_path, updated)?;
            println!("Removed cgrep section from {:?}", copilot_instructions_path);
        }
    }

    print_uninstall_success("GitHub Copilot");
    Ok(())
}
