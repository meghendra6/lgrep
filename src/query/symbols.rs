// SPDX-License-Identifier: MIT OR Apache-2.0

//! Symbol search command

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tantivy::{collector::TopDocs, query::QueryParser, schema::Value, Index, ReloadPolicy, TantivyDocument};

use crate::cli::OutputFormat;
use crate::indexer::scanner::{detect_language, FileScanner, ScannedFile};
use crate::parser::symbols::SymbolExtractor;
use cgrep::config::Config;
use cgrep::filters::{matches_file_type, CompiledGlob, matches_glob_compiled, should_exclude_compiled};
use cgrep::output::{use_colors, colorize_path, colorize_line_num, colorize_kind, colorize_name};
use cgrep::utils::{get_root_with_index, INDEX_DIR};

/// Symbol result for JSON output
#[derive(Debug, Serialize)]
struct SymbolResult {
    name: String,
    kind: String,
    path: String,
    line: usize,
}

/// Search the tantivy index for files containing a symbol name.
/// Returns None if no index exists, falling back to full scan.
fn find_files_with_symbol(root: &Path, symbol_name: &str) -> Result<Option<Vec<PathBuf>>> {
    let index_path = root.join(INDEX_DIR);
    if !index_path.exists() {
        return Ok(None); // No index, fall back to scan
    }

    let index = match Index::open_in_dir(&index_path) {
        Ok(idx) => idx,
        Err(_) => return Ok(None),
    };

    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .context("Failed to create index reader")?;

    let searcher = reader.searcher();
    let schema = index.schema();

    // Get the symbols and path fields
    let symbols_field = schema.get_field("symbols").ok();
    let path_field = schema.get_field("path").ok();

    let (symbols_field, path_field) = match (symbols_field, path_field) {
        (Some(s), Some(p)) => (s, p),
        _ => return Ok(None),
    };

    // Build a query for the symbol name (case-insensitive via tantivy tokenization)
    let query_parser = QueryParser::for_index(&index, vec![symbols_field]);
    let query = match query_parser.parse_query(&symbol_name.to_lowercase()) {
        Ok(q) => q,
        Err(_) => return Ok(None),
    };

    // Search for matching documents - get up to 10000 files
    let top_docs = searcher.search(&query, &TopDocs::with_limit(10000))?;

    let mut file_paths = Vec::with_capacity(top_docs.len());
    for (_score, doc_address) in top_docs {
        if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
            if let Some(path_value) = doc.get_first(path_field) {
                if let Some(path_str) = path_value.as_str() {
                    file_paths.push(root.join(path_str));
                }
            }
        }
    }

    Ok(Some(file_paths))
}

/// Run the symbols command
pub fn run(
    name: &str,
    symbol_type: Option<&str>,
    lang: Option<&str>,
    file_type: Option<&str>,
    glob_pattern: Option<&str>,
    exclude_pattern: Option<&str>,
    quiet: bool,
    format: OutputFormat,
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
        Some(indexed_paths) if !indexed_paths.is_empty() => {
            // Only read the files that the index tells us contain the symbol
            let mut scanned = Vec::with_capacity(indexed_paths.len());
            for path in indexed_paths {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let language = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .and_then(detect_language);
                    scanned.push(ScannedFile {
                        path: path.clone(),
                        content,
                        language,
                    });
                }
            }
            scanned
        }
        _ => {
            // Fall back to full scan if no index
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
        if config_exclude_patterns.iter().any(|p| should_exclude_compiled(&rel_path, Some(p))) {
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
            println!("{}", serde_json::to_string_pretty(&results)?);
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
                    println!("\n{} Found {} symbols", "✓".green(), results.len().to_string().cyan());
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
