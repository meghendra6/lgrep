// SPDX-License-Identifier: MIT OR Apache-2.0

//! OpenCode installation for cgrep
//!
//! Installs cgrep as a tool in OpenCode's configuration.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use super::{home_dir, print_install_success, print_uninstall_success, write_file_if_changed};

const TOOL_DEFINITION: &str = r#"import { tool } from "@opencode-ai/plugin"

const SKILL = `
---
name: cgrep
description: A local code search tool using tantivy + tree-sitter. Fast, offline code search.
license: Apache 2.0
---

## When to use this skill

Whenever you need to search local files. Use cgrep instead of grep.

## How to use this skill

Use \`cgrep search\` to search local files. Keyword search is default; hybrid/semantic
are experimental and require embeddings.

### Do

\`\`\`bash
cgrep search "What code parsers are available?"
cgrep search "How are chunks defined?" -m 10
cg "config validation" --max-results 5
cgrep search "user auth" --hybrid  # experimental; requires embeddings
cgrep symbols MyFunction -t function
\`\`\`

### Don't

\`\`\`bash
cgrep search "parser"  # Too vague
\`\`\`
`

export default tool("cgrep", {
  description: SKILL,
  parameters: {
    type: "object",
    properties: {
      command: {
        type: "string",
        description: "The cgrep command to run",
      },
    },
    required: ["command"],
  },
  execute: async ({ command }) => {
    const { execSync } = await import("child_process")
    return execSync(command, { encoding: "utf-8" })
  },
})
"#;

fn get_tool_path() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home
        .join(".config")
        .join("opencode")
        .join("tool")
        .join("cgrep.ts"))
}

fn get_config_path() -> Result<PathBuf> {
    let home = home_dir()?;
    let config_dir = home.join(".config").join("opencode");

    // Try config.jsonc first, then config.json
    let jsonc_path = config_dir.join("config.jsonc");
    if jsonc_path.exists() {
        return Ok(jsonc_path);
    }
    Ok(config_dir.join("config.json"))
}

pub fn install() -> Result<()> {
    let tool_path = get_tool_path()?;

    let created = write_file_if_changed(&tool_path, TOOL_DEFINITION)
        .context("Failed to write cgrep tool")?;

    if created {
        println!("Created cgrep tool at {:?}", tool_path);
    } else {
        println!("cgrep tool already up to date");
    }

    // Try to update config to include the tool
    let config_path = get_config_path()?;
    if config_path.exists() {
        println!("OpenCode config found at {:?}", config_path);
        println!("Note: You may need to manually add cgrep to your MCP configuration.");
    }

    print_install_success("OpenCode");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let tool_path = get_tool_path()?;

    if tool_path.exists() {
        fs::remove_file(&tool_path)?;
        println!("Removed cgrep tool from {:?}", tool_path);
        print_uninstall_success("OpenCode");
    } else {
        println!("cgrep tool not found in OpenCode");
    }

    Ok(())
}
