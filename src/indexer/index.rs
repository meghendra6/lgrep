// SPDX-License-Identifier: MIT OR Apache-2.0

//! Index builder using tantivy for BM25 search

use anyhow::{Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;
use std::sync::mpsc;
use std::time::SystemTime;
use tantivy::{
    schema::{Field, Schema, STORED, TEXT},
    Index, IndexWriter, TantivyDocument,
};

use crate::indexer::scanner::{detect_language, FileScanner};
use crate::parser::symbols::SymbolExtractor;
use cgrep::utils::INDEX_DIR;
const METADATA_FILE: &str = ".cgrep/metadata.json";

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

struct TextChunk {
    start_line: u64,
    content: String,
}

enum ReadOutcome {
    Text { chunks: Vec<TextChunk>, hash: String },
    Binary { hash: Option<String> },
}

fn read_text_chunks(path: &Path, max_doc_bytes: usize) -> Result<ReadOutcome> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut chunks: Vec<TextChunk> = Vec::new();
    let mut current_chunk = String::new();
    let mut chunk_start_line: u64 = 1;
    let mut current_line: u64 = 0;
    let mut line = String::new();

    loop {
        line.clear();
        let read = match reader.read_line(&mut line) {
            Ok(read) => read,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::InvalidData {
                    return Ok(ReadOutcome::Binary { hash: None });
                }
                return Err(err.into());
            }
        };
        if read == 0 {
            break;
        }

        current_line += 1;
        if line.as_bytes().iter().any(|&b| b == 0) {
            return Ok(ReadOutcome::Binary { hash: None });
        }

        hasher.update(line.as_bytes());

        if current_chunk.len() + line.len() > max_doc_bytes && !current_chunk.is_empty() {
            let content = std::mem::take(&mut current_chunk);
            chunks.push(TextChunk {
                start_line: chunk_start_line,
                content,
            });
            chunk_start_line = current_line;
        }

        current_chunk.push_str(&line);
    }

    if !current_chunk.is_empty() {
        chunks.push(TextChunk {
            start_line: chunk_start_line,
            content: current_chunk,
        });
    }

    let hash = hasher.finalize().to_hex().to_string();
    Ok(ReadOutcome::Text { chunks, hash })
}

