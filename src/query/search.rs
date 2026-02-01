// SPDX-License-Identifier: MIT OR Apache-2.0

//! Full-text search with BM25 ranking using tantivy

use anyhow::{Context, Result};
use colored::Colorize;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::time::Instant;
use tantivy::{
    collector::TopDocs,
    query::{BooleanQuery, FuzzyTermQuery, Occur, QueryParser},
    schema::{Term, Value},
    Index, TantivyDocument,
};

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use cgrep::config::Config;
use cgrep::errors::IndexNotFoundError;
use cgrep::filters::{matches_file_type, CompiledGlob, matches_glob_compiled, should_exclude_compiled};
use cgrep::output::{use_colors, colorize_path, colorize_line_num, colorize_match, colorize_context};

const INDEX_DIR: &str = ".cgrep";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchMode {
    Index,
    Scan,
}

struct SearchOutcome {
    results: Vec<SearchResult>,
    files_with_matches: usize,
    total_matches: usize,
    mode: SearchMode,
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
    fuzzy: bool,
    no_index: bool,
    regex: bool,
    case_sensitive: bool,
    format: OutputFormat,
) -> Result<()> {
    let start_time = Instant::now();
    let use_color = use_colors() && format == OutputFormat::Text;
    
    // Precompile glob patterns for efficient repeated matching
    let compiled_glob = glob_pattern.and_then(CompiledGlob::new);
    let compiled_exclude = exclude_pattern.and_then(CompiledGlob::new);
    
    // Load config for defaults
    let config = Config::load();
    let effective_max_results = config.merge_max_results(Some(max_results));
    let config_exclude_patterns: Vec<CompiledGlob> = config
        .exclude_patterns
        .iter()
        .filter_map(|p| CompiledGlob::new(p.as_str()))
        .collect();
    
    let root = path
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    let index_path = root.join(INDEX_DIR);

    let requested_mode = if no_index || regex {
        SearchMode::Scan
    } else {
        SearchMode::Index
    };

    if requested_mode == SearchMode::Scan && fuzzy {
        eprintln!("Warning: --fuzzy is only supported with index search; ignoring.");
    }

    let compiled_regex = if regex {
        Some(
            RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .context("Invalid regex pattern")?,
        )
    } else {
        None
    };

    let outcome = if requested_mode == SearchMode::Index && index_path.exists() {
        index_search(
            query,
            &root,
            effective_max_results,
            context,
            file_type,
            compiled_glob.as_ref(),
            compiled_exclude.as_ref(),
            &config_exclude_patterns,
            fuzzy,
        )?
    } else {
        if requested_mode == SearchMode::Index && !index_path.exists() {
            eprintln!(
                "Index not found at {}. Falling back to scan mode.",
                index_path.display()
            );
        }
        scan_search(
            query,
            &root,
            effective_max_results,
            context,
            file_type,
            compiled_glob.as_ref(),
            compiled_exclude.as_ref(),
            &config_exclude_patterns,
            compiled_regex.as_ref(),
            case_sensitive,
        )?
    };

    let elapsed = start_time.elapsed();

    // Output based on format
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&outcome.results)?);
        }
        OutputFormat::Text => {
            if outcome.results.is_empty() {
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
                        outcome.results.len().to_string().cyan(),
                        query.yellow()
                    );
                } else {
                    println!("\nFound {} results for: {}\n", outcome.results.len(), query);
                }

                let mut prev_had_context = false;
                for (idx, result) in outcome.results.iter().enumerate() {
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
                        match outcome.mode {
                            SearchMode::Index => {
                                println!(
                                    "{}{}  {} (score: {:.2})",
                                    colorize_path(&result.path, use_color),
                                    line_info,
                                    "➜".blue(),
                                    result.score
                                );
                            }
                            SearchMode::Scan => {
                                println!(
                                    "{}{}  {} (match)",
                                    colorize_path(&result.path, use_color),
                                    line_info,
                                    "➜".blue()
                                );
                            }
                        }
                    } else {
                        match outcome.mode {
                            SearchMode::Index => {
                                println!(
                                    "{}{}  (score: {:.2})",
                                    result.path,
                                    line_info,
                                    result.score
                                );
                            }
                            SearchMode::Scan => {
                                println!(
                                    "{}{}  (match)",
                                    result.path,
                                    line_info
                                );
                            }
                        }
                    }

                    if !result.snippet.is_empty() {
                        let highlighted = if outcome.mode == SearchMode::Scan {
                            if let Some(re) = compiled_regex.as_ref() {
                                highlight_matches_regex(&result.snippet, re, use_color)
                            } else {
                                highlight_matches(&result.snippet, query, use_color)
                            }
                        } else {
                            highlight_matches(&result.snippet, query, use_color)
                        };
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
                    outcome.files_with_matches,
                    outcome.total_matches,
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

fn index_search(
    query: &str,
    root: &std::path::Path,
    max_results: usize,
    context: usize,
    file_type: Option<&str>,
    compiled_glob: Option<&CompiledGlob>,
    compiled_exclude: Option<&CompiledGlob>,
    config_exclude_patterns: &[CompiledGlob],
    fuzzy: bool,
) -> Result<SearchOutcome> {
    let index_path = root.join(INDEX_DIR);
    if !index_path.exists() {
        return Err(IndexNotFoundError {
            index_path: index_path.display().to_string(),
        }
        .into());
    }

    let index = Index::open_in_dir(&index_path).context("Failed to open index")?;
    let reader = index.reader()?;
    let searcher = reader.searcher();

    let schema = index.schema();
    let content_field = schema.get_field("content").context("Missing content field")?;
    let path_field = schema.get_field("path").context("Missing path field")?;
    let symbols_field = schema.get_field("symbols").context("Missing symbols field")?;

    let parsed_query: Box<dyn tantivy::query::Query> = if fuzzy {
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            anyhow::bail!("Fuzzy search requires at least one search term");
        }
        let mut fuzzy_queries: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        for term in terms {
            let distance = if term.len() <= 4 { 1 } else { 2 };

            let content_term = Term::from_field_text(content_field, term);
            let content_fuzzy = FuzzyTermQuery::new(content_term, distance, true);
            fuzzy_queries.push((Occur::Should, Box::new(content_fuzzy)));

            let symbols_term = Term::from_field_text(symbols_field, term);
            let symbols_fuzzy = FuzzyTermQuery::new(symbols_term, distance, true);
            fuzzy_queries.push((Occur::Should, Box::new(symbols_fuzzy)));
        }

        Box::new(BooleanQuery::new(fuzzy_queries))
    } else {
        let query_parser = QueryParser::for_index(&index, vec![content_field, symbols_field]);
        Box::new(query_parser.parse_query(query)?)
    };

    let fetch_limit = max_results * 5;
    let top_docs = searcher.search(&parsed_query, &TopDocs::with_limit(fetch_limit))?;

    let mut files_with_matches: HashSet<String> = HashSet::new();
    let mut total_matches = 0;
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

        if !matches_file_type(path_value, file_type) {
            continue;
        }
        if !matches_glob_compiled(path_value, compiled_glob) {
            continue;
        }
        if should_exclude_compiled(path_value, compiled_exclude) {
            continue;
        }
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(path_value, Some(p)))
        {
            continue;
        }

        files_with_matches.insert(path_value.to_string());
        total_matches += 1;

        let content_value = doc
            .get_first(content_field)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (snippet, line_num) = find_snippet_with_line(content_value, query, 150);

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

    Ok(SearchOutcome {
        results,
        files_with_matches: files_with_matches.len(),
        total_matches,
        mode: SearchMode::Index,
    })
}

