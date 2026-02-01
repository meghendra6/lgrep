// SPDX-License-Identifier: MIT OR Apache-2.0

//! Index builder using tantivy for BM25 search

use anyhow::{Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::SystemTime;
use tantivy::{
    schema::{Field, Schema, STORED, TEXT},
    Index, IndexWriter, TantivyDocument,
};

use crate::indexer::scanner::{detect_language, FileScanner};
use crate::parser::symbols::SymbolExtractor;

const INDEX_DIR: &str = ".cgrep";
const METADATA_FILE: &str = ".cgrep/metadata.json";

/// Metadata for incremental indexing
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexMetadata {
    /// Map of file path to modified time (as secs since epoch)
    files: HashMap<String, u64>,
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
}

impl IndexBuilder {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
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

        let scanner = FileScanner::new(&self.root);
        let files = scanner.list_files()?;
        let total_files = files.len();

        enum ProcessedFile {
            Skipped { path: String, mtime: u64 },
            Indexed { path: String, mtime: u64, doc: TantivyDocument },
            ReadError { path: String },
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

        rayon::scope(|s| {
            let tx_producer = tx.clone();
            let pb_producer = pb.clone();
            s.spawn(move |_| {
                files.par_iter().for_each_with(tx_producer, |tx, path| {
                    let path_str = path.to_string_lossy().to_string();
                    pb_producer.set_message(path_str.clone());

                    let mtime = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    let needs_indexing =
                        force || old_metadata.files.get(&path_str).copied() != Some(mtime);

                    if !needs_indexing {
                        let _ = tx.send(ProcessedFile::Skipped { path: path_str, mtime });
                        pb_producer.inc(1);
                        return;
                    }

                    let content = match std::fs::read_to_string(path) {
                        Ok(content) => content,
                        Err(_) => {
                            let _ = tx.send(ProcessedFile::ReadError { path: path_str });
                            pb_producer.inc(1);
                            return;
                        }
                    };

                    let lang_str = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .and_then(detect_language)
                        .unwrap_or_default();

                    let symbols = if !lang_str.is_empty() {
                        let extractor = SymbolExtractor::new();
                        extractor
                            .extract(&content, &lang_str)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|s| s.name)
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else {
                        String::new()
                    };

                    let mut doc = TantivyDocument::default();
                    doc.add_text(path_field, &path_str);
                    doc.add_text(content_field, &content);
                    doc.add_text(language_field, &lang_str);
                    doc.add_text(symbols_field, &symbols);

                    let _ = tx.send(ProcessedFile::Indexed {
                        path: path_str,
                        mtime,
                        doc,
                    });
                    pb_producer.inc(1);
                });
            });

            drop(tx);
            for msg in rx {
                match msg {
                    ProcessedFile::Skipped { path, mtime } => {
                        skipped_count += 1;
                        new_metadata.files.insert(path, mtime);
                    }
                    ProcessedFile::Indexed { path, mtime, doc } => {
                        if indexing_error.is_none() {
                            if let Err(err) = writer.add_document(doc) {
                                indexing_error = Some(err.into());
                            }
                        }
                        indexed_count += 1;
                        new_metadata.files.insert(path, mtime);
                    }
                    ProcessedFile::ReadError { path } => {
                        error_count += 1;
                        eprintln!("Warning: failed to read {}", path);
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
pub fn run(path: Option<&str>, force: bool) -> Result<()> {
    let root = path
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    let builder = IndexBuilder::new(&root)?;
    let count = builder.build(force)?;

    println!("Index complete: {} files", count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
}
