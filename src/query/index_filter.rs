// SPDX-License-Identifier: MIT OR Apache-2.0

//! Index-backed helpers for narrowing file scans.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tantivy::{
    collector::DocSetCollector,
    query::{BooleanQuery, Occur, Query, TermQuery},
    schema::{Field, FieldType, IndexRecordOption, Term, Value},
    Index, ReloadPolicy, TantivyDocument,
};

use crate::indexer::scanner::{detect_language, ScannedFile};
use cgrep::utils::INDEX_DIR;

/// Find files that likely contain a symbol name using the index.
pub fn find_files_with_symbol(root: &Path, symbol_name: &str) -> Result<Option<Vec<PathBuf>>> {
    find_files_with_field(root, "symbols", symbol_name)
}

/// Find files that likely contain a text term using the index.
pub fn find_files_with_content(root: &Path, term: &str) -> Result<Option<Vec<PathBuf>>> {
    find_files_with_field(root, "content", term)
}

/// Read a list of files into scanned-file structs.
pub fn read_scanned_files(paths: &[PathBuf]) -> Vec<ScannedFile> {
    let mut scanned = Vec::with_capacity(paths.len());
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(path) {
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

fn find_files_with_field(
    root: &Path,
    field_name: &str,
    term: &str,
) -> Result<Option<Vec<PathBuf>>> {
    let index_path = root.join(INDEX_DIR);
    if !index_path.exists() {
        return Ok(None);
    }

    let index = match Index::open_in_dir(&index_path) {
        Ok(index) => index,
        Err(_) => return Ok(None),
    };

    let schema = index.schema();
    let field = match schema.get_field(field_name) {
        Ok(field) => field,
        Err(_) => return Ok(None),
    };
    let path_field = match schema.get_field("path") {
        Ok(field) => field,
        Err(_) => return Ok(None),
    };

    let tokens = tokenize_for_field(&index, field, term)?;
    if tokens.is_empty() {
        return Ok(None);
    }

    let query = build_or_query(field, &tokens);
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .context("Failed to create index reader")?;

    let searcher = reader.searcher();
    let docset = searcher.search(&query, &DocSetCollector)?;

    let mut unique_paths: HashSet<PathBuf> = HashSet::with_capacity(docset.len());
    for doc_address in docset {
        if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
            if let Some(path_value) = doc.get_first(path_field).and_then(|v| v.as_str()) {
                unique_paths.insert(root.join(path_value));
            }
        }
    }

    let mut paths: Vec<PathBuf> = unique_paths.into_iter().collect();
    paths.sort();
    Ok(Some(paths))
}

fn build_or_query(field: Field, tokens: &[String]) -> BooleanQuery {
    let subqueries = tokens
        .iter()
        .map(|token| {
            let term = Term::from_field_text(field, token);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            (Occur::Should, Box::new(query) as Box<dyn Query>)
        })
        .collect();

    BooleanQuery::new(subqueries)
}

fn tokenize_for_field(index: &Index, field: Field, text: &str) -> Result<Vec<String>> {
    let schema = index.schema();
    let field_entry = schema.get_field_entry(field);

    let tokenizer_name = match field_entry.field_type() {
        FieldType::Str(options) => options
            .get_indexing_options()
            .map(|indexing| indexing.tokenizer().to_string()),
        _ => None,
    };

    let Some(tokenizer_name) = tokenizer_name else {
        return Ok(Vec::new());
    };

    let mut analyzer = index
        .tokenizers()
        .get(&tokenizer_name)
        .ok_or_else(|| anyhow::anyhow!("Tokenizer not found: {}", tokenizer_name))?;

    let mut token_stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    token_stream.process(&mut |token| tokens.push(token.text.to_string()));
    tokens.sort();
    tokens.dedup();
    Ok(tokens)
}
