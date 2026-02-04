// SPDX-License-Identifier: MIT OR Apache-2.0

//! Index builder using tantivy for BM25 search

use anyhow::{Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc;
use std::time::SystemTime;
use tantivy::{
    schema::{Field, Schema, Term, STORED, STRING, TEXT},
    Index, IndexWriter, TantivyDocument,
};

use crate::indexer::scanner::{detect_language, FileScanner};
use crate::parser::symbols::{Symbol, SymbolExtractor};
use cgrep::config::{Config, EmbeddingProviderType};
use cgrep::embedding::{
    CommandProvider, DummyProvider, EmbeddingProvider, EmbeddingProviderConfig, EmbeddingStorage,
    FastEmbedder, SymbolEmbeddingInput, DEFAULT_EMBEDDING_DIM,
};
use cgrep::utils::INDEX_DIR;
const METADATA_FILE: &str = ".cgrep/metadata.json";
pub(crate) const DEFAULT_WRITER_BUDGET_BYTES: usize = 50_000_000;
const HIGH_MEMORY_WRITER_BUDGET_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingsMode {
    Off,
    Auto,
    Precompute,
}

impl EmbeddingsMode {
    fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "off" | "false" | "0" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "precompute" | "on" | "true" | "1" => Ok(Self::Precompute),
            other => anyhow::bail!(
                "Invalid value for --embeddings: '{}'. Expected one of: auto, precompute, off",
                other
            ),
        }
    }
}

struct EmbeddingBatchEntry {
    path: String,
    file_hash: String,
    start_idx: usize,
    count: usize,
    symbols: Vec<SymbolEmbeddingMeta>,
}

#[derive(Debug, Clone)]
struct SymbolEmbeddingMeta {
    symbol_id: String,
    lang: String,
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
}

#[derive(Default)]
struct EmbeddingIndexStats {
    files_total: usize,
    files_embedded: usize,
    files_skipped_up_to_date: usize,
    files_deleted: usize,
    symbols_embedded: usize,
}

fn create_embedding_provider(
    mode: EmbeddingsMode,
    config: &Config,
) -> Result<Option<Box<dyn EmbeddingProvider>>> {
    // If the user explicitly disabled embeddings in config, honor it in auto mode.
    // (CLI `--embeddings=precompute` is considered an explicit override.)
    if mode == EmbeddingsMode::Auto
        && matches!(
            config.embeddings.enabled(),
            cgrep::config::EmbeddingEnabled::Off
        )
    {
        return Ok(None);
    }

    // Detect whether the repo config appears to have any embeddings configuration at all.
    // This lets us keep `cgrep index` quiet by default (no warnings) when the provider
    // isn't configured/available.
    let has_embeddings_config = config.embeddings.enabled.is_some()
        || config.embeddings.provider.is_some()
        || config.embeddings.model.is_some()
        || config.embeddings.command.is_some()
        || config.embeddings.chunk_lines.is_some()
        || config.embeddings.chunk_overlap.is_some()
        || config.embeddings.max_file_bytes.is_some()
        || config.embeddings.semantic_max_chunks.is_some()
        || EmbeddingProviderConfig::has_env_overrides();

    let provider_type = config.embeddings.provider();
    let provider_result: Result<Box<dyn EmbeddingProvider>> = match provider_type {
        EmbeddingProviderType::Builtin => EmbeddingProviderConfig::from_env()
            .and_then(|provider_config| FastEmbedder::new(provider_config))
            .map(|provider| Box::new(provider) as Box<dyn EmbeddingProvider>),
        EmbeddingProviderType::Dummy => Ok(Box::new(DummyProvider::new(DEFAULT_EMBEDDING_DIM))),
        EmbeddingProviderType::Command => Ok(Box::new(CommandProvider::new(
            config.embeddings.command().to_string(),
            config.embeddings.model().to_string(),
        ))),
    };

    match mode {
        EmbeddingsMode::Off => Ok(None),
        EmbeddingsMode::Auto => match provider_result {
            Ok(p) => Ok(Some(p)),
            Err(err) => {
                if has_embeddings_config {
                    eprintln!(
                        "Warning: embeddings are disabled (provider unavailable): {}",
                        err
                    );
                }
                Ok(None)
            }
        },
        EmbeddingsMode::Precompute => Ok(Some(provider_result?)),
    }
}

fn read_utf8_text_bytes(bytes: &[u8]) -> Result<Option<String>> {
    if bytes.contains(&0) {
        return Ok(None);
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    Ok(Some(text.to_string()))
}

fn read_utf8_text(path: &Path) -> Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    if let Ok(mmap) = unsafe { Mmap::map(&file) } {
        return read_utf8_text_bytes(&mmap);
    }

    let bytes = std::fs::read(path)?;
    read_utf8_text_bytes(&bytes)
}

