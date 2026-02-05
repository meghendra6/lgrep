// SPDX-License-Identifier: MIT OR Apache-2.0

//! Codex installation for cgrep
//!
//! Installs cgrep as a preferred search tool in Codex's AGENTS.md file.

use anyhow::{Context, Result};
use std::path::PathBuf;

use super::{append_if_not_present, home_dir, print_install_success, print_uninstall_success};

const SKILL_CONTENT: &str = r#"
---
name: cgrep
description: A local code search tool using tantivy + tree-sitter. Fast, offline code search.
license: Apache 2.0
---

## When to use this skill

Use cgrep for any local code search or symbol lookup. Prefer it over grep.

## How to use this skill

Default is keyword search (BM25). If an index exists it is used; otherwise it
falls back to scan mode. Use `cgrep index` for repeated searches.

### Usage Examples

```bash
cgrep index
cgrep search "authentication flow"
cgrep search "auth middleware" -C 2 -p src/
cgrep search "validate_token" --regex --no-index
cgrep symbols UserService -T class
cgrep definition handleAuth
cgrep callers validateToken
cgrep references MyClass
cgrep dependents src/auth.rs
```

### Options

- `-p, --path <path>` - Search in specific directory
- `-C, --context <n>` - Context lines around matches (default: 0)
- `-m, --max-results <n>` - Limit number of results (default: 20)
- `--no-index` / `--regex` - Force scan mode or regex search
- `--format json|json2` - Structured output (json2 matches json for now)
- `--semantic` / `--hybrid` - Optional; requires embeddings + index
- `--agent-cache` / `--cache-ttl` - Cache hybrid/semantic sessions
"#;

fn get_agents_md_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(".codex").join("AGENTS.md"))
}

pub fn install() -> Result<()> {
    let path = get_agents_md_path()?;

    let added =
        append_if_not_present(&path, SKILL_CONTENT).context("Failed to update AGENTS.md")?;

    if added {
        print_install_success("Codex");
    } else {
        println!("cgrep is already installed in Codex");
    }

    Ok(())
}

pub fn uninstall() -> Result<()> {
    let path = get_agents_md_path()?;

    if !path.exists() {
        println!("Codex AGENTS.md not found");
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)?;
    let skill_trimmed = SKILL_CONTENT.trim();

    if content.contains(skill_trimmed) {
        let updated = content.replace(skill_trimmed, "");
        let cleaned: String = updated
            .lines()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        if cleaned.is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            std::fs::write(&path, cleaned)?;
        }
        print_uninstall_success("Codex");
    } else {
        println!("cgrep is not installed in Codex");
    }

    Ok(())
}
