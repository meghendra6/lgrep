// SPDX-License-Identifier: MIT OR Apache-2.0

//! Symbol search command

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::collections::HashSet;
use std::time::Instant;

use crate::cli::OutputFormat;
use crate::indexer::scanner::{FileScanner, ScannedFile};
use crate::parser::symbols::SymbolExtractor;
use crate::query::index_filter::{find_files_with_symbol, read_scanned_files};
use cgrep::config::Config;
use cgrep::filters::{
    matches_file_type, matches_glob_compiled, should_exclude_compiled, CompiledGlob,
};
use cgrep::output::{
    colorize_kind, colorize_line_num, colorize_name, colorize_path, print_json, use_colors,
};
use cgrep::utils::get_root_with_index;

/// Symbol result for JSON output
#[derive(Debug, Serialize)]
struct SymbolResult {
    name: String,
    kind: String,
    path: String,
    line: usize,
}

/// Run the symbols command
#[allow(clippy::too_many_arguments)]
pub fn run(
    name: &str,
    symbol_type: Option<&str>,
    lang: Option<&str>,
    file_type: Option<&str>,
    glob_pattern: Option<&str>,
    exclude_pattern: Option<&str>,
    quiet: bool,
    format: OutputFormat,
    compact: bool,
) -> Result<()> {
    let start_time = Instant::now();
    let use_color = use_colors() && format == OutputFormat::Text;

    // Load config for exclude patterns
    let config = Config::load();

    // Precompile glob patterns for efficient repeated matching
    let compiled_glob = glob_pattern.and_then(CompiledGlob::new);
    let compiled_exclude = exclude_pattern.and_then(CompiledGlob::new);

    // Compile config exclude patterns
    let config_exclude_patterns: Vec<CompiledGlob> = config
        .exclude_patterns
        .iter()
        .filter_map(|p| CompiledGlob::new(p.as_str()))
        .collect();

    let root = get_root_with_index(std::env::current_dir()?);
    let extractor = SymbolExtractor::new();
    let name_lower = name.to_lowercase();

    // Try to use index for fast file filtering first
    let files: Vec<ScannedFile> = match find_files_with_symbol(&root, name)? {
        Some(indexed_paths) => read_scanned_files(&indexed_paths),
        None => {
            let scanner = FileScanner::new(&root);
            scanner.scan()?
        }
    };

    let mut results: Vec<SymbolResult> = Vec::new();
    let mut files_searched: HashSet<String> = HashSet::new();

    for file in files {
        let rel_path = file
            .path
            .strip_prefix(&root)
            .unwrap_or(&file.path)
            .display()
            .to_string();

        // Apply path filters
        if !matches_file_type(&rel_path, file_type) {
            continue;
        }
        if !matches_glob_compiled(&rel_path, compiled_glob.as_ref()) {
            continue;
        }
        if should_exclude_compiled(&rel_path, compiled_exclude.as_ref()) {
            continue;
        }
        // Also check config exclude patterns
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(&rel_path, Some(p)))
        {
            continue;
        }

        // Filter by language if specified
        if let Some(filter_lang) = lang {
            if file.language.as_deref() != Some(filter_lang) {
                continue;
            }
        }

        files_searched.insert(rel_path.clone());

        if let Some(ref file_lang) = file.language {
            if let Ok(symbols) = extractor.extract(&file.content, file_lang) {
                for symbol in symbols {
                    // Filter by name
                    if !symbol.name.to_lowercase().contains(&name_lower) {
                        continue;
                    }

                    // Filter by type if specified
                    if let Some(filter_type) = symbol_type {
                        if symbol.kind.to_string() != filter_type.to_lowercase() {
                            continue;
                        }
                    }

                    results.push(SymbolResult {
                        name: symbol.name.clone(),
                        kind: symbol.kind.to_string(),
                        path: rel_path.clone(),
                        line: symbol.line,
                    });
                }
            }
        }
    }

    let elapsed = start_time.elapsed();

    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
            print_json(&results, compact)?;
        }
        OutputFormat::Text => {
            if results.is_empty() {
                if use_color {
                    println!("{} No symbols found matching: {}", "✗".red(), name.yellow());
                } else {
                    println!("No symbols found matching: {}", name);
                }
            } else {
                if use_color {
                    println!(
                        "\n{} Searching for symbol: {}\n",
                        "🔍".cyan(),
                        name.yellow()
                    );
                } else {
                    println!("\nSearching for symbol: {}\n", name);
                }

                for result in &results {
                    let kind_str = format!("[{}]", result.kind);
                    println!(
                        "  {} {} {}:{}",
                        colorize_kind(&kind_str, use_color),
                        colorize_name(&result.name, use_color),
                        colorize_path(&result.path, use_color),
                        colorize_line_num(result.line, use_color)
                    );
                }

                if use_color {
                    println!(
                        "\n{} Found {} symbols",
                        "✓".green(),
                        results.len().to_string().cyan()
                    );
                } else {
                    println!("\nFound {} symbols", results.len());
                }
            }

            // Print stats unless quiet
            if !quiet {
                eprintln!(
                    "\n{} files | {} symbols | {:.2}ms",
                    files_searched.len(),
                    results.len(),
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }

    Ok(())
}