fn flush_embedding_batch(
    provider: &mut dyn EmbeddingProvider,
    batch_texts: &mut Vec<String>,
    batch_entries: &mut Vec<EmbeddingBatchEntry>,
    storage: &mut EmbeddingStorage,
    stats: &mut EmbeddingIndexStats,
) -> Result<()> {
    if batch_texts.is_empty() {
        return Ok(());
    }

    let vectors = provider.embed_texts(batch_texts)?;
    if vectors.len() != batch_texts.len() {
        anyhow::bail!(
            "Embedding provider returned {} vectors for {} inputs",
            vectors.len(),
            batch_texts.len()
        );
    }

    if let Some(first) = vectors.first() {
        let dimension = first.len();
        if dimension > 0 {
            let _ = storage.set_meta("dimension", &dimension.to_string());
        }
    }

    for entry in batch_entries.iter() {
        let end = entry.start_idx + entry.count;
        let slice = &vectors[entry.start_idx..end];

        let mut inputs: Vec<SymbolEmbeddingInput<'_>> = Vec::with_capacity(entry.count);
        for (i, embedding) in slice.iter().enumerate() {
            let meta = &entry.symbols[i];
            inputs.push(SymbolEmbeddingInput {
                symbol_id: meta.symbol_id.as_str(),
                lang: meta.lang.as_str(),
                symbol_kind: meta.kind.as_str(),
                symbol_name: meta.name.as_str(),
                start_line: meta.start_line,
                end_line: meta.end_line,
                embedding: embedding.as_slice(),
            });
        }

        storage.replace_file_symbols(&entry.path, &entry.file_hash, &inputs)?;

        stats.files_embedded += 1;
        stats.symbols_embedded += entry.count;
    }

    batch_texts.clear();
    batch_entries.clear();
    Ok(())
}

fn index_embeddings(
    root: &Path,
    mode: EmbeddingsMode,
    embeddings_force: bool,
    config: &Config,
    index_metadata: &IndexMetadata,
) -> Result<EmbeddingIndexStats> {
    let Some(mut provider) = create_embedding_provider(mode, config)? else {
        return Ok(EmbeddingIndexStats::default());
    };

    let mut stats = EmbeddingIndexStats::default();
    let mut storage = EmbeddingStorage::open_default(root)?;

    if embeddings_force {
        storage.reset_schema()?;
    } else if !storage.is_symbol_unit()? {
        let message = "Embeddings DB schema mismatch (expected symbol-level). Run `cgrep index --embeddings-force` to rebuild embeddings.";
        return match mode {
            EmbeddingsMode::Auto => {
                eprintln!("Warning: {}. Skipping embeddings.", message);
                Ok(stats)
            }
            EmbeddingsMode::Precompute => Err(anyhow::anyhow!(message)),
            EmbeddingsMode::Off => Ok(stats),
        };
    }

    let provider_label = match config.embeddings.provider() {
        EmbeddingProviderType::Builtin => "builtin",
        EmbeddingProviderType::Dummy => "dummy",
        EmbeddingProviderType::Command => "command",
    };

    let _ = storage.set_meta("schema_version", "2");
    let _ = storage.set_meta("unit", "symbol");
    let _ = storage.set_meta("provider", provider_label);
    // Best-effort: record model early (dimension becomes known after first embed call).
    let _ = storage.set_meta("model", provider.model_id());
    let batch_size = provider.batch_size();
    let max_file_bytes = config.embeddings.max_file_bytes();
    let extractor = SymbolExtractor::new();

    let result: Result<()> = (|| {
        let current_paths: HashSet<&str> =
            index_metadata.files.keys().map(|p| p.as_str()).collect();

        // Clean embeddings for files that no longer exist in the repo.
        let stored_paths = storage.list_paths()?;
        for stored_path in stored_paths {
            if !current_paths.contains(stored_path.as_str()) {
                let _ = storage.delete_file_symbols(&stored_path)?;
                stats.files_deleted += 1;
            }
        }

        let mut batch_texts: Vec<String> = Vec::new();
        let mut batch_entries: Vec<EmbeddingBatchEntry> = Vec::new();

        for (path, meta) in index_metadata.files.iter() {
            stats.files_total += 1;

            // If the file is binary, ensure any old embeddings are removed.
            if meta.is_binary || meta.hash.is_empty() {
                let _ = storage.delete_file_symbols(path)?;
                continue;
            }

            if !embeddings_force && !storage.file_needs_update(path, &meta.hash)? {
                stats.files_skipped_up_to_date += 1;
                continue;
            }

            let file_path = Path::new(path);
            let text = match read_utf8_text(file_path) {
                Ok(Some(text)) => text,
                Ok(None) => {
                    // Binary/non-UTF8: ensure we don't keep stale embeddings.
                    let _ = storage.delete_file_symbols(path)?;
                    continue;
                }
                Err(err) => {
                    // Keep any existing embeddings if the file can't be read right now.
                    eprintln!("Warning: failed to read {} for embeddings: {}", path, err);
                    continue;
                }
            };

            let file_hash = meta.hash.clone();

            if text.as_bytes().len() > max_file_bytes {
                storage.replace_file_symbols(path, &file_hash, &[])?;
                stats.files_embedded += 1;
                continue;
            }

            let lang_str = file_path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(detect_language)
                .unwrap_or_default();

            if lang_str.is_empty() {
                let _ = storage.delete_file_symbols(path)?;
                continue;
            }

            let symbols = match extractor.extract(&text, &lang_str) {
                Ok(symbols) => symbols,
                Err(_) => Vec::new(),
            };

            if symbols.is_empty() {
                storage.replace_file_symbols(path, &file_hash, &[])?;
                stats.files_embedded += 1;
                continue;
            }

            let mut texts: Vec<String> = Vec::with_capacity(symbols.len());
            let mut symbol_meta: Vec<SymbolEmbeddingMeta> = Vec::with_capacity(symbols.len());

            for symbol in symbols {
                let symbol_id = symbol_id_for(path, &lang_str, &symbol);
                let start_line = (symbol.line.min(u32::MAX as usize)) as u32;
                let end_line = (symbol.end_line.min(u32::MAX as usize)) as u32;
                let content = build_symbol_content(&text, &symbol);
                if content.is_empty() {
                    continue;
                }

                texts.push(content);
                symbol_meta.push(SymbolEmbeddingMeta {
                    symbol_id,
                    lang: lang_str.to_string(),
                    kind: symbol.kind.to_string(),
                    name: symbol.name.clone(),
                    start_line,
                    end_line,
                });
            }

            if texts.is_empty() {
                storage.replace_file_symbols(path, &file_hash, &[])?;
                stats.files_embedded += 1;
                continue;
            }

            // Batch across files but keep each file's symbols contiguous.
            if !batch_texts.is_empty() && batch_texts.len() + texts.len() > batch_size {
                flush_embedding_batch(
                    provider.as_mut(),
                    &mut batch_texts,
                    &mut batch_entries,
                    &mut storage,
                    &mut stats,
                )?;
            }

            let start_idx = batch_texts.len();
            let count = texts.len();
            batch_texts.extend(texts);
            batch_entries.push(EmbeddingBatchEntry {
                path: path.clone(),
                file_hash,
                start_idx,
                count,
                symbols: symbol_meta,
            });
        }

        flush_embedding_batch(
            provider.as_mut(),
            &mut batch_texts,
            &mut batch_entries,
            &mut storage,
            &mut stats,
        )?;

        Ok(())
    })();

    match (mode, result) {
        (_, Ok(())) => Ok(stats),
        (EmbeddingsMode::Auto, Err(err)) => {
            eprintln!("Warning: embedding indexing failed (auto mode): {}", err);
            Ok(stats)
        }
        (_, Err(err)) => Err(err),
    }
}