fn scan_search(
    query: &str,
    root: &std::path::Path,
    max_results: usize,
    context: usize,
    file_type: Option<&str>,
    compiled_glob: Option<&CompiledGlob>,
    compiled_exclude: Option<&CompiledGlob>,
    config_exclude_patterns: &[CompiledGlob],
    regex: Option<&Regex>,
    case_sensitive: bool,
) -> Result<SearchOutcome> {
    if regex.is_none() && query.is_empty() {
        anyhow::bail!("Search query cannot be empty");
    }

    let query_lower = if !case_sensitive { query.to_lowercase() } else { String::new() };
    let scanner = FileScanner::new(root);
    let files = scanner.scan()?;

    let mut results: Vec<SearchResult> = Vec::new();
    let mut files_with_matches: HashSet<String> = HashSet::new();
    let mut total_matches = 0;

    'files: for file in files {
        let rel_path = file
            .path
            .strip_prefix(root)
            .unwrap_or(&file.path)
            .display()
            .to_string();

        if !matches_file_type(&rel_path, file_type) {
            continue;
        }
        if !matches_glob_compiled(&rel_path, compiled_glob) {
            continue;
        }
        if should_exclude_compiled(&rel_path, compiled_exclude) {
            continue;
        }
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(&rel_path, Some(p)))
        {
            continue;
        }

        if context == 0 {
            for (idx, line) in file.content.lines().enumerate() {
                if results.len() >= max_results {
                    break 'files;
                }

                let matched = if let Some(re) = regex {
                    re.is_match(line)
                } else if case_sensitive {
                    line.contains(query)
                } else {
                    line.to_lowercase().contains(&query_lower)
                };

                if !matched {
                    continue;
                }

                files_with_matches.insert(rel_path.clone());
                total_matches += 1;

                let trimmed = line.trim();
                let snippet = if trimmed.len() <= 150 {
                    trimmed.to_string()
                } else {
                    format!("{}...", &trimmed[..150])
                };

                results.push(SearchResult {
                    path: rel_path.clone(),
                    score: 1.0,
                    snippet,
                    line: Some(idx + 1),
                    context_before: vec![],
                    context_after: vec![],
                });
            }
        } else {
            let lines: Vec<&str> = file.content.lines().collect();
            for (idx, line) in lines.iter().enumerate() {
                if results.len() >= max_results {
                    break 'files;
                }

                let matched = if let Some(re) = regex {
                    re.is_match(line)
                } else if case_sensitive {
                    line.contains(query)
                } else {
                    line.to_lowercase().contains(&query_lower)
                };

                if !matched {
                    continue;
                }

                files_with_matches.insert(rel_path.clone());
                total_matches += 1;

                let trimmed = line.trim();
                let snippet = if trimmed.len() <= 150 {
                    trimmed.to_string()
                } else {
                    format!("{}...", &trimmed[..150])
                };

                let (context_before, context_after) = get_context_from_lines(&lines, idx + 1, context);

                results.push(SearchResult {
                    path: rel_path.clone(),
                    score: 1.0,
                    snippet,
                    line: Some(idx + 1),
                    context_before,
                    context_after,
                });
            }
        }
    }

    Ok(SearchOutcome {
        results,
        files_with_matches: files_with_matches.len(),
        total_matches,
        mode: SearchMode::Scan,
    })
}

