// SPDX-License-Identifier: MIT OR Apache-2.0

//! Agent installation module for cgrep
//!
//! Provides install/uninstall commands for various AI coding agents.

pub mod claude_code;
pub mod codex;
pub mod copilot;
pub mod opencode;

use anyhow::Result;
use std::fs;
use std::path::Path;

/// Helper to write a file only if the content differs from existing
pub fn write_file_if_changed(path: &Path, content: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if existing == content {
            return Ok(false);
        }
    }

    fs::write(path, content)?;
    Ok(true)
}

/// Helper to append content to a file if not already present
pub fn append_if_not_present(path: &Path, content: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let content_trimmed = content.trim();
    if existing.contains(content) || existing.contains(content_trimmed) {
        return Ok(false);
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    use std::io::Write;
    writeln!(file, "{}", content)?;
    Ok(true)
}

/// Get the user's home directory
pub fn home_dir() -> Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))
}

/// Print success message for installation
pub fn print_install_success(agent: &str) {
    println!("✓ Successfully installed cgrep for {}", agent);
    println!();
    println!("  cgrep is a local code search tool (BM25 + AST symbols).");
    println!("  It indexes files locally using tantivy + tree-sitter.");
    println!();
    println!(
        "  To uninstall: cgrep agent uninstall {}",
        agent.to_lowercase().replace(' ', "-")
    );
}

/// Print success message for uninstallation
pub fn print_uninstall_success(agent: &str) {
    println!("✓ Successfully uninstalled cgrep from {}", agent);
}