/// Metadata for incremental indexing
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexMetadata {
    /// Map of file path to metadata
    #[serde(default, deserialize_with = "deserialize_files")]
    files: HashMap<String, FileMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct FileMetadata {
    mtime: u64,
    size: u64,
    hash: String,
    symbols: String,
    is_binary: bool,
}

impl Default for FileMetadata {
    fn default() -> Self {
        Self {
            mtime: 0,
            size: 0,
            hash: String::new(),
            symbols: String::new(),
            is_binary: false,
        }
    }
}

impl FileMetadata {
    fn legacy(mtime_secs: u64) -> Self {
        let mtime = mtime_secs.saturating_mul(1_000_000_000);
        Self {
            mtime,
            ..Self::default()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FileMetadataCompat {
    Legacy(u64),
    V1(FileMetadata),
}

fn deserialize_files<'de, D>(deserializer: D) -> Result<HashMap<String, FileMetadata>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: HashMap<String, FileMetadataCompat> = HashMap::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(path, meta)| {
            let meta = match meta {
                FileMetadataCompat::Legacy(mtime) => FileMetadata::legacy(mtime),
                FileMetadataCompat::V1(meta) => meta,
            };
            (path, meta)
        })
        .collect())
}

#[cfg(test)]
const MAX_DOC_BYTES: usize = 64 * 1024;
#[cfg(not(test))]
const MAX_DOC_BYTES: usize = 1024 * 1024;
const SYMBOL_PREVIEW_MAX_LINES: usize = 20;
const SYMBOL_CONTENT_MAX_CHARS: usize = 2000;

struct TextChunk {
    start_line: u64,
    content: String,
}

enum ReadOutcome {
    Text {
        chunks: Vec<TextChunk>,
        hash: String,
    },
    Binary {
        hash: Option<String>,
    },
}

fn read_text_chunks(path: &Path, max_doc_bytes: usize) -> Result<ReadOutcome> {
    let file = std::fs::File::open(path)?;
    if let Ok(mmap) = unsafe { Mmap::map(&file) } {
        return read_text_chunks_from_bytes(&mmap, max_doc_bytes);
    }

    let bytes = std::fs::read(path)?;
    read_text_chunks_from_bytes(&bytes, max_doc_bytes)
}

fn read_text_chunks_from_bytes(bytes: &[u8], max_doc_bytes: usize) -> Result<ReadOutcome> {
    if bytes.iter().any(|&b| b == 0) {
        return Ok(ReadOutcome::Binary { hash: None });
    }

    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Ok(ReadOutcome::Binary { hash: None }),
    };

    let hash = blake3::hash(bytes).to_hex().to_string();
    let chunks = build_chunks(text, max_doc_bytes);
    Ok(ReadOutcome::Text { chunks, hash })
}

