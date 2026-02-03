// SPDX-License-Identifier: MIT OR Apache-2.0

//! Find all callers of a function

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use cgrep::utils::get_root_with_index;

/// Caller result for JSON output
#[derive(Debug, Serialize)]
struct CallerResult {
    path: String,
    line: usize,
    code: String,
}

/// Run the callers command
pub fn run(function: &str, format: OutputFormat) -> Result<()> {
    let root = get_root_with_index(std::env::current_dir()?);
    let scanner = FileScanner::new(&root);
    let files = scanner.scan()?;

    // Pattern to match function calls
    // Matches: functionName( or object.functionName( or object?.functionName(
    let pattern = format!(r"\b{}\s*\(", regex::escape(function));
    let re = Regex::new(&pattern)?;

    let mut results: Vec<CallerResult> = Vec::new();

    for file in &files {
        let rel_path = file
            .path
            .strip_prefix(&root)
            .unwrap_or(&file.path)
            .display()
            .to_string();

        for (line_num, line) in file.content.lines().enumerate() {
            if re.is_match(line) {
                // Skip definition lines (function declarations)
                let line_lower = line.to_lowercase();
                if line_lower.contains("function ")
                    || line_lower.contains("fn ")
                    || line_lower.contains("def ")
                    || line_lower.contains("func ")
                {
                    continue;
                }

                results.push(CallerResult {
                    path: rel_path.clone(),
                    line: line_num + 1,
                    code: line.trim().to_string(),
                });
            }
        }
    }

    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        OutputFormat::Text => {
            if results.is_empty() {
                println!(
                    "{} No callers found for: {}",
                    "✗".red(),
                    function.yellow()
                );
            } else {
                println!(
                    "\n{} Finding callers of: {}\n",
                    "🔍".cyan(),
                    function.yellow()
                );
                for result in &results {
                    println!(
                        "  {}:{} {}",
                        result.path.cyan(),
                        result.line.to_string().yellow(),
                        result.code.dimmed()
                    );
                }
                println!(
                    "\n{} Found {} call sites",
                    "✓".green(),
                    results.len().to_string().cyan()
                );
            }
        }
    }

    Ok(())
}