fn get_context_from_lines(lines: &[&str], line_num: usize, context: usize) -> (Vec<String>, Vec<String>) {
    if lines.is_empty() {
        return (vec![], vec![]);
    }
    let idx = line_num.saturating_sub(1);
    let start = idx.saturating_sub(context);
    let end = (idx + context + 1).min(lines.len());

    let before = lines[start..idx].iter().map(|l| (*l).to_string()).collect();
    let after = if idx + 1 < end {
        lines[idx + 1..end].iter().map(|l| (*l).to_string()).collect()
    } else {
        vec![]
    };

    (before, after)
}

fn highlight_matches_regex(text: &str, re: &Regex, use_color: bool) -> String {
    if !use_color {
        return text.to_string();
    }
    re.replace_all(text, |caps: &regex::Captures| colorize_match(&caps[0], true))
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_search_plain_text_case_insensitive() {
        let dir = TempDir::new().expect("tempdir");
        let file_path = dir.path().join("sample.txt");
        std::fs::write(&file_path, "Hello World\nSecond line").expect("write");

        let outcome = scan_search(
            "world",
            dir.path(),
            10,
            0,
            None,
            None,
            None,
            &[],
            None,
            false,
        )
        .expect("scan");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].path, "sample.txt");
        assert_eq!(outcome.results[0].line, Some(1));
    }

    #[test]
    fn scan_search_regex_match() {
        let dir = TempDir::new().expect("tempdir");
        let file_path = dir.path().join("numbers.txt");
        std::fs::write(&file_path, "abc123\nnope\nxyz456").expect("write");

        let re = Regex::new(r"\d{3}").expect("regex");
        let outcome = scan_search(
            r"\d{3}",
            dir.path(),
            10,
            0,
            None,
            None,
            None,
            &[],
            Some(&re),
            true,
        )
        .expect("scan");

        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].path, "numbers.txt");
        assert_eq!(outcome.results[0].line, Some(1));
        assert_eq!(outcome.results[1].line, Some(3));
    }
}
