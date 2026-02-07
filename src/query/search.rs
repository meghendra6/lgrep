// SPDX-License-Identifier: MIT OR Apache-2.0

//! Full-text search with BM25 ranking using tantivy

use anyhow::{Context, Result};
use colored::Colorize;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use tantivy::{
    collector::TopDocs,
    query::{BooleanQuery, FuzzyTermQuery, Occur, QueryParser, TermQuery},
    schema::{Term, Value},
    Index, TantivyDocument,
};

use crate::cli::OutputFormat;
use crate::indexer::scanner::FileScanner;
use cgrep::cache::{CacheKey, SearchCache};
use cgrep::config::{Config, EmbeddingProviderType};
use cgrep::embedding::{
    CommandProvider, DummyProvider, EmbeddingProvider, EmbeddingProviderConfig, EmbeddingStorage,
    FastEmbedder, DEFAULT_EMBEDDING_DIM,
};
use cgrep::errors::IndexNotFoundError;
use cgrep::filters::{
    matches_file_type, matches_glob_compiled, should_exclude_compiled, CompiledGlob,
};
use cgrep::hybrid::{
    BM25Result, HybridConfig, HybridResult, HybridSearcher, SearchMode as HybridSearchMode,
};
use cgrep::output::{
    colorize_context, colorize_line_num, colorize_match, colorize_path, print_json, use_colors,
};
use cgrep::utils::INDEX_DIR;
const DEFAULT_CACHE_TTL_MS: u64 = 600_000; // 10 minutes

/// Search result for internal use and text output
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    pub snippet: String,
    pub line: Option<usize>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    /// BM25/text score for hybrid search
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_score: Option<f32>,
    /// Vector/embedding score for hybrid search
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_score: Option<f32>,
    /// Combined hybrid score
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hybrid_score: Option<f32>,
    /// Unique result identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    /// Symbol start line (for semantic/hybrid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_start: Option<u32>,
    /// Symbol end line (for semantic/hybrid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_end: Option<u32>,
}

/// Minimal search result for JSON output
#[derive(Debug, Serialize)]
struct SearchResultJson<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    snippet: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_before: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_after: Option<&'a [String]>,
}

impl<'a> SearchResultJson<'a> {
    fn from_result(result: &'a SearchResult) -> Self {
        Self {
            path: result.path.as_str(),
            line: result.line,
            snippet: result.snippet.as_str(),
            context_before: if result.context_before.is_empty() {
                None
            } else {
                Some(result.context_before.as_slice())
            },
            context_after: if result.context_after.is_empty() {
                None
            } else {
                Some(result.context_after.as_slice())
            },
        }
    }
}

/// Ultra-minimal search result for compact JSON output (AI agent optimized)
#[derive(Debug, Serialize)]
struct SearchResultCompactJson<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    snippet: &'a str,
}

