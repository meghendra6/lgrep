//! Full-text search with BM25 ranking using tantivy

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::time::Instant;
use tantivy::{
    collector::TopDocs,
    query::QueryParser,
    schema::Value,
    Index, TantivyDocument,
};

use crate::cli::OutputFormat;
use crate::indexer::IndexBuilder;
use lgrep::filters::{matches_file_type, CompiledGlob, matches_glob_compiled, should_exclude_compiled};
use lgrep::output::{use_colors, colorize_path, colorize_line_num, colorize_match, colorize_context};

const INDEX_DIR: &str = ".lgrep";

/// Search result for JSON output
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    pub snippet: String,
    pub line: Option<usize>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// Run the search command
pub fn run(
    query: &str,
    path: Option<&str>,
    max_results: usize,
    context: usize,
    file_type: Option<&str>,
    glob_pattern: Option<&str>,
    exclude_pattern: Option<&str>,
    quiet: bool,
    format: OutputFormat,
) -> Result<()> {
    let start_time = Instant::now();
    let use_color = use_colors() && format == OutputFormat::Text;
    
    // Precompile glob patterns for efficient repeated matching
    let compiled_glob = glob_pattern.and_then(CompiledGlob::new);
    let compiled_exclude = exclude_pattern.and_then(CompiledGlob::new);
    
    let root = path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let index_path = root.join(INDEX_DIR);

    // Check if index exists, create if not
    if !index_path.exists() {
        if use_color {
            println!("{} Index not found, building...", "⚠".yellow());
        } else {
            println!("Index not found, building...");
        }
        let builder = IndexBuilder::new(&root)?;
        builder.build(false)?;
    }

    let index = Index::open_in_dir(&index_path).context("Failed to open index")?;

    let reader = index.reader()?;
    let searcher = reader.searcher();

    let schema = index.schema();
    let content_field = schema.get_field("content").context("Missing content field")?;
    let path_field = schema.get_field("path").context("Missing path field")?;
    let symbols_field = schema.get_field("symbols").context("Missing symbols field")?;

    // Search in both content and symbols
    let query_parser = QueryParser::for_index(&index, vec![content_field, symbols_field]);
    let parsed_query = query_parser.parse_query(query)?;

    // Get more results than needed for filtering
    let fetch_limit = max_results * 5;
    let top_docs = searcher.search(&parsed_query, &TopDocs::with_limit(fetch_limit))?;

    // Track stats
    let mut files_searched: HashSet<String> = HashSet::new();
    let mut total_matches = 0;

    // Collect results with filtering
    let mut results: Vec<SearchResult> = Vec::new();
    for (score, doc_address) in &top_docs {
        if results.len() >= max_results {
            break;
        }
        
        let doc: TantivyDocument = searcher.doc(*doc_address)?;

        let path_value = doc
            .get_first(path_field)
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Apply filters
        if !matches_file_type(path_value, file_type) {
            continue;
        }
        if !matches_glob_compiled(path_value, compiled_glob.as_ref()) {
            continue;
        }
        if should_exclude_compiled(path_value, compiled_exclude.as_ref()) {
            continue;
        }

        files_searched.insert(path_value.to_string());
        total_matches += 1;

        let content_value = doc
            .get_first(content_field)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (snippet, line_num) = find_snippet_with_line(content_value, query, 150);
        
        // Get context lines if requested
        let (context_before, context_after) = if context > 0 && line_num.is_some() {
            get_context_lines(&root.join(path_value), line_num.unwrap(), context)
        } else {
            (vec![], vec![])
        };

        results.push(SearchResult {
            path: path_value.to_string(),
            score: *score,
            snippet,
            line: line_num,
            context_before,
            context_after,
        });
    }

    let elapsed = start_time.elapsed();

    // Output based on format
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        OutputFormat::Text => {
            if results.is_empty() {
                if use_color {
                    println!("{} No results found for: {}", "✗".red(), query.yellow());
                } else {
                    println!("No results found for: {}", query);
                }
            } else {
                if use_color {
                    println!(
                        "\n{} Found {} results for: {}\n",
                        "✓".green(),
                        results.len().to_string().cyan(),
                        query.yellow()
                    );
                } else {
                    println!("\nFound {} results for: {}\n", results.len(), query);
                }

                let mut prev_had_context = false;
                for (idx, result) in results.iter().enumerate() {
                    // Print separator between context groups
                    if idx > 0 && (prev_had_context || !result.context_before.is_empty()) {
                        println!("{}", if use_color { "--".dimmed().to_string() } else { "--".to_string() });
                    }

                    // Print context before
                    for (i, line) in result.context_before.iter().enumerate() {
                        if let Some(match_line) = result.line {
                            let ctx_line_num = match_line.saturating_sub(result.context_before.len() - i);
                            println!(
                                "{}-{}:  {}",
                                colorize_path(&result.path, use_color),
                                colorize_line_num(ctx_line_num, use_color),
                                colorize_context(line, use_color)
                            );
                        }
                    }

                    // Print match line
                    let line_info = result.line
                        .map(|l| format!(":{}", colorize_line_num(l, use_color)))
                        .unwrap_or_default();
                    
                    if use_color {
                        println!(
                            "{}{}  {} (score: {:.2})",
                            colorize_path(&result.path, use_color),
                            line_info,
                            "➜".blue(),
                            result.score
                        );
                    } else {
                        println!(
                            "{}{}  (score: {:.2})",
                            result.path,
                            line_info,
                            result.score
                        );
                    }

                    if !result.snippet.is_empty() {
                        let highlighted = highlight_matches(&result.snippet, query, use_color);
                        for line in highlighted.lines().take(3) {
                            println!("    {}", line);
                        }
                    }

                    // Print context after
                    for (i, line) in result.context_after.iter().enumerate() {
                        if let Some(match_line) = result.line {
                            let ctx_line_num = match_line + i + 1;
                            println!(
                                "{}-{}:  {}",
                                colorize_path(&result.path, use_color),
                                colorize_line_num(ctx_line_num, use_color),
                                colorize_context(line, use_color)
                            );
                        }
                    }

                    prev_had_context = !result.context_after.is_empty();
                    
                    if result.context_before.is_empty() && result.context_after.is_empty() {
                        println!();
                    }
                }
            }

            // Print stats unless quiet
            if !quiet {
                eprintln!(
                    "\n{} files | {} matches | {:.2}ms",
                    files_searched.len(),
                    total_matches,
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }

    Ok(())
}

/// Get context lines around a match
fn get_context_lines(file_path: &std::path::Path, line_num: usize, context: usize) -> (Vec<String>, Vec<String>) {
    let Ok(file) = fs::File::open(file_path) else {
        return (vec![], vec![]);
    };
    
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    
    let start = line_num.saturating_sub(context + 1);
    let end = (line_num + context).min(lines.len());
    
    let before: Vec<String> = if line_num > 1 {
        lines[start..line_num.saturating_sub(1)].to_vec()
    } else {
        vec![]
    };
    
    let after: Vec<String> = if line_num < lines.len() {
        lines[line_num..end].to_vec()
    } else {
        vec![]
    };
    
    (before, after)
}

/// Highlight query matches in text
fn highlight_matches(text: &str, query: &str, use_color: bool) -> String {
    if !use_color {
        return text.to_string();
    }
    
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut result = text.to_string();
    
    for term in terms {
        let re = regex::RegexBuilder::new(&regex::escape(term))
            .case_insensitive(true)
            .build();
        
        if let Ok(re) = re {
            result = re.replace_all(&result, |caps: &regex::Captures| {
                colorize_match(&caps[0], true)
            }).to_string();
        }
    }
    
    result
}

/// Find a relevant snippet containing the query terms, also returning line number
fn find_snippet_with_line(content: &str, query: &str, max_len: usize) -> (String, Option<usize>) {
    let query_lower = query.to_lowercase();
    let terms: Vec<&str> = query_lower.split_whitespace().collect();

    for (line_num, line) in content.lines().enumerate() {
        let line_lower = line.to_lowercase();
        if terms.iter().any(|term| line_lower.contains(term)) {
            let trimmed = line.trim();
            let snippet = if trimmed.len() <= max_len {
                trimmed.to_string()
            } else {
                format!("{}...", &trimmed[..max_len])
            };
            return (snippet, Some(line_num + 1));
        }
    }

    // Return first non-empty line if no match
    let snippet = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| {
            let trimmed = l.trim();
            if trimmed.len() <= max_len {
                trimmed.to_string()
            } else {
                format!("{}...", &trimmed[..max_len])
            }
        })
        .unwrap_or_default();
    
    (snippet, None)
}
