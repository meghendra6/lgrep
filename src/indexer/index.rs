//! Index builder using tantivy for BM25 search

use anyhow::{Context, Result};
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;
use tantivy::{
    schema::{Schema, STORED, TEXT, Field},
    Index, IndexWriter, TantivyDocument,
};
use colored::Colorize;

use crate::indexer::scanner::FileScanner;
use crate::parser::symbols::SymbolExtractor;

const INDEX_DIR: &str = ".lgrep";
const METADATA_FILE: &str = ".lgrep/metadata.json";

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
        let line_number = schema_builder.add_u64_field("line_number", tantivy::schema::INDEXED | STORED);

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
            Index::open_in_dir(&index_path)
                .context("Failed to open existing index")?
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
        let files = scanner.scan()?;
        let total_files = files.len();

        // Track counters
        let indexed_count = AtomicUsize::new(0);
        let skipped_count = AtomicUsize::new(0);
        let mut new_metadata = IndexMetadata::default();

        // Process files in parallel and collect results
        let pb = ProgressBar::new(total_files as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{bar:40.cyan/blue}] {pos}/{len} files | Indexing {msg}")
                .expect("valid progress bar template")
                .progress_chars("##."),
        );
        let processed_files: Vec<_> = files
            .par_iter()
            .progress_with(pb.clone())
            .filter_map(|file| {
                let path_str = file.path.to_string_lossy().to_string();
                pb.set_message(path_str.clone());

                // Get file mtime
                let mtime = std::fs::metadata(&file.path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                // Check if file needs re-indexing
                let needs_indexing = force
                    || old_metadata.files.get(&path_str).copied() != Some(mtime);

                if !needs_indexing {
                    skipped_count.fetch_add(1, Ordering::Relaxed);
                    return Some((path_str, mtime, None));
                }

                // Extract symbols using tree-sitter (thread-safe via new extractor per file)
                let symbols = if let Some(ref lang) = file.language {
                    let extractor = SymbolExtractor::new();
                    extractor
                        .extract(&file.content, lang)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|s| s.name)
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    String::new()
                };

                let lang_str = file.language.clone().unwrap_or_default();
                indexed_count.fetch_add(1, Ordering::Relaxed);

                Some((path_str, mtime, Some((file.content.clone(), lang_str, symbols))))
            })
            .collect();

        pb.finish_and_clear();

        // Add documents to index (must be sequential)
        for (path_str, mtime, doc_data) in &processed_files {
            if let Some((content, lang_str, symbols)) = doc_data {
                let mut doc = TantivyDocument::default();
                doc.add_text(self.fields.path, path_str);
                doc.add_text(self.fields.content, content);
                doc.add_text(self.fields.language, lang_str);
                doc.add_text(self.fields.symbols, symbols);

                writer.add_document(doc)?;
            }

            // Always update metadata
            new_metadata.files.insert(path_str.clone(), *mtime);
        }

        writer.commit()?;

        // Save updated metadata
        let metadata_json = serde_json::to_string_pretty(&new_metadata)?;
        std::fs::write(&metadata_path, metadata_json)?;

        let indexed = indexed_count.load(Ordering::Relaxed);
        let skipped = skipped_count.load(Ordering::Relaxed);

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
        Index::open_in_dir(&index_path).context("Failed to open index. Run 'lgrep index' first.")
    }
}

/// Run the index command
pub fn run(path: Option<&str>, force: bool) -> Result<()> {
    let root = path.map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    let builder = IndexBuilder::new(&root)?;
    let count = builder.build(force)?;

    println!("Index complete: {} files", count);
    Ok(())
}
