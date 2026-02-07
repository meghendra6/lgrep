// SPDX-License-Identifier: MIT OR Apache-2.0

//! Find symbol definition location

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use crate::parser::symbols::{SymbolExtractor, SymbolKind};
use crate::query::index_filter::{find_files_with_symbol, read_scanned_files};
use cgrep::output::print_json;
use cgrep::utils::get_root_with_index;

/// Definition result for JSON output
#[derive(Debug, Serialize)]
struct DefinitionResult {
    name: String,
    kind: String,
    path: String,
    line: usize,
    column: usize,
}

/// Run the definition command
pub fn run(name: &str, format: OutputFormat, compact: bool) -> Result<()> {
    let root = get_root_with_index(std::env::current_dir()?);
    let extractor = SymbolExtractor::new();

    let files = match find_files_with_symbol(&root, name)? {
        Some(indexed_paths) => read_scanned_files(&indexed_paths),
        None => {
            let scanner = FileScanner::new(&root);
            scanner.scan()?
        }
    };
    let name_lower = name.to_lowercase();

    // Priority: exact match > contains
    let mut exact_matches = Vec::new();
    let mut partial_matches = Vec::new();

    for file in &files {
        if let Some(ref file_lang) = file.language {
            if let Ok(symbols) = extractor.extract(&file.content, file_lang) {
                for symbol in symbols {
                    // Skip variable/property references, focus on definitions
                    if matches!(
                        symbol.kind,
                        SymbolKind::Function
                            | SymbolKind::Class
                            | SymbolKind::Interface
                            | SymbolKind::Type
                            | SymbolKind::Struct
                            | SymbolKind::Enum
                            | SymbolKind::Trait
                    ) {
                        if symbol.name.to_lowercase() == name_lower {
                            exact_matches.push((file.path.clone(), symbol));
                        } else if symbol.name.to_lowercase().contains(&name_lower) {
                            partial_matches.push((file.path.clone(), symbol));
                        }
                    }
                }
            }
        }
    }

    let matches = if !exact_matches.is_empty() {
        exact_matches
    } else {
        partial_matches
    };

    // Collect results
    let results: Vec<DefinitionResult> = matches
        .iter()
        .map(|(path, symbol)| {
            let rel_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string();
            DefinitionResult {
                name: symbol.name.clone(),
                kind: symbol.kind.to_string(),
                path: rel_path,
                line: symbol.line,
                column: symbol.column,
            }
        })
        .collect();

    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
            print_json(&results, compact)?;
        }
        OutputFormat::Text => {
            if results.is_empty() {
                println!("{} No definition found for: {}", "✗".red(), name.yellow());
                return Ok(());
            }

            println!(
                "\n{} Finding definition of: {}\n",
                "🔍".cyan(),
                name.yellow()
            );

            for (path, symbol) in &matches {
                let rel_path = path.strip_prefix(&root).unwrap_or(path).display();
                let kind_str = format!("[{}]", symbol.kind);

                println!(
                    "  {} {} {}:{}:{}",
                    kind_str.blue(),
                    symbol.name.green(),
                    rel_path.to_string().cyan(),
                    symbol.line.to_string().yellow(),
                    symbol.column.to_string().yellow()
                );

                // Show context from file
                if let Ok(content) = std::fs::read_to_string(path) {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = symbol.line.saturating_sub(1);
                    let end = (start + 3).min(lines.len());

                    println!();
                    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
                        let line_num = format!("{:4}", i + 1);
                        let prefix = if i + 1 == symbol.line {
                            format!("{} ", "➜".green())
                        } else {
                            "  ".to_string()
                        };
                        println!("    {} {} {}", prefix, line_num.dimmed(), line);
                    }
                    println!();
                }
            }
        }
    }

    Ok(())
}
