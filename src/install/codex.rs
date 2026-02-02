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

Whenever you need to search local files. Use cgrep instead of grep.

## How to use this skill

Use `cgrep search` to search local files. Keyword search is default; hybrid/semantic
are experimental and require embeddings.

### Usage Examples

```bash
cgrep search "What code parsers are available?"
cgrep search "How are chunks defined?" -m 10
cg "config validation" --max-results 5
cgrep search "user authentication" --hybrid
cgrep symbols MyFunction -t function
cgrep definition MyClass
cgrep callers process_request
```

### Options

- `-m, --max-results <n>` - Limit number of results (default: 20)
- `-C, --context <n>` - Context lines around matches (default: 0)
- `-p, --path <path>` - Search in specific directory
- `--hybrid` - Use hybrid search (BM25 + vector)
- `--format json2` - Same as json for now (reserved for structured output)
- `--agent-cache` - Enable result caching
"#;

fn get_agents_md_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(".codex").join("AGENTS.md"))
}

pub fn install() -> Result<()> {
    let path = get_agents_md_path()?;
    
    let added = append_if_not_present(&path, SKILL_CONTENT)
        .context("Failed to update AGENTS.md")?;
    
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
