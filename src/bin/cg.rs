// SPDX-License-Identifier: MIT OR Apache-2.0

//! cg - Shorthand command for cgrep
//!
//! This is a convenience wrapper that provides:
//! - `cg <query>` runs `cg search <query>` (direct search)
//! - `cg` with no args launches TUI mode (when implemented)
//! - Default profile is "human" for better interactive experience

use std::env;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    
    // Get path to cgrep binary (same directory as cg)
    let cgrep_path = match env::current_exe() {
        Ok(path) => path.with_file_name("cgrep"),
        Err(_) => "cgrep".into(),
    };
    
    // Build cgrep command
    let mut cmd = Command::new(&cgrep_path);
    
    if args.is_empty() {
        // No args: show help for now (TUI mode TODO)
        cmd.arg("--help");
    } else if !args[0].starts_with('-') && !is_subcommand(&args[0]) {
        // First arg is not a flag or subcommand: treat as search query
        cmd.arg("search").args(&args);
    } else {
        // Pass through to cgrep
        cmd.args(&args);
    }
    
    // Execute cgrep
    match cmd.status() {
        Ok(status) => {
            if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(status.code().unwrap_or(1) as u8)
            }
        }
        Err(e) => {
            eprintln!("Error running cgrep: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Check if the argument is a known cgrep subcommand
fn is_subcommand(arg: &str) -> bool {
    matches!(
        arg.to_lowercase().as_str(),
        "search" | "s" | "symbols" | "definition" | "def" | "callers" | 
        "references" | "refs" | "dependents" | "deps" | "index" | "watch" |
        "install-claude-code" | "uninstall-claude-code" |
        "install-codex" | "uninstall-codex" |
        "install-copilot" | "uninstall-copilot" |
        "install-opencode" | "uninstall-opencode" |
        "completions" | "help"
    )
}