fn extract_symbols_from_chunks(chunks: &[TextChunk], lang: &str) -> String {
    let extractor = SymbolExtractor::new();
    let mut seen = HashSet::new();

    for chunk in chunks {
        if let Ok(symbols) = extractor.extract(&chunk.content, lang) {
            for symbol in symbols {
                seen.insert(symbol.name);
            }
        }
    }

    seen.into_iter().collect::<Vec<_>>().join(" ")
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
    pub content: Field,
    pub language: Field,
    pub symbols: Field,
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
        let content = schema_builder.add_text_field("content", TEXT | STORED);
        let language = schema_builder.add_text_field("language", TEXT | STORED);
        let symbols = schema_builder.add_text_field("symbols", TEXT | STORED);
        let line_number =
            schema_builder.add_u64_field("line_number", tantivy::schema::INDEXED | STORED);

        let schema = schema_builder.build();
        let fields = IndexFields {
            path,
            content,
            language,
            symbols,
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
    pub fn build(&self, force: bool) -> Result<usize> {
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
            Index::open_in_dir(&index_path).context("Failed to open existing index")?
        } else {
            if index_path.exists() {
                std::fs::remove_dir_all(&index_path)?;
            }
            std::fs::create_dir_all(&index_path)?;
            Index::create_in_dir(&index_path, self.schema.clone())
                .context("Failed to create index")?
        };

        let mut writer: IndexWriter = index
            .writer(50_000_000) // 50MB heap
            .context("Failed to create index writer")?;

        let scanner = FileScanner::with_excludes(&self.root, self.exclude_patterns.clone());
        let files = scanner.list_files()?;
        let total_files = files.len();

        enum ProcessedFile {
            Skipped { path: String, meta: FileMetadata },
            Indexed { path: String, meta: FileMetadata, docs: Vec<TantivyDocument> },
            ReadError { path: String, fallback: Option<FileMetadata> },
        }

        let mut new_metadata = IndexMetadata {
            files: HashMap::with_capacity(total_files),
        };
        let mut indexed_count = 0usize;
        let mut skipped_count = 0usize;
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
        let content_field = self.fields.content;
        let language_field = self.fields.language;
        let symbols_field = self.fields.symbols;
        let line_number_field = self.fields.line_number;

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
                            let _ = tx.send(ProcessedFile::ReadError { path: path_str, fallback: None });
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

                    if let Some(meta) = should_skip_without_read(existing_meta.as_ref(), mtime, size, force) {
                        let _ = tx.send(ProcessedFile::Skipped { path: path_str, meta });
                        pb_producer.inc(1);
                        return;
                    }

                    let outcome = match read_text_chunks(path, MAX_DOC_BYTES) {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            let _ = tx.send(ProcessedFile::ReadError { path: path_str, fallback: existing_meta });
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
                            let _ = tx.send(ProcessedFile::Skipped { path: path_str, meta });
                            pb_producer.inc(1);
                            return;
                        }
                    };

                    if let Some(meta) = existing_meta.as_ref() {
                        if !force && !hash.is_empty() && meta.hash == hash {
                            let mut updated = meta.clone();
                            updated.mtime = mtime;
                            updated.size = size;
                            let _ = tx.send(ProcessedFile::Skipped { path: path_str, meta: updated });
                            pb_producer.inc(1);
                            return;
                        }
                    }

                    let lang_str = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .and_then(detect_language)
                        .unwrap_or_default();

                    let symbols = if !lang_str.is_empty() {
                        if let Some(meta) = existing_meta.as_ref() {
                            if meta.hash == hash {
                                meta.symbols.clone()
                            } else {
                                extract_symbols_from_chunks(&chunks, &lang_str)
                            }
                        } else {
                            extract_symbols_from_chunks(&chunks, &lang_str)
                        }
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
                        let _ = tx.send(ProcessedFile::Skipped { path: path_str, meta });
                        pb_producer.inc(1);
                        return;
                    }

                    let mut docs: Vec<TantivyDocument> = Vec::with_capacity(chunks.len());
                    for chunk in &chunks {
                        let mut doc = TantivyDocument::default();
                        doc.add_text(path_field, &path_str);
                        doc.add_text(content_field, &chunk.content);
                        doc.add_text(language_field, &lang_str);
                        doc.add_text(symbols_field, &symbols);
                        doc.add_u64(line_number_field, chunk.start_line);
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
                    ProcessedFile::Skipped { path, meta } => {
                        skipped_count += 1;
                        new_metadata.files.insert(path, meta);
                    }
                    ProcessedFile::Indexed { path, meta, docs } => {
                        if indexing_error.is_none() {
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

        if skipped > 0 {
            println!(
                "{} Indexed {} files ({} unchanged, {} total)",
                "✓".green(),
                indexed.to_string().cyan(),
                skipped.to_string().dimmed(),
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
pub fn run(path: Option<&str>, force: bool, excludes: Vec<String>) -> Result<()> {
    let root = path
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    let builder = IndexBuilder::with_excludes(&root, excludes)?;
    let count = builder.build(force)?;

    println!("Index complete: {} files", count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::path::Path;

    fn load_metadata(root: &Path) -> IndexMetadata {
        let metadata_path = root.join(METADATA_FILE);
        let content = std::fs::read_to_string(metadata_path).expect("read metadata");
        serde_json::from_str(&content).expect("parse metadata")
    }

    #[test]
    fn incremental_index_skips_unchanged_files() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("one.rs"), "fn a() {}").expect("write one");
        std::fs::write(root.join("two.txt"), "hello").expect("write two");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder.build(false).expect("first build");
        assert_eq!(first, 2);

        let second = builder.build(false).expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn hash_based_skip_on_touch() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("one.rs");
        std::fs::write(&file_path, "fn a() {}").expect("write one");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder.build(false).expect("first build");
        assert_eq!(first, 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file_path, "fn a() {}").expect("touch same content");
        let second = builder.build(false).expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn content_change_reindexes() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        let file_path = root.join("change.rs");
        std::fs::write(&file_path, "fn a() {}").expect("write one");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder.build(false).expect("first build");
        assert_eq!(first, 1);

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file_path, "fn b() {}").expect("write two");
        let second = builder.build(false).expect("second build");
        assert_eq!(second, 1);
    }

    #[test]
    fn binary_files_are_skipped() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("good.rs"), "fn ok() {}").expect("write good");
        std::fs::write(root.join("bin.rs"), vec![0, 159, 146, 150]).expect("write bin");

        let builder = IndexBuilder::new(root).expect("builder");
        let first = builder.build(false).expect("first build");
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
        let count = builder.build(false).expect("build");
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
        let first = builder.build(false).expect("first build");
        assert_eq!(first, 0);

        let second = builder.build(false).expect("second build");
        assert_eq!(second, 0);
    }

    #[test]
    fn symbols_cached_in_metadata() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("lib.rs"), "fn cached_symbol() {}").expect("write lib");

        let builder = IndexBuilder::new(root).expect("builder");
        let count = builder.build(false).expect("build");
        assert_eq!(count, 1);

        let metadata = load_metadata(root);
        let lib_key = root.join("lib.rs").to_string_lossy().to_string();
        let meta = metadata.files.get(&lib_key).expect("meta");
        assert!(meta.symbols.contains("cached_symbol"));
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
