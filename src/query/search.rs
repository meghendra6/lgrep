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
use cgrep::cache::{CacheKey, SearchCache};
use cgrep::config::{Config, EmbeddingProviderType};
use cgrep::embedding::provider::create_provider;
use cgrep::embedding::storage::EmbeddingStorage;
use cgrep::embedding::EmbeddingProviderConfig;
use cgrep::errors::IndexNotFoundError;
use cgrep::filters::{
    matches_file_type, matches_glob_compiled, should_exclude_compiled, CompiledGlob,
};
use cgrep::hybrid::{
    BM25Result, HybridConfig, HybridResult, HybridSearcher, SearchMode as HybridSearchMode,
};
use cgrep::output::{
    colorize_context, colorize_line_num, colorize_match, colorize_path, use_colors,
};
use cgrep::utils::INDEX_DIR;
const DEFAULT_CACHE_TTL_MS: u64 = 600_000; // 10 minutes

/// Search result for JSON output
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
    /// Chunk start line (for semantic/hybrid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_start: Option<u32>,
    /// Chunk end line (for semantic/hybrid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_end: Option<u32>,
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

    let requested_root = path
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    // Find index root (may be in parent directory)
    let (root, index_path, using_parent) = match cgrep::utils::find_index_root(&requested_root) {
        Some(index_root) => (
            index_root.root.clone(),
            index_root.index_path,
            index_root.is_parent,
        ),
        None => (
            requested_root.clone(),
            requested_root.join(INDEX_DIR),
            false,
        ),
    };

    // Load config relative to the index root so running from subdirectories works.
    let config = Config::load_for_dir(&root);
    let effective_max_results = config.merge_max_results(Some(max_results));
    let config_exclude_patterns: Vec<CompiledGlob> = config
        .exclude_patterns
        .iter()
        .filter_map(|p| CompiledGlob::new(p.as_str()))
        .collect();

    if using_parent {
        eprintln!("Using index from: {}", root.display());
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
                &root,
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
                if requested_mode == IndexMode::Index && !index_path.exists() {
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
            }
        }
    };

    let elapsed = start_time.elapsed();

    // Output based on format
    match format {
        OutputFormat::Json | OutputFormat::Json2 => {
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
                        match outcome.mode {
                            IndexMode::Index => {
                                println!(
                                    "{}{}  {} (score: {:.2})",
                                    colorize_path(&result.path, use_color),
                                    line_info,
                                    "➜".blue(),
                                    result.score
                                );
                            }
                            IndexMode::Scan => {
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
                            IndexMode::Index => {
                                println!(
                                    "{}{}  (score: {:.2})",
                                    result.path, line_info, result.score
                                );
                            }
                            IndexMode::Scan => {
                                println!("{}{}  (match)", result.path, line_info);
                            }
                        }
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
    let content_field = schema
        .get_field("content")
        .context("Missing content field")?;
    let path_field = schema.get_field("path").context("Missing path field")?;
    let symbols_field = schema
        .get_field("symbols")
        .context("Missing symbols field")?;
    let line_offset_field = schema
        .get_field("line_number")
        .context("Missing line_number field")?;

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

        let line_offset = doc
            .get_first(line_offset_field)
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        let (snippet, line_num) = find_snippet_with_line(content_value, query, 150);
        let line_num = line_num.map(|l| l + line_offset.saturating_sub(1));

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
            text_score: None,
            vector_score: None,
            hybrid_score: None,
            result_id: None,
            chunk_start: None,
            chunk_end: None,
        });
    }

    Ok(SearchOutcome {
        results,
        files_with_matches: files_with_matches.len(),
        total_matches,
        mode: IndexMode::Index,
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
fn hybrid_search(
    query: &str,
    root: &std::path::Path,
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
    let index_path = root.join(INDEX_DIR);
    let embedding_db_path = root.join(".cgrep").join("embeddings.sqlite");

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
    };

    // Try cache
    if use_cache {
        if let Ok(cache) = SearchCache::new(root, cache_ttl_ms) {
            if let Ok(Some(entry)) = cache.get::<Vec<HybridResult>>(&cache_key) {
                // Return cached results
                let results: Vec<SearchResult> = entry
                    .data
                    .iter()
                    .map(|hr| SearchResult {
                        path: hr.path.clone(),
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
                    .collect();

                return Ok(SearchOutcome {
                    results,
                    files_with_matches: entry
                        .data
                        .iter()
                        .map(|r| r.path.clone())
                        .collect::<HashSet<_>>()
                        .len(),
                    total_matches: entry.data.len(),
                    mode: IndexMode::Index,
                });
            }
        }
    }

    // Open embedding storage if available
    let embedding_storage = if embedding_db_path.exists() {
        EmbeddingStorage::open(&embedding_db_path).ok()
    } else {
        None
    };

    // Get BM25 results first
    if !index_path.exists() {
        return Err(anyhow::anyhow!(
            "Index required for hybrid search. Run: cgrep index"
        ));
    }

    let bm25_outcome = index_search(
        query,
        root,
        max_results * 3, // Get more for reranking
        0,               // No context yet
        file_type,
        compiled_glob,
        compiled_exclude,
        config_exclude_patterns,
        false,
    )?;

    // Convert to BM25Result format
    let bm25_results: Vec<BM25Result> = bm25_outcome
        .results
        .iter()
        .map(|r| BM25Result {
            path: r.path.clone(),
            score: r.score,
            snippet: r.snippet.clone(),
            line: r.line,
            chunk_start: r.line.map(|l| l as u32),
            chunk_end: r.line.map(|l| l as u32),
        })
        .collect();

    // Create hybrid searcher
    let hybrid_config = HybridConfig::default().with_max_results(max_results);
    let hybrid_searcher = HybridSearcher::new(hybrid_config);

    // Helper to make result ID
    let make_id = |path: &str, line: Option<usize>| -> String {
        let input = match line {
            Some(l) => format!("{}:{}", path, l),
            None => path.to_string(),
        };
        let hash = blake3::hash(input.as_bytes());
        hash.to_hex()[..16].to_string()
    };

    // Perform hybrid search based on mode
    let hybrid_results: Vec<HybridResult> = match mode {
        HybridSearchMode::Semantic | HybridSearchMode::Hybrid => {
            if let Some(ref storage) = embedding_storage {
                let provider_type = config.embeddings.provider();
                let provider_config = match provider_type {
                    EmbeddingProviderType::Command => Some(EmbeddingProviderConfig {
                        provider: "command".to_string(),
                        model: config.embeddings.model().to_string(),
                        command: Some(config.embeddings.command().to_string()),
                        normalize: true,
                    }),
                    EmbeddingProviderType::Dummy => Some(EmbeddingProviderConfig {
                        provider: "dummy".to_string(),
                        model: "dummy".to_string(),
                        command: None,
                        normalize: true,
                    }),
                    EmbeddingProviderType::Builtin => None,
                };

                let query_embedding = match provider_config {
                    Some(ref provider_config) => match create_provider(provider_config)
                        .and_then(|p| p.embed_one(query))
                    {
                        Ok(query_embedding) => Some(query_embedding),
                        Err(err) => {
                            eprintln!("Warning: embedding query failed (using BM25 only): {}", err);
                            None
                        }
                    },
                    None => {
                        eprintln!(
                            "Warning: embedding provider type 'builtin' is not implemented yet. Using BM25 only."
                        );
                        None
                    }
                };

                if let Some(query_embedding) = query_embedding {
                    hybrid_searcher
                        .rerank_with_embeddings(bm25_results, &query_embedding, storage)
                        .unwrap_or_default()
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
                            result_id: Some(make_id(&r.path, r.line)),
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
                        result_id: Some(make_id(&r.path, r.line)),
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
                    result_id: Some(make_id(&r.path, r.line)),
                })
                .collect()
        }
    };

    // Convert to SearchResult with context
    let mut results: Vec<SearchResult> = Vec::with_capacity(max_results.min(hybrid_results.len()));
    let mut files_with_matches: HashSet<String> = HashSet::new();

    for hr in hybrid_results.iter().take(max_results) {
        // Apply filters
        if !matches_file_type(&hr.path, file_type) {
            continue;
        }
        if !matches_glob_compiled(&hr.path, compiled_glob) {
            continue;
        }
        if should_exclude_compiled(&hr.path, compiled_exclude) {
            continue;
        }
        if config_exclude_patterns
            .iter()
            .any(|p| should_exclude_compiled(&hr.path, Some(p)))
        {
            continue;
        }

        files_with_matches.insert(hr.path.clone());

        // Get context lines if needed
        let (context_before, context_after) = if context > 0 && hr.line.is_some() {
            get_context_lines(&root.join(&hr.path), hr.line.unwrap(), context)
        } else {
            (vec![], vec![])
        };

        results.push(SearchResult {
            path: hr.path.clone(),
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
        if let Ok(cache) = SearchCache::new(root, cache_ttl_ms) {
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