impl<'a> SearchResultCompactJson<'a> {
    fn from_result(result: &'a SearchResult) -> Self {
        Self {
            path: result.path.as_str(),
            line: result.line,
            snippet: result.snippet.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexMode {
    Index,
    Scan,
}

struct SearchOutcome {
    results: Vec<SearchResult>,
    files_with_matches: usize,
    total_matches: usize,
    mode: IndexMode,
}

/// Run the search command
#[allow(clippy::too_many_arguments)]
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
    compact: bool,
    search_mode: Option<HybridSearchMode>,
    _context_pack: Option<usize>,
    use_cache: bool,
    cache_ttl: Option<u64>,
) -> Result<()> {
    let start_time = Instant::now();
    let use_color = use_colors() && format == OutputFormat::Text;

    // Precompile glob patterns for efficient repeated matching
    let compiled_glob = glob_pattern.and_then(CompiledGlob::new);
    let compiled_exclude = exclude_pattern.and_then(CompiledGlob::new);

    let search_root = resolve_search_root(path)?;

    // Find index root (may be in parent directory)
    let (index_root, index_path, using_parent) = match cgrep::utils::find_index_root(&search_root) {
        Some(index_root) => (
            index_root.root.clone(),
            index_root.index_path,
            index_root.is_parent,
        ),
        None => (search_root.clone(), search_root.join(INDEX_DIR), false),
    };

    // Load config relative to the index root so running from subdirectories works.
    let config = Config::load_for_dir(&index_root);
    let effective_max_results = config.merge_max_results(Some(max_results));
    let config_exclude_patterns: Vec<CompiledGlob> = config
        .exclude_patterns
        .iter()
        .filter_map(|p| CompiledGlob::new(p.as_str()))
        .collect();

    if using_parent {
        eprintln!("Using index from: {}", index_root.display());
    }

    let requested_mode = if no_index || regex {
        IndexMode::Scan
    } else {
        IndexMode::Index
    };

    if requested_mode == IndexMode::Scan && fuzzy {
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

    // Check for hybrid search mode
    let effective_search_mode = search_mode.unwrap_or(HybridSearchMode::Keyword);

    let outcome = match effective_search_mode {
        HybridSearchMode::Semantic | HybridSearchMode::Hybrid => {
            // Use hybrid search
            hybrid_search(
                query,
                &index_root,
                &search_root,
                &config,
                effective_max_results,
                context,
                file_type,
                compiled_glob.as_ref(),
                compiled_exclude.as_ref(),
                &config_exclude_patterns,
                effective_search_mode,
                use_cache,
                cache_ttl.unwrap_or(DEFAULT_CACHE_TTL_MS),
            )?
        }
        HybridSearchMode::Keyword => {
            // Standard BM25 or scan search
            if requested_mode == IndexMode::Index && index_path.exists() {
                index_search(
                    query,
                    &index_root,
                    &search_root,
                    effective_max_results,
                    context,
                    file_type,
                    compiled_glob.as_ref(),
                    compiled_exclude.as_ref(),
                    &config_exclude_patterns,
                    fuzzy,
                )?
            } else {
                if requested_mode == IndexMode::Index && !index_path.exists() {
                    eprintln!(
                        "Index not found at {}. Falling back to scan mode.",
                        index_path.display()
                    );
                }
                scan_search(
                    query,
                    &search_root,
                    effective_max_results,
                    context,
                    file_type,
                    compiled_glob.as_ref(),
                    compiled_exclude.as_ref(),
                    &config_exclude_patterns,
                    compiled_regex.as_ref(),
                    case_sensitive,
                )?
            }
        }
    };

    let elapsed = start_time.elapsed();

    // Output based on format
    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
            if compact {
                let json_results: Vec<SearchResultCompactJson<'_>> = outcome
                    .results
                    .iter()
                    .map(SearchResultCompactJson::from_result)
                    .collect();
                print_json(&json_results, compact)?;
            } else {
                let json_results: Vec<SearchResultJson<'_>> = outcome
                    .results
                    .iter()
                    .map(SearchResultJson::from_result)
                    .collect();
                print_json(&json_results, compact)?;
            }
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

                let highlight_snippet = |snippet: &str| {
                    if outcome.mode == IndexMode::Scan {
                        if let Some(re) = compiled_regex.as_ref() {
                            highlight_matches_regex(snippet, re, use_color)
                        } else {
                            highlight_matches(snippet, query, use_color)
                        }
                    } else {
                        highlight_matches(snippet, query, use_color)
                    }
                };

                let format_line_prefix = |marker: &str, line_num: usize, width: usize| {
                    let padded = format!("{:>width$}", line_num, width = width);
                    let num = if use_color {
                        padded.yellow().to_string()
                    } else {
                        padded
                    };
                    let marker = if use_color && marker == ">" {
                        marker.blue().to_string()
                    } else {
                        marker.to_string()
                    };
                    format!("{} {} | ", marker, num)
                };

                let mut prev_had_context = false;
                for (idx, result) in outcome.results.iter().enumerate() {
                    let has_context =
                        !result.context_before.is_empty() || !result.context_after.is_empty();

                    // Print separator between context groups
                    if idx > 0 && (prev_had_context || has_context) {
                        println!(
                            "{}",
                            if use_color {
                                "--".dimmed().to_string()
                            } else {
                                "--".to_string()
                            }
                        );
                    }

                    // Print match header
                    let line_info = result
                        .line
                        .map(|l| format!(":{}", colorize_line_num(l, use_color)))
                        .unwrap_or_default();

                    if use_color {
                        println!("{}{}", colorize_path(&result.path, use_color), line_info);
                    } else {
                        println!("{}{}", result.path, line_info);
                    }

                    if has_context {
                        if let Some(match_line) = result.line {
                            let max_line = match_line + result.context_after.len();
                            let min_line = match_line.saturating_sub(result.context_before.len());
                            let width = std::cmp::max(max_line, min_line).to_string().len();

                            // Print context before
                            for (i, line) in result.context_before.iter().enumerate() {
                                let ctx_line_num =
                                    match_line.saturating_sub(result.context_before.len() - i);
                                let prefix = format_line_prefix(" ", ctx_line_num, width);
                                println!("{}{}", prefix, colorize_context(line, use_color));
                            }

                            // Print match line (single-line snippet)
                            if !result.snippet.is_empty() {
                                let highlighted = highlight_snippet(&result.snippet);
                                let match_text = highlighted.lines().next().unwrap_or("");
                                let prefix = format_line_prefix(">", match_line, width);
                                println!("{}{}", prefix, match_text);
                            }

                            // Print context after
                            for (i, line) in result.context_after.iter().enumerate() {
                                let ctx_line_num = match_line + i + 1;
                                let prefix = format_line_prefix(" ", ctx_line_num, width);
                                println!("{}{}", prefix, colorize_context(line, use_color));
                            }
                        }
                    } else if !result.snippet.is_empty() {
                        let highlighted = highlight_snippet(&result.snippet);
                        for line in highlighted.lines().take(3) {
                            println!("    {}", line);
                        }
                    }

                    prev_had_context = has_context;

                    if !has_context {
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
fn get_context_lines(
    file_path: &std::path::Path,
    line_num: usize,
    context: usize,
) -> (Vec<String>, Vec<String>) {
    let Ok(file) = fs::File::open(file_path) else {
        return (vec![], vec![]);
    };

    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().map_while(|line| line.ok()).collect();

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

fn context_for_line(
    file_path: &Path,
    line_num: Option<usize>,
    context: usize,
) -> (Vec<String>, Vec<String>) {
    if context == 0 {
        return (vec![], vec![]);
    }

    match line_num {
        Some(line) => get_context_lines(file_path, line, context),
        None => (vec![], vec![]),
    }
}

struct IndexCandidate {
    stored_path: String,
    full_path: PathBuf,
    display_path: String,
    score: f32,
    snippet: String,
    line: Option<usize>,
    symbol_id: Option<String>,
    symbol_start: Option<u32>,
    symbol_end: Option<u32>,
}

#[allow(clippy::too_many_arguments)]
fn collect_index_candidates(
    query: &str,
    index_root: &Path,
    search_root: &Path,
    max_candidates: usize,
    doc_type: &str,
    file_type: Option<&str>,
    compiled_glob: Option<&CompiledGlob>,
    compiled_exclude: Option<&CompiledGlob>,
    config_exclude_patterns: &[CompiledGlob],
    fuzzy: bool,
) -> Result<Vec<IndexCandidate>> {
    let index_path = index_root.join(INDEX_DIR);
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
    let content_field = schema
        .get_field("content")
        .context("Missing content field")?;
    let path_field = schema.get_field("path").context("Missing path field")?;
    let symbols_field = schema
        .get_field("symbols")
        .context("Missing symbols field")?;
    let doc_type_field = schema
        .get_field("doc_type")
        .context("Missing doc_type field")?;
    let symbol_id_field = schema
        .get_field("symbol_id")
        .context("Missing symbol_id field")?;
    let symbol_end_line_field = schema
        .get_field("symbol_end_line")
        .context("Missing symbol_end_line field")?;
    let line_offset_field = schema
        .get_field("line_number")
        .context("Missing line_number field")?;

    let text_query: Box<dyn tantivy::query::Query> = if fuzzy {
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

    let doc_type_term = Term::from_field_text(doc_type_field, doc_type);
    let doc_type_query = TermQuery::new(doc_type_term, tantivy::schema::IndexRecordOption::Basic);
    let parsed_query: Box<dyn tantivy::query::Query> = Box::new(BooleanQuery::new(vec![
        (Occur::Must, text_query),
        (Occur::Must, Box::new(doc_type_query)),
    ]));

    let fetch_limit = max_candidates.saturating_mul(5).max(1);
    let top_docs = searcher.search(&parsed_query, &TopDocs::with_limit(fetch_limit))?;

    let mut candidates: Vec<IndexCandidate> = Vec::new();

    for (score, doc_address) in &top_docs {
        if candidates.len() >= max_candidates {
            break;
        }

        let doc: TantivyDocument = searcher.doc(*doc_address)?;
        let path_value = doc
            .get_first(path_field)
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let full_path = resolve_full_path(path_value, index_root);
        let Some(display_path) = scoped_display_path(&full_path, search_root) else {
            continue;
        };

        if !matches_file_type(&display_path, file_type) {
            continue;
        }
        if !matches_glob_compiled(&display_path, compiled_glob) {
            continue;
        }
        if should_exclude_compiled(&display_path, compiled_exclude) {
            continue;
        }
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(&display_path, Some(p)))
        {
            continue;
        }

        let content_value = doc
            .get_first(content_field)
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let line_offset = doc
            .get_first(line_offset_field)
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        let (snippet, line_num) = find_snippet_with_line(content_value, query, 150);
        let doc_type_value = doc
            .get_first(doc_type_field)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut line_num = line_num.map(|l| l + line_offset.saturating_sub(1));
        if line_num.is_none() && doc_type_value == "symbol" {
            line_num = Some(line_offset);
        }

        let symbol_id = if doc_type_value == "symbol" {
            doc.get_first(symbol_id_field)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        } else {
            None
        };

        let symbol_end = if doc_type_value == "symbol" {
            doc.get_first(symbol_end_line_field)
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
        } else {
            None
        };

        candidates.push(IndexCandidate {
            stored_path: path_value.to_string(),
            full_path,
            display_path,
            score: *score,
            snippet,
            line: line_num,
            symbol_id,
            symbol_start: if doc_type_value == "symbol" {
                Some(line_offset as u32)
            } else {
                None
            },
            symbol_end,
        });
    }

    Ok(candidates)
}

#[allow(clippy::too_many_arguments)]
fn index_search(
    query: &str,
    index_root: &Path,
    search_root: &Path,
    max_results: usize,
    context: usize,
    file_type: Option<&str>,
    compiled_glob: Option<&CompiledGlob>,
    compiled_exclude: Option<&CompiledGlob>,
    config_exclude_patterns: &[CompiledGlob],
    fuzzy: bool,
) -> Result<SearchOutcome> {
    let candidates = collect_index_candidates(
        query,
        index_root,
        search_root,
        max_results,
        "file",
        file_type,
        compiled_glob,
        compiled_exclude,
        config_exclude_patterns,
        fuzzy,
    )?;

    let mut files_with_matches: HashSet<String> = HashSet::new();
    let mut results: Vec<SearchResult> = Vec::new();

    for candidate in candidates {
        let (context_before, context_after) =
            context_for_line(&candidate.full_path, candidate.line, context);

        let display_path = candidate.display_path;
        files_with_matches.insert(display_path.clone());

        results.push(SearchResult {
            path: display_path,
            score: candidate.score,
            snippet: candidate.snippet,
            line: candidate.line,
            context_before,
            context_after,
            text_score: None,
            vector_score: None,
            hybrid_score: None,
            result_id: None,
            chunk_start: None,
            chunk_end: None,
        });
    }

    let total_matches = results.len();

    Ok(SearchOutcome {
        results,
        files_with_matches: files_with_matches.len(),
        total_matches,
        mode: IndexMode::Index,
    })
}

#[allow(clippy::too_many_arguments)]
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

    let query_lower = if !case_sensitive {
        query.to_lowercase()
    } else {
        String::new()
    };
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
                    text_score: None,
                    vector_score: None,
                    hybrid_score: None,
                    result_id: None,
                    chunk_start: None,
                    chunk_end: None,
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

                let (context_before, context_after) =
                    get_context_from_lines(&lines, idx + 1, context);

                results.push(SearchResult {
                    path: rel_path.clone(),
                    score: 1.0,
                    snippet,
                    line: Some(idx + 1),
                    context_before,
                    context_after,
                    text_score: None,
                    vector_score: None,
                    hybrid_score: None,
                    result_id: None,
                    chunk_start: None,
                    chunk_end: None,
                });
            }
        }
    }

    Ok(SearchOutcome {
        results,
        files_with_matches: files_with_matches.len(),
        total_matches,
        mode: IndexMode::Scan,
    })
}

/// Hybrid search combining BM25 with vector embeddings
#[allow(clippy::too_many_arguments)]
fn hybrid_search(
    query: &str,
    index_root: &Path,
    search_root: &Path,
    config: &Config,
    max_results: usize,
    context: usize,
    file_type: Option<&str>,
    compiled_glob: Option<&CompiledGlob>,
    compiled_exclude: Option<&CompiledGlob>,
    config_exclude_patterns: &[CompiledGlob],
    mode: HybridSearchMode,
    use_cache: bool,
    cache_ttl_ms: u64,
) -> Result<SearchOutcome> {
    let index_path = index_root.join(INDEX_DIR);
    let embedding_db_path = index_root.join(".cgrep").join("embeddings.sqlite");

    // Build cache key
    let cache_key = CacheKey {
        query: query.to_string(),
        mode: mode.to_string(),
        max_results,
        context,
        file_type: file_type.map(|s| s.to_string()),
        glob: compiled_glob.map(|_| "set".to_string()),
        exclude: compiled_exclude.map(|_| "set".to_string()),
        profile: None,
        index_hash: None,
        embedding_model: None,
        search_root: Some(search_root.to_string_lossy().to_string()),
    };

    // Try cache
    if use_cache {
        if let Ok(cache) = SearchCache::new(index_root, cache_ttl_ms) {
            if let Ok(Some(entry)) = cache.get::<Vec<HybridResult>>(&cache_key) {
                // Return cached results
                let results: Vec<SearchResult> = entry
                    .data
                    .iter()
                    .filter_map(|hr| {
                        let full_path = resolve_full_path(&hr.path, index_root);
                        let display_path = scoped_display_path(&full_path, search_root)?;
                        Some(SearchResult {
                            path: display_path,
                            score: hr.score,
                            snippet: hr.snippet.clone(),
                            line: hr.line,
                            context_before: vec![],
                            context_after: vec![],
                            text_score: Some(hr.text_score),
                            vector_score: Some(hr.vector_score),
                            hybrid_score: Some(hr.score),
                            result_id: hr.result_id.clone(),
                            chunk_start: hr.chunk_start,
                            chunk_end: hr.chunk_end,
                        })
                    })
                    .collect();

                let files_with_matches = results
                    .iter()
                    .map(|r| r.path.clone())
                    .collect::<HashSet<_>>()
                    .len();
                let total_matches = results.len();

                return Ok(SearchOutcome {
                    results,
                    files_with_matches,
                    total_matches,
                    mode: IndexMode::Index,
                });
            }
        }
    }

    // Open embedding storage if available
    let embedding_storage = if embedding_db_path.exists() {
        match EmbeddingStorage::open(&embedding_db_path) {
            Ok(storage) => match storage.is_symbol_unit() {
                Ok(true) => Some(storage),
                Ok(false) => {
                    eprintln!(
                        "Warning: embeddings DB schema mismatch (expected symbol-level). Using BM25 only."
                    );
                    None
                }
                Err(err) => {
                    eprintln!("Warning: failed to read embeddings metadata: {}", err);
                    None
                }
            },
            Err(err) => {
                eprintln!("Warning: failed to open embeddings DB: {}", err);
                None
            }
        }
    } else {
        None
    };

    // Get BM25 results first
    if !index_path.exists() {
        return Err(anyhow::anyhow!(
            "Index required for hybrid search. Run: cgrep index"
        ));
    }

    let bm25_candidates = collect_index_candidates(
        query,
        index_root,
        search_root,
        max_results * 3, // Get more for reranking
        "symbol",
        file_type,
        compiled_glob,
        compiled_exclude,
        config_exclude_patterns,
        false,
    )?;

    // Convert to BM25Result format
    let bm25_results: Vec<BM25Result> = bm25_candidates
        .into_iter()
        .map(|candidate| BM25Result {
            path: candidate.stored_path,
            score: candidate.score,
            snippet: candidate.snippet,
            line: candidate.line,
            chunk_start: candidate
                .symbol_start
                .or_else(|| candidate.line.map(|l| l as u32)),
            chunk_end: candidate
                .symbol_end
                .or_else(|| candidate.line.map(|l| l as u32)),
            symbol_id: candidate.symbol_id,
        })
        .collect();

    // Create hybrid searcher
    let hybrid_config = HybridConfig::default().with_max_results(max_results);
    let hybrid_searcher = HybridSearcher::new(hybrid_config);

    // Perform hybrid search based on mode
    let hybrid_results: Vec<HybridResult> = match mode {
        HybridSearchMode::Semantic | HybridSearchMode::Hybrid => {
            if let Some(ref storage) = embedding_storage {
                let provider_type = config.embeddings.provider();
                let provider_result: Result<Box<dyn EmbeddingProvider>> = match provider_type {
                    EmbeddingProviderType::Builtin => EmbeddingProviderConfig::from_env()
                        .and_then(FastEmbedder::new)
                        .map(|provider| Box::new(provider) as Box<dyn EmbeddingProvider>),
                    EmbeddingProviderType::Dummy => {
                        Ok(Box::new(DummyProvider::new(DEFAULT_EMBEDDING_DIM)))
                    }
                    EmbeddingProviderType::Command => Ok(Box::new(CommandProvider::new(
                        config.embeddings.command().to_string(),
                        config.embeddings.model().to_string(),
                    ))),
                };

                let query_embedding = match provider_result {
                    Ok(mut provider) => match provider.embed_one(query) {
                        Ok(query_embedding) => Some(query_embedding),
                        Err(err) => {
                            eprintln!("Warning: embedding query failed (using BM25 only): {}", err);
                            None
                        }
                    },
                    Err(err) => {
                        eprintln!("Warning: embedding provider unavailable: {}", err);
                        None
                    }
                };

                if let Some(query_embedding) = query_embedding {
                    match mode {
                        HybridSearchMode::Semantic => hybrid_searcher
                            .semantic_search(bm25_results, &query_embedding, storage)
                            .unwrap_or_default(),
                        HybridSearchMode::Hybrid => hybrid_searcher
                            .rerank_with_embeddings(bm25_results, &query_embedding, storage)
                            .unwrap_or_default(),
                        HybridSearchMode::Keyword => Vec::new(),
                    }
                } else {
                    bm25_results
                        .iter()
                        .map(|r| HybridResult {
                            path: r.path.clone(),
                            score: r.score,
                            text_score: r.score,
                            vector_score: 0.0,
                            text_norm: r.score,
                            vector_norm: 0.0,
                            snippet: r.snippet.clone(),
                            line: r.line,
                            chunk_start: r.chunk_start,
                            chunk_end: r.chunk_end,
                            result_id: r.symbol_id.clone(),
                        })
                        .collect()
                }
            } else {
                eprintln!("Warning: No embedding storage found. Using BM25 only.");
                bm25_results
                    .iter()
                    .map(|r| HybridResult {
                        path: r.path.clone(),
                        score: r.score,
                        text_score: r.score,
                        vector_score: 0.0,
                        text_norm: r.score,
                        vector_norm: 0.0,
                        snippet: r.snippet.clone(),
                        line: r.line,
                        chunk_start: r.chunk_start,
                        chunk_end: r.chunk_end,
                        result_id: r.symbol_id.clone(),
                    })
                    .collect()
            }
        }
        HybridSearchMode::Keyword => {
            // Should not reach here
            bm25_results
                .iter()
                .map(|r| HybridResult {
                    path: r.path.clone(),
                    score: r.score,
                    text_score: r.score,
                    vector_score: 0.0,
                    text_norm: r.score,
                    vector_norm: 0.0,
                    snippet: r.snippet.clone(),
                    line: r.line,
                    chunk_start: r.chunk_start,
                    chunk_end: r.chunk_end,
                    result_id: r.symbol_id.clone(),
                })
                .collect()
        }
    };

    // Convert to SearchResult with context
    let mut results: Vec<SearchResult> = Vec::with_capacity(max_results.min(hybrid_results.len()));
    let mut files_with_matches: HashSet<String> = HashSet::new();

    for hr in hybrid_results.iter().take(max_results) {
        let full_path = resolve_full_path(&hr.path, index_root);
        let Some(display_path) = scoped_display_path(&full_path, search_root) else {
            continue;
        };

        // Apply filters
        if !matches_file_type(&display_path, file_type) {
            continue;
        }
        if !matches_glob_compiled(&display_path, compiled_glob) {
            continue;
        }
        if should_exclude_compiled(&display_path, compiled_exclude) {
            continue;
        }
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(&display_path, Some(p)))
        {
            continue;
        }

        files_with_matches.insert(display_path.clone());

        // Get context lines if needed
        let (context_before, context_after) = context_for_line(&full_path, hr.line, context);

        results.push(SearchResult {
            path: display_path,
            score: hr.score,
            snippet: hr.snippet.clone(),
            line: hr.line,
            context_before,
            context_after,
            text_score: Some(hr.text_score),
            vector_score: Some(hr.vector_score),
            hybrid_score: Some(hr.score),
            result_id: hr.result_id.clone(),
            chunk_start: hr.chunk_start,
            chunk_end: hr.chunk_end,
        });
    }

    // Store in cache
    if use_cache {
        if let Ok(cache) = SearchCache::new(index_root, cache_ttl_ms) {
            let to_cache: Vec<HybridResult> =
                hybrid_results.into_iter().take(max_results).collect();
            let _ = cache.put(&cache_key, to_cache);
        }
    }

    let total_matches = results.len();
    let files_count = files_with_matches.len();

    Ok(SearchOutcome {
        results,
        files_with_matches: files_count,
        total_matches,
        mode: IndexMode::Index,
    })
}

fn get_context_from_lines(
    lines: &[&str],
    line_num: usize,
    context: usize,
) -> (Vec<String>, Vec<String>) {
    if lines.is_empty() {
        return (vec![], vec![]);
    }
    let idx = line_num.saturating_sub(1);
    let start = idx.saturating_sub(context);
    let end = (idx + context + 1).min(lines.len());

    let before = lines[start..idx].iter().map(|l| (*l).to_string()).collect();
    let after = if idx + 1 < end {
        lines[idx + 1..end]
            .iter()
            .map(|l| (*l).to_string())
            .collect()
    } else {
        vec![]
    };

    (before, after)
}

fn highlight_matches_regex(text: &str, re: &Regex, use_color: bool) -> String {
    if !use_color {
        return text.to_string();
    }
    re.replace_all(text, |caps: &regex::Captures| {
        colorize_match(&caps[0], true)
    })
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
            result = re
                .replace_all(&result, |caps: &regex::Captures| {
                    colorize_match(&caps[0], true)
                })
                .to_string();
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

fn resolve_search_root(path: Option<&str>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Cannot determine current directory")?;
    let requested = path.map(PathBuf::from).unwrap_or_else(|| cwd.clone());
    let absolute = if requested.is_absolute() {
        requested
    } else {
        cwd.join(requested)
    };
    Ok(normalize_path(&absolute))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                cleaned.push(component.as_os_str());
            }
        }
    }

    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

fn resolve_full_path(path_value: &str, index_root: &Path) -> PathBuf {
    let path = Path::new(path_value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        index_root.join(path)
    }
}

fn scoped_display_path(full_path: &Path, search_root: &Path) -> Option<String> {
    full_path
        .strip_prefix(search_root)
        .ok()
        .map(|rel| rel.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::index::DEFAULT_WRITER_BUDGET_BYTES;
    use crate::indexer::IndexBuilder;
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

    #[test]
    fn index_search_scopes_to_search_root_and_relativizes_paths() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let root_file = root.join("root.rs");
        let subdir = root.join("src");
        let sub_file = subdir.join("sub.rs");

        std::fs::create_dir_all(&subdir).expect("create subdir");
        std::fs::write(&root_file, "needle in root").expect("write root");
        std::fs::write(&sub_file, "needle in sub").expect("write sub");

        let builder = IndexBuilder::new(root).expect("builder");
        builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("build");

        let outcome = index_search("needle", root, &subdir, 10, 0, None, None, None, &[], false)
            .expect("index search");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].path, "sub.rs");
    }
}
