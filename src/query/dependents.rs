// SPDX-License-Identifier: MIT OR Apache-2.0

//! Find files that depend on a given file

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use serde::Serialize;
use std::path::Path;

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use cgrep::utils::get_root_with_index;

/// Dependent result for JSON output
#[derive(Debug, Serialize)]
struct DependentResult {
    path: String,
    line: usize,
    import_line: String,
}

/// Run the dependents command
pub fn run(file: &str, format: OutputFormat) -> Result<()> {
    let root = get_root_with_index(std::env::current_dir()?);
    let scanner = FileScanner::new(&root);
    let files = scanner.scan()?;

    let target_path = Path::new(file);
    let target_stem = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file);

    // Patterns to match imports
    let patterns = vec![
        // JavaScript/TypeScript: import ... from 'path' or require('path')
        format!(r#"(?:import|from|require)\s*[\(\s]?['"](?:[./]*{})['"]"#, regex::escape(target_stem)),
        // Python: import path or from path import
        format!(r"(?:import|from)\s+{}(?:\s|$|,)", regex::escape(target_stem)),
        // Rust: use path or mod path
        format!(r"(?:use|mod)\s+(?:crate::)?{}(?:::|;|\s)", regex::escape(target_stem)),
        // Go: import "path"
        format!(r#"import\s+[\(\s]*['"](?:[./]*{})['"]"#, regex::escape(target_stem)),
    ];

    let regexes: Vec<Regex> = patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

    let mut results: Vec<DependentResult> = Vec::new();

    for scanned_file in &files {
        let rel_path = scanned_file
            .path
            .strip_prefix(&root)
            .unwrap_or(&scanned_file.path);

        // Skip the target file itself
        if rel_path.to_string_lossy().contains(target_stem) {
            continue;
        }

        for (line_num, line) in scanned_file.content.lines().enumerate() {
            for re in &regexes {
                if re.is_match(line) {
                    results.push(DependentResult {
                        path: rel_path.display().to_string(),
                        line: line_num + 1,
                        import_line: line.trim().to_string(),
                    });
                    break;
                }
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
                    "{} No files depend on: {}",
                    "✗".red(),
                    file.yellow()
                );
            } else {
                println!(
                    "\n{} Finding files that depend on: {}\n",
                    "🔍".cyan(),
                    file.yellow()
                );
                for result in &results {
                    println!(
                        "  {}:{} {}",
                        result.path.cyan(),
                        result.line.to_string().yellow(),
                        result.import_line.dimmed()
                    );
                }
                println!(
                    "\n{} Found {} dependent files",
                    "✓".green(),
                    results.len().to_string().cyan()
                );
            }
        }
    }

    Ok(())
}
