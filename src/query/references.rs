// SPDX-License-Identifier: MIT OR Apache-2.0

//! Find all references to a symbol

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use crate::query::changed_files::ChangedFiles;
use crate::query::index_filter::{find_files_with_content, read_scanned_files};
use cgrep::output::print_json;
use cgrep::utils::get_root_with_index;

/// Reference result for JSON output
#[derive(Debug, Serialize)]
struct ReferenceResult {
    path: String,
    line: usize,
    column: usize,
    code: String,
}

/// Run the references command
pub fn run(
    name: &str,
    path: Option<&str>,
    max_results: usize,
    changed: Option<&str>,
    format: OutputFormat,
    compact: bool,
) -> Result<()> {
    let root = match path {
        Some(p) => get_root_with_index(std::path::PathBuf::from(p).canonicalize()?),
        None => get_root_with_index(std::env::current_dir()?),
    };
    let files = match find_files_with_content(&root, name)? {
        Some(indexed_paths) => read_scanned_files(&indexed_paths),
        None => {
            let scanner = FileScanner::new(&root);
            scanner.scan()?
        }
    };
    let changed_filter = changed
        .map(|rev| ChangedFiles::from_scope(&root, rev))
        .transpose()?;

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
        if let Some(filter) = changed_filter.as_ref() {
            if !filter.matches_rel_path(&rel_path) {
                continue;
            }
        }

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
            print_json(&results, compact)?;
        }
        OutputFormat::Text => {
            if results.is_empty() {
                println!("{} No references found for: {}", "✗".red(), name.yellow());
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