fn build_chunks(text: &str, max_doc_bytes: usize) -> Vec<TextChunk> {
    let bytes = text.as_bytes();
    let mut chunks: Vec<TextChunk> = Vec::new();
    let mut current_chunk = String::new();
    let mut chunk_start_line: u64 = 1;
    let mut current_line: u64 = 0;
    let mut line_start: usize = 0;

    for (idx, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            let line_end = idx + 1;
            current_line += 1;
            let line = &text[line_start..line_end];

            if current_chunk.len() + line.len() > max_doc_bytes && !current_chunk.is_empty() {
                let content = std::mem::take(&mut current_chunk);
                chunks.push(TextChunk {
                    start_line: chunk_start_line,
                    content,
                });
                chunk_start_line = current_line;
            }

            current_chunk.push_str(line);
            line_start = line_end;
        }
    }

    if line_start < bytes.len() {
        current_line += 1;
        let line = &text[line_start..];
        if current_chunk.len() + line.len() > max_doc_bytes && !current_chunk.is_empty() {
            let content = std::mem::take(&mut current_chunk);
            chunks.push(TextChunk {
                start_line: chunk_start_line,
                content,
            });
            chunk_start_line = current_line;
        }
        current_chunk.push_str(line);
    }

    if !current_chunk.is_empty() {
        chunks.push(TextChunk {
            start_line: chunk_start_line,
            content: current_chunk,
        });
    }

    chunks
}

fn join_chunks(chunks: &[TextChunk]) -> String {
    let mut text = String::new();
    for chunk in chunks {
        text.push_str(&chunk.content);
    }
    text
}

fn extract_symbols_from_text(text: &str, lang: &str) -> Vec<Symbol> {
    let extractor = SymbolExtractor::new();
    extractor.extract(text, lang).unwrap_or_default()
}

fn extract_symbol_names(symbols: &[Symbol]) -> String {
    let mut seen = HashSet::new();
    for symbol in symbols {
        seen.insert(symbol.name.clone());
    }
    seen.into_iter().collect::<Vec<_>>().join(" ")
}

fn symbol_id_for(path: &str, lang: &str, symbol: &Symbol) -> String {
    let range = if let (Some(start), Some(end)) = (symbol.byte_start, symbol.byte_end) {
        format!("{}:{}", start, end)
    } else {
        format!("{}:{}", symbol.line, symbol.end_line)
    };
    let input = format!(
        "{}:{}:{}:{}:{}",
        path,
        lang,
        symbol.kind,
        symbol.name,
        range
    );
    let hash = blake3::hash(input.as_bytes());
    hash.to_hex().to_string()
}

fn build_symbol_preview(source: &str, symbol: &Symbol) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    let start_idx = symbol.line.saturating_sub(1);
    if start_idx >= lines.len() {
        return String::new();
    }

    let symbol_end_excl = symbol.end_line.min(lines.len());
    let preview_end = (start_idx + SYMBOL_PREVIEW_MAX_LINES).min(symbol_end_excl);
    lines[start_idx..preview_end].join("\n")
}

fn build_symbol_content(source: &str, symbol: &Symbol) -> String {
    let header = format!("{} {}", symbol.name, symbol.kind);
    let preview = build_symbol_preview(source, symbol);
    let combined = if preview.is_empty() {
        header
    } else {
        format!("{}\n{}", header, preview)
    };
    truncate_to_chars(&combined, SYMBOL_CONTENT_MAX_CHARS)
}

fn truncate_to_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut count = 0usize;
    for (idx, _) in input.char_indices() {
        if count == max_chars {
            return input[..idx].to_string();
        }
        count += 1;
    }
    input.to_string()
}

fn should_skip_without_read(
    existing_meta: Option<&FileMetadata>,
    mtime: u64,
    size: u64,
    force: bool,
) -> Option<FileMetadata> {
    if force {
        return None;
    }

    let meta = existing_meta?;
    if meta.is_binary && meta.mtime == mtime && meta.size == size {
        return Some(meta.clone());
    }

    if !meta.hash.is_empty() && meta.mtime == mtime && meta.size == size {
        return Some(meta.clone());
    }

    None
}

/// Tantivy field handles
pub struct IndexFields {
    pub path: Field,
    pub path_exact: Field,
    pub content: Field,
    pub language: Field,
    pub symbols: Field,
    pub doc_type: Field,
    pub symbol_id: Field,
    pub symbol_end_line: Field,
    #[allow(dead_code)]
    pub line_number: Field,
}

/// Build search index
pub struct IndexBuilder {
    root: std::path::PathBuf,
    schema: Schema,
    fields: IndexFields,
    exclude_patterns: Vec<String>,
}

