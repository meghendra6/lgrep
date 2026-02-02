// SPDX-License-Identifier: MIT OR Apache-2.0

//! Find all references to a symbol

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;

/// Reference result for JSON output
#[derive(Debug, Serialize)]
struct ReferenceResult {
    path: String,
    line: usize,
    column: usize,
    code: String,
}

/// Run the references command
pub fn run(name: &str, path: Option<&str>, max_results: usize, format: OutputFormat) -> Result<()> {
    let root = match path {
        Some(p) => std::path::PathBuf::from(p).canonicalize()?,
        None => std::env::current_dir()?,
    };
    let scanner = FileScanner::new(&root);
    let files = scanner.scan()?;

    // Pattern to match symbol with word boundaries
    let pattern = format!(r"\b{}\b", regex::escape(name));
    let re = Regex::new(&pattern)?;

    let mut results: Vec<ReferenceResult> = Vec::new();

    for file in &files {
        let rel_path = file
            .path
            .strip_prefix(&root)
            .unwrap_or(&file.path)
            .display()
            .to_string();

        for (line_num, line) in file.content.lines().enumerate() {
            if let Some(mat) = re.find(line) {
                results.push(ReferenceResult {
                    path: rel_path.clone(),
                    line: line_num + 1,
                    column: mat.start() + 1,
                    code: line.trim().to_string(),
                });

                if results.len() >= max_results {
                    break;
                }
            }
        }

        if results.len() >= max_results {
            break;
        }
    }

    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        OutputFormat::Text => {
            if results.is_empty() {
                println!(
                    "{} No references found for: {}",
                    "✗".red(),
                    name.yellow()
                );
            } else {
                println!(
                    "\n{} Finding references of: {}\n",
                    "🔍".cyan(),
                    name.yellow()
                );
                for result in &results {
                    println!(
                        "  {}:{}:{} {}",
                        result.path.cyan(),
                        result.line.to_string().yellow(),
                        result.column.to_string().dimmed(),
                        result.code.dimmed()
                    );
                }
                println!(
                    "\n{} Found {} references",
                    "✓".green(),
                    results.len().to_string().cyan()
                );
            }
        }
    }

    Ok(())
}
