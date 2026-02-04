// SPDX-License-Identifier: MIT OR Apache-2.0

//! Claude Code installation for cgrep
//!
//! Installs cgrep as a preferred search tool in Claude Code's CLAUDE.md file.

use anyhow::{Context, Result};
use std::path::PathBuf;

use super::{append_if_not_present, home_dir, print_install_success, print_uninstall_success};

const SKILL_CONTENT: &str = r#"
## cgrep Local Code Search

Use `cgrep` for fast local code search (BM25 keyword search + AST symbols).
Hybrid/semantic modes are experimental and require embeddings.

### When to use cgrep

- Searching for code patterns, functions, or implementations
- Finding files related to a concept or feature
- Natural language queries about the codebase

### Usage

```bash
cgrep search "authentication flow"       # BM25 keyword search
cgrep search "error handling" -m 10      # limit results
cgrep search "user auth" --hybrid        # experimental (requires embeddings)
cgrep symbols MyClass                     # find symbol definitions
cgrep definition handleClick              # find function definition
```

### Prefer cgrep over

- Built-in grep for conceptual searches (hybrid/semantic when available)
- Multiple grep attempts to find code
- Pattern-based searches when intent matters
"#;

fn get_claude_md_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(".claude").join("CLAUDE.md"))
}

pub fn install() -> Result<()> {
    let path = get_claude_md_path()?;

    let added =
        append_if_not_present(&path, SKILL_CONTENT).context("Failed to update CLAUDE.md")?;

    if added {
        print_install_success("Claude Code");
    } else {
        println!("cgrep is already installed in Claude Code");
    }

    Ok(())
}

pub fn uninstall() -> Result<()> {
    let path = get_claude_md_path()?;

    if !path.exists() {
        println!("Claude Code CLAUDE.md not found");
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)?;
    let skill_trimmed = SKILL_CONTENT.trim();

    if content.contains(skill_trimmed) {
        let updated = content.replace(skill_trimmed, "");
        // Clean up extra blank lines
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
        print_uninstall_success("Claude Code");
    } else {
        println!("cgrep is not installed in Claude Code");
    }

    Ok(())
}