impl IndexBuilder {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        Self::with_excludes(root, Vec::new())
    }

    /// Create index builder with exclude patterns
    pub fn with_excludes(root: impl AsRef<Path>, excludes: Vec<String>) -> Result<Self> {
        let mut schema_builder = Schema::builder();

        let path = schema_builder.add_text_field("path", TEXT | STORED);
        let path_exact = schema_builder.add_text_field("path_exact", STRING | STORED);
        let content = schema_builder.add_text_field("content", TEXT | STORED);
        let language = schema_builder.add_text_field("language", TEXT | STORED);
        let symbols = schema_builder.add_text_field("symbols", TEXT | STORED);
        let doc_type = schema_builder.add_text_field("doc_type", STRING | STORED);
        let symbol_id = schema_builder.add_text_field("symbol_id", STRING | STORED);
        let symbol_end_line = schema_builder.add_u64_field("symbol_end_line", STORED);
        let line_number =
            schema_builder.add_u64_field("line_number", tantivy::schema::INDEXED | STORED);

        let schema = schema_builder.build();
        let fields = IndexFields {
            path,
            path_exact,
            content,
            language,
            symbols,
            doc_type,
            symbol_id,
            symbol_end_line,
            line_number,
        };

        Ok(Self {
            root: root.as_ref().to_path_buf(),
            schema,
            fields,
            exclude_patterns: excludes,
        })
    }

    /// Build or rebuild the index (with incremental support)
    pub fn build(&self, force: bool, writer_budget_bytes: usize) -> Result<usize> {
        let index_path = self.root.join(INDEX_DIR);
        let metadata_path = self.root.join(METADATA_FILE);

        // Load existing metadata if not forcing rebuild
        let old_metadata = if !force && metadata_path.exists() {
            let content = std::fs::read_to_string(&metadata_path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            IndexMetadata::default()
        };

        std::fs::create_dir_all(&index_path)?;

        // Check if valid index exists (has meta.json from tantivy)
        let index_meta_exists = index_path.join("meta.json").exists();

        // Open existing index or create new one
        let index = if index_meta_exists && !force {
            let index = Index::open_in_dir(&index_path).context("Failed to open existing index")?;
            let schema = index.schema();
            if schema.get_field("path_exact").is_err()
                || schema.get_field("doc_type").is_err()
                || schema.get_field("symbol_id").is_err()
                || schema.get_field("symbol_end_line").is_err()
            {
                anyhow::bail!(
                    "Index schema upgrade required: missing symbol-level fields.\n\
                     Run 'cgrep index --force' to rebuild the index."
                );
            }
            index
        } else {
            if index_path.exists() {
                std::fs::remove_dir_all(&index_path)?;
            }
            std::fs::create_dir_all(&index_path)?;
            Index::create_in_dir(&index_path, self.schema.clone())
                .context("Failed to create index")?
        };

        let mut writer: IndexWriter = index
            .writer(writer_budget_bytes)
            .context("Failed to create index writer")?;

        let scanner = FileScanner::with_excludes(&self.root, self.exclude_patterns.clone())
            .with_gitignore(false);
        let files = scanner.list_files()?;
        let current_paths: HashSet<String> = files
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let total_files = files.len();

        enum ProcessedFile {
            Skipped {
                path: String,
                meta: FileMetadata,
                delete_docs: bool,
            },
            Indexed {
                path: String,
                meta: FileMetadata,
                docs: Vec<TantivyDocument>,
            },
            ReadError {
                path: String,
                fallback: Option<FileMetadata>,
            },
        }

        let mut new_metadata = IndexMetadata {
            files: HashMap::with_capacity(total_files),
        };
        let mut indexed_count = 0usize;
        let mut skipped_count = 0usize;
        let mut deleted_count = 0usize;
        let mut error_count = 0usize;
        let mut indexing_error: Option<anyhow::Error> = None;

        let pb = ProgressBar::new(total_files as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{bar:40.cyan/blue}] {pos}/{len} files | Indexing {msg}")
                .expect("valid progress bar template")
                .progress_chars("##."),
        );

        let (tx, rx) = mpsc::sync_channel::<ProcessedFile>(64);
        let path_field = self.fields.path;
        let path_exact_field = self.fields.path_exact;
        let content_field = self.fields.content;
        let language_field = self.fields.language;
        let symbols_field = self.fields.symbols;
        let doc_type_field = self.fields.doc_type;
        let symbol_id_field = self.fields.symbol_id;
        let symbol_end_line_field = self.fields.symbol_end_line;
        let line_number_field = self.fields.line_number;

        if !old_metadata.files.is_empty() {
            let removed_paths: Vec<String> = old_metadata
                .files
                .keys()
                .filter(|path| !current_paths.contains(*path))
                .cloned()
                .collect();
            if !removed_paths.is_empty() {
                for path in &removed_paths {
                    writer.delete_term(Term::from_field_text(path_exact_field, path));
                }
                deleted_count = removed_paths.len();
            }
        }

        let io_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let io_threads = (io_threads * 2).clamp(4, 64);
        let pool = ThreadPoolBuilder::new()
            .num_threads(io_threads)
            .build()
            .context("Failed to create indexing thread pool")?;

        pool.scope(|s| {
            let tx_producer = tx.clone();
            let pb_producer = pb.clone();
            s.spawn(move |_| {
                files.par_iter().for_each_with(tx_producer, |tx, path| {
                    let path_str = path.to_string_lossy().to_string();
                    pb_producer.set_message(path_str.clone());

                    let metadata = match std::fs::metadata(path) {
                        Ok(metadata) => metadata,
                        Err(_) => {
                            let _ = tx.send(ProcessedFile::ReadError {
                                path: path_str,
                                fallback: None,
                            });
                            pb_producer.inc(1);
                            return;
                        }
                    };

                    let mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    let size = metadata.len();

                    let existing_meta = old_metadata.files.get(&path_str).cloned();

                    if let Some(meta) =
                        should_skip_without_read(existing_meta.as_ref(), mtime, size, force)
                    {
                        let _ = tx.send(ProcessedFile::Skipped {
                            path: path_str,
                            meta,
                            delete_docs: false,
                        });
                        pb_producer.inc(1);
                        return;
                    }

                    let outcome = match read_text_chunks(path, MAX_DOC_BYTES) {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            let _ = tx.send(ProcessedFile::ReadError {
                                path: path_str,
                                fallback: existing_meta,
                            });
                            pb_producer.inc(1);
                            return;
                        }
                    };

                    let (chunks, hash) = match outcome {
                        ReadOutcome::Text { chunks, hash } => (chunks, hash),
                        ReadOutcome::Binary { hash } => {
                            let meta = FileMetadata {
                                mtime,
                                size,
                                hash: hash.unwrap_or_default(),
                                symbols: String::new(),
                                is_binary: true,
                            };
                            let _ = tx.send(ProcessedFile::Skipped {
                                path: path_str,
                                meta,
                                delete_docs: true,
                            });
                            pb_producer.inc(1);
                            return;
                        }
                    };

                    if let Some(meta) = existing_meta.as_ref() {
                        if !force && !hash.is_empty() && meta.hash == hash {
                            let mut updated = meta.clone();
                            updated.mtime = mtime;
                            updated.size = size;
                            let _ = tx.send(ProcessedFile::Skipped {
                                path: path_str,
                                meta: updated,
                                delete_docs: false,
                            });
                            pb_producer.inc(1);
                            return;
                        }
                    }

                    let lang_str = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .and_then(detect_language)
                        .unwrap_or_default();

                    let full_text = join_chunks(&chunks);
                    let symbol_list = if !lang_str.is_empty() {
                        extract_symbols_from_text(&full_text, &lang_str)
                    } else {
                        Vec::new()
                    };
                    let symbols = if !lang_str.is_empty() {
                        extract_symbol_names(&symbol_list)
                    } else {
                        String::new()
                    };

                    let meta = FileMetadata {
                        mtime,
                        size,
                        hash,
                        symbols: symbols.clone(),
                        is_binary: false,
                    };

                    if chunks.is_empty() {
                        let _ = tx.send(ProcessedFile::Skipped {
                            path: path_str,
                            meta,
                            delete_docs: true,
                        });
                        pb_producer.inc(1);
                        return;
                    }

                    let mut docs: Vec<TantivyDocument> =
                        Vec::with_capacity(chunks.len() + symbol_list.len());
                    for chunk in &chunks {
                        let mut doc = TantivyDocument::default();
                        doc.add_text(path_field, &path_str);
                        doc.add_text(path_exact_field, &path_str);
                        doc.add_text(content_field, &chunk.content);
                        doc.add_text(language_field, &lang_str);
                        doc.add_text(symbols_field, &symbols);
                        doc.add_text(doc_type_field, "file");
                        doc.add_u64(line_number_field, chunk.start_line);
                        docs.push(doc);
                    }

                    for symbol in &symbol_list {
                        let symbol_id = symbol_id_for(&path_str, &lang_str, symbol);
                        let content = build_symbol_content(&full_text, symbol);
                        if content.is_empty() {
                            continue;
                        }

                        let mut doc = TantivyDocument::default();
                        doc.add_text(path_field, &path_str);
                        doc.add_text(path_exact_field, &path_str);
                        doc.add_text(content_field, &content);
                        doc.add_text(language_field, &lang_str);
                        doc.add_text(symbols_field, &symbol.name);
                        doc.add_text(doc_type_field, "symbol");
                        doc.add_text(symbol_id_field, &symbol_id);
                        doc.add_u64(line_number_field, symbol.line as u64);
                        doc.add_u64(symbol_end_line_field, symbol.end_line as u64);
                        docs.push(doc);
                    }

                    let _ = tx.send(ProcessedFile::Indexed {
                        path: path_str,
                        meta,
                        docs,
                    });
                    pb_producer.inc(1);
                });
            });

            drop(tx);
            for msg in rx {
                match msg {
                    ProcessedFile::Skipped {
                        path,
                        meta,
                        delete_docs,
                    } => {
                        if delete_docs {
                            writer.delete_term(Term::from_field_text(path_exact_field, &path));
                        }
                        skipped_count += 1;
                        new_metadata.files.insert(path, meta);
                    }
                    ProcessedFile::Indexed { path, meta, docs } => {
                        if indexing_error.is_none() {
                            writer.delete_term(Term::from_field_text(path_exact_field, &path));
                            for doc in docs {
                                if let Err(err) = writer.add_document(doc) {
                                    indexing_error = Some(err.into());
                                    break;
                                }
                            }
                        }
                        indexed_count += 1;
                        new_metadata.files.insert(path, meta);
                    }
                    ProcessedFile::ReadError { path, fallback } => {
                        error_count += 1;
                        eprintln!("Warning: failed to read {}", path);
                        if let Some(meta) = fallback {
                            new_metadata.files.insert(path, meta);
                        }
                    }
                }
            }
        });

        pb.finish_and_clear();

        if let Some(err) = indexing_error {
            return Err(err);
        }

        writer.commit()?;

        // Save updated metadata
        let metadata_json = serde_json::to_string_pretty(&new_metadata)?;
        std::fs::write(&metadata_path, metadata_json)?;

        let indexed = indexed_count;
        let skipped = skipped_count;

        if error_count > 0 {
            eprintln!("Warning: {} files could not be read", error_count);
        }

        if skipped > 0 || deleted_count > 0 {
            println!(
                "{} Indexed {} files ({} unchanged, {} removed, {} total)",
                "✓".green(),
                indexed.to_string().cyan(),
                skipped.to_string().dimmed(),
                deleted_count.to_string().dimmed(),
                total_files
            );
        } else {
            println!("{} Indexed {} files", "✓".green(), indexed);
        }

        Ok(indexed)
    }

    /// Open existing index
    #[allow(dead_code)]
    pub fn open(root: impl AsRef<Path>) -> Result<Index> {
        let index_path = root.as_ref().join(INDEX_DIR);
        Index::open_in_dir(&index_path).context("Failed to open index. Run 'cgrep index' first.")
    }
}

/// Run the index command
pub fn run(
    path: Option<&str>,
    force: bool,
    excludes: Vec<String>,
    high_memory: bool,
    embeddings_mode: &str,
    embeddings_force: bool,
) -> Result<()> {
    let root = path
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    let config = Config::load_for_dir(&root);

    // Merge CLI excludes with config excludes (CLI takes precedence by being added first)
    let mut all_excludes = excludes;
    all_excludes.extend(config.index().exclude_paths().iter().cloned());

    let builder = IndexBuilder::with_excludes(&root, all_excludes)?;
    let writer_budget_bytes = if high_memory {
        eprintln!("Using high-memory indexing: writer budget = 1GiB");
        HIGH_MEMORY_WRITER_BUDGET_BYTES
    } else {
        DEFAULT_WRITER_BUDGET_BYTES
    };
    let count = builder.build(force, writer_budget_bytes)?;

    println!("Index complete: {} files", count);

    let mode = EmbeddingsMode::parse(embeddings_mode)?;
    if embeddings_force && mode == EmbeddingsMode::Off {
        eprintln!("Warning: --embeddings-force has no effect when --embeddings=off");
        return Ok(());
    }

    if mode != EmbeddingsMode::Off {
        let metadata_path = root.join(METADATA_FILE);
        let content = std::fs::read_to_string(&metadata_path).with_context(|| {
            format!("Failed to read index metadata: {}", metadata_path.display())
        })?;
        let index_metadata: IndexMetadata =
            serde_json::from_str(&content).context("Failed to parse index metadata")?;

        let stats = index_embeddings(&root, mode, embeddings_force, &config, &index_metadata)?;
        if stats.files_embedded > 0 || stats.files_skipped_up_to_date > 0 || stats.files_deleted > 0
        {
            println!(
                "Embeddings: {} files embedded ({} symbols), {} up-to-date, {} removed",
                stats.files_embedded,
                stats.symbols_embedded,
                stats.files_skipped_up_to_date,
                stats.files_deleted
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tantivy::{collector::Count, query::TermQuery, schema::IndexRecordOption, Index};
    use tempfile::TempDir;

    fn load_metadata(root: &Path) -> IndexMetadata {
        let metadata_path = root.join(METADATA_FILE);
        let content = std::fs::read_to_string(metadata_path).expect("read metadata");
        serde_json::from_str(&content).expect("parse metadata")
    }

    fn count_docs_for_path(root: &Path, file_path: &Path) -> usize {
        let index_path = root.join(INDEX_DIR);
        let index = Index::open_in_dir(&index_path).expect("open index");
        let schema = index.schema();
        let path_exact_field = schema.get_field("path_exact").expect("path_exact field");
        let doc_type_field = schema.get_field("doc_type").expect("doc_type field");
        let reader = index.reader().expect("reader");
        let searcher = reader.searcher();
        let path_term =
            Term::from_field_text(path_exact_field, &file_path.to_string_lossy().to_string());
        let doc_type_term = Term::from_field_text(doc_type_field, "file");
        let query = tantivy::query::BooleanQuery::new(vec![
            (
                tantivy::query::Occur::Must,
                Box::new(TermQuery::new(path_term, IndexRecordOption::Basic)),
            ),
            (
                tantivy::query::Occur::Must,
                Box::new(TermQuery::new(doc_type_term, IndexRecordOption::Basic)),
            ),
        ]);
        searcher.search(&query, &Count).expect("count")
    }

    #[test]
    fn incremental_index_skips_unchanged_files() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("one.rs"), "fn a() {}").expect("write one");
        std::fs::write(root.join("two.txt"), "hello").expect("write two");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 2);

        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn index_includes_gitignored_paths() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), ".venv/\n").expect("write gitignore");
        std::fs::create_dir_all(root.join(".venv/lib")).expect("create venv");
        std::fs::write(root.join(".venv/lib/site.py"), "print('ok')").expect("write venv file");
        std::fs::write(root.join("main.rs"), "fn main() {}").expect("write main");

        let builder = IndexBuilder::new(root).expect("builder");
        let indexed = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("build");
        assert_eq!(indexed, 2);
    }

    #[test]
    fn hash_based_skip_on_touch() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("one.rs");
        std::fs::write(&file_path, "fn a() {}").expect("write one");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file_path, "fn a() {}").expect("touch same content");
        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn content_change_reindexes() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("change.rs");
        std::fs::write(&file_path, "fn a() {}").expect("write one");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file_path, "fn b() {}").expect("write two");
        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 1);
    }

    #[test]
    fn reindex_replaces_existing_docs() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("replace.rs");
        std::fs::write(&file_path, "fn a() {}").expect("write one");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 1);
        assert_eq!(count_docs_for_path(root, &file_path), 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file_path, "fn b() {}").expect("write two");
        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 1);
        assert_eq!(count_docs_for_path(root, &file_path), 1);
    }

    #[test]
    fn removed_files_are_deleted_from_index() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let keep_path = root.join("keep.rs");
        let drop_path = root.join("drop.rs");
        std::fs::write(&keep_path, "fn keep() {}").expect("write keep");
        std::fs::write(&drop_path, "fn drop() {}").expect("write drop");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 2);
        assert_eq!(count_docs_for_path(root, &drop_path), 1);

        std::fs::remove_file(&drop_path).expect("remove drop");
        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 0);
        assert_eq!(count_docs_for_path(root, &drop_path), 0);
        assert_eq!(count_docs_for_path(root, &keep_path), 1);
    }

    #[test]
    fn binary_files_are_skipped() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("good.rs"), "fn ok() {}").expect("write good");
        std::fs::write(root.join("bin.rs"), vec![0, 159, 146, 150]).expect("write bin");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 1);

        let metadata = load_metadata(root);
        let bin_key = root.join("bin.rs").to_string_lossy().to_string();
        let bin_meta = metadata.files.get(&bin_key).expect("bin meta");
        assert!(bin_meta.is_binary);
        assert!(bin_meta.symbols.is_empty());
    }

    #[test]
    fn non_utf8_text_is_skipped_as_binary() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("bad.rs"), vec![0xFF, 0xFE, 0xFD]).expect("write bad");

        let builder = IndexBuilder::new(root).expect("builder");
        let count = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("build");
        assert_eq!(count, 0);

        let metadata = load_metadata(root);
        let bad_key = root.join("bad.rs").to_string_lossy().to_string();
        let meta = metadata.files.get(&bad_key).expect("meta");
        assert!(meta.is_binary);
    }

    #[test]
    fn binary_files_skip_on_unchanged() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("bin.rs"), vec![0, 1, 2, 3]).expect("write bin");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("first build");
        assert_eq!(first, 0);

        let second = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn symbols_cached_in_metadata() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("lib.rs"), "fn cached_symbol() {}").expect("write lib");

        let builder = IndexBuilder::new(root).expect("builder");
        let count = builder
            .build(false, DEFAULT_WRITER_BUDGET_BYTES)
            .expect("build");
        assert_eq!(count, 1);

        let metadata = load_metadata(root);
        let lib_key = root.join("lib.rs").to_string_lossy().to_string();
        let meta = metadata.files.get(&lib_key).expect("meta");
        assert!(meta.symbols.contains("cached_symbol"));
    }

    #[test]
    fn symbol_id_is_stable() {
        let symbol = Symbol {
            name: "alpha".to_string(),
            kind: crate::parser::symbols::SymbolKind::Function,
            line: 10,
            column: 1,
            end_line: 20,
            byte_start: Some(100),
            byte_end: Some(200),
            scope: None,
        };

        let id1 = symbol_id_for("src/lib.rs", "rust", &symbol);
        let id2 = symbol_id_for("src/lib.rs", "rust", &symbol);
        assert_eq!(id1, id2);

        let mut symbol_changed = symbol.clone();
        symbol_changed.byte_end = Some(201);
        let id3 = symbol_id_for("src/lib.rs", "rust", &symbol_changed);
        assert_ne!(id1, id3);

        let mut symbol_line_range = symbol.clone();
        symbol_line_range.byte_start = None;
        symbol_line_range.byte_end = None;
        let id4 = symbol_id_for("src/lib.rs", "rust", &symbol_line_range);
        assert_ne!(id1, id4);
    }

    #[test]
    fn chunking_records_line_offsets() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("chunk.rs");
        let content = "line1\nline2\nline3\nline4\n";
        std::fs::write(&file_path, content).expect("write");

        let outcome = read_text_chunks(&file_path, 12).expect("chunk");
        let chunks = match outcome {
            ReadOutcome::Text { chunks, .. } => chunks,
            ReadOutcome::Binary { .. } => panic!("expected text"),
        };

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[1].start_line, 3);
    }
}
