// SPDX-License-Identifier: MIT OR Apache-2.0

//! SQLite-based storage for symbol embedding vectors.
//!
//! This module provides persistent storage for embedding vectors associated with
//! symbols. It supports CRUD operations, incremental updates based on file
//! hashes, and brute-force cosine similarity search.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// Default embedding dimension for sentence-transformers/all-MiniLM-L6-v2.
pub const DEFAULT_EMBEDDING_DIM: usize = 384;

/// Represents a symbol embedding with its metadata.
#[derive(Debug, Clone)]
pub struct SymbolEmbedding {
    /// Unique identifier for this symbol
    pub symbol_id: String,
    /// Path to the source file (relative to repository root)
    pub path: String,
    /// Language identifier
    pub lang: String,
    /// Symbol kind (function, class, etc.)
    pub symbol_kind: String,
    /// Symbol name
    pub symbol_name: String,
    /// Starting line number (1-indexed)
    pub start_line: u32,
    /// Ending line number (1-indexed, inclusive)
    pub end_line: u32,
    /// Hash of the file content for change detection
    pub file_hash: String,
    /// Hash of the symbol content used for embeddings
    pub content_hash: String,
    /// Embedding vector (f32 values)
    pub embedding: Vec<f32>,
    /// Unix timestamp when this embedding was created
    pub created_at: i64,
}

/// Input symbol data for bulk embedding writes.
pub struct SymbolEmbeddingInput<'a> {
    pub symbol_id: &'a str,
    pub lang: &'a str,
    pub symbol_kind: &'a str,
    pub symbol_name: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub content_hash: &'a str,
    pub embedding: &'a [f32],
}

/// A search result from similarity search.
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    /// The matching symbol
    pub symbol: SymbolEmbedding,
    /// Cosine similarity score (0.0 to 1.0)
    pub score: f32,
}

/// SQLite-based storage for embedding vectors.
///
/// Stores embeddings in `.cgrep/embeddings.sqlite` by default.
pub struct EmbeddingStorage {
    conn: Connection,
    path: PathBuf,
}

impl EmbeddingStorage {
    /// Opens or creates an embedding storage at the specified path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;
        let has_tables: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        let bootstrap_meta = has_tables == 0;

        let storage = Self { conn, path };
        storage.init_schema(bootstrap_meta)?;
        storage.ensure_symbol_schema()?;

        Ok(storage)
    }

    /// Opens an embedding storage in the default location for a repository.
    pub fn open_default<P: AsRef<Path>>(repo_root: P) -> Result<Self> {
        let db_path = repo_root.as_ref().join(".cgrep").join("embeddings.sqlite");
        Self::open(db_path)
    }

    /// Initializes the database schema if it does not exist.
    fn init_schema(&self, bootstrap_meta: bool) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS symbol_embeddings (
                symbol_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                lang TEXT NOT NULL,
                symbol_kind TEXT NOT NULL,
                symbol_name TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                file_hash TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                embedding BLOB NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_symbol_embeddings_path_line
                ON symbol_embeddings(path, start_line, end_line);

            CREATE TABLE IF NOT EXISTS symbol_files (
                path TEXT PRIMARY KEY,
                file_hash TEXT NOT NULL,
                last_modified INTEGER NOT NULL,
                symbol_count INTEGER NOT NULL
            );
            "#,
            )
            .context("Failed to initialize database schema")?;

        if bootstrap_meta {
            self.set_meta("schema_version", "3")?;
            self.set_meta("unit", "symbol")?;
        }

        Ok(())
    }

    fn ensure_symbol_schema(&self) -> Result<()> {
        let unit = self.get_meta("unit")?;
        if unit.as_deref() != Some("symbol") {
            return Ok(());
        }

        let mut stmt = self.conn.prepare("PRAGMA table_info(symbol_embeddings)")?;
        let mut rows = stmt.query([])?;
        let mut has_content_hash = false;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == "content_hash" {
                has_content_hash = true;
                break;
            }
        }

        if !has_content_hash {
            self.conn.execute(
                "ALTER TABLE symbol_embeddings ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }

        let _ = self.set_meta("schema_version", "3");
        Ok(())
    }

    /// Resets the database schema, dropping old tables.
    pub fn reset_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
            DROP TABLE IF EXISTS embedding_chunks;
            DROP TABLE IF EXISTS embedding_files;
            DROP TABLE IF EXISTS embeddings_meta;
            DROP TABLE IF EXISTS symbol_embeddings;
            DROP TABLE IF EXISTS symbol_files;
            DROP TABLE IF EXISTS meta;
            "#,
            )
            .context("Failed to reset embedding schema")?;

        self.init_schema(true)
    }

    /// Returns the path to the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Closes the storage connection explicitly.
    pub fn close(self) -> Result<()> {
        self.conn.close().map_err(|(_, e)| e)?;
        Ok(())
    }

    /// Checks if this database is using symbol-level embeddings.
    pub fn is_symbol_unit(&self) -> Result<bool> {
        Ok(self.get_meta("unit")?.as_deref() == Some("symbol"))
    }

    /// Replaces all embeddings for a file in a single transaction.
    ///
    /// This deletes any existing symbols for the file and then inserts the new symbols.
    pub fn replace_file_symbols(
        &mut self,
        path: &str,
        file_hash: &str,
        last_modified: i64,
        symbols: &[SymbolEmbeddingInput<'_>],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;

        tx.execute(
            "DELETE FROM symbol_embeddings WHERE path = ?1",
            params![path],
        )?;
        tx.execute("DELETE FROM symbol_files WHERE path = ?1", params![path])?;

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        if !symbols.is_empty() {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO symbol_embeddings (
                    symbol_id, path, lang, symbol_kind, symbol_name, start_line, end_line,
                    file_hash, content_hash, embedding, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
            )?;

            for symbol in symbols {
                let embedding_blob = Self::embedding_to_blob(symbol.embedding);
                stmt.execute(params![
                    symbol.symbol_id,
                    path,
                    symbol.lang,
                    symbol.symbol_kind,
                    symbol.symbol_name,
                    symbol.start_line,
                    symbol.end_line,
                    file_hash,
                    symbol.content_hash,
                    embedding_blob,
                    created_at
                ])?;
            }
        }

        tx.execute(
            r#"
            INSERT INTO symbol_files (path, file_hash, last_modified, symbol_count)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                file_hash = excluded.file_hash,
                last_modified = excluded.last_modified,
                symbol_count = excluded.symbol_count
            "#,
            params![path, file_hash, last_modified, symbols.len() as u32],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Syncs symbol embeddings for a file, updating only changed symbols.
    pub fn sync_file_symbols(
        &mut self,
        path: &str,
        file_hash: &str,
        last_modified: i64,
        symbol_ids: &[String],
        symbols: &[SymbolEmbeddingInput<'_>],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;

        if symbol_ids.is_empty() {
            tx.execute(
                "DELETE FROM symbol_embeddings WHERE path = ?1",
                params![path],
            )?;
            tx.execute("DELETE FROM symbol_files WHERE path = ?1", params![path])?;
            tx.commit()?;
            return Ok(());
        }

        let max_vars = 900usize;
        if symbol_ids.len() <= max_vars {
            let mut query = String::from(
                "DELETE FROM symbol_embeddings WHERE path = ?1 AND symbol_id NOT IN (",
            );
            for i in 0..symbol_ids.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!("?{}", i + 2));
            }
            query.push(')');

            let mut params_vec: Vec<&dyn rusqlite::ToSql> =
                Vec::with_capacity(symbol_ids.len() + 1);
            params_vec.push(&path);
            for id in symbol_ids {
                params_vec.push(id);
            }
            tx.execute(&query, params_vec.as_slice())?;
        } else {
            tx.execute_batch(
                r#"
                CREATE TEMP TABLE IF NOT EXISTS temp_symbol_ids (symbol_id TEXT PRIMARY KEY);
                DELETE FROM temp_symbol_ids;
                "#,
            )?;
            {
                let mut stmt =
                    tx.prepare("INSERT OR IGNORE INTO temp_symbol_ids (symbol_id) VALUES (?1)")?;
                for id in symbol_ids {
                    stmt.execute(params![id])?;
                }
            }
            tx.execute(
                "DELETE FROM symbol_embeddings WHERE path = ?1 AND symbol_id NOT IN (SELECT symbol_id FROM temp_symbol_ids)",
                params![path],
            )?;
            tx.execute("DELETE FROM temp_symbol_ids", [])?;
        }

        if !symbols.is_empty() {
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO symbol_embeddings (
                    symbol_id, path, lang, symbol_kind, symbol_name, start_line, end_line,
                    file_hash, content_hash, embedding, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(symbol_id) DO UPDATE SET
                    path = excluded.path,
                    lang = excluded.lang,
                    symbol_kind = excluded.symbol_kind,
                    symbol_name = excluded.symbol_name,
                    start_line = excluded.start_line,
                    end_line = excluded.end_line,
                    file_hash = excluded.file_hash,
                    content_hash = excluded.content_hash,
                    embedding = excluded.embedding,
                    created_at = excluded.created_at
                "#,
            )?;

            for symbol in symbols {
                let embedding_blob = Self::embedding_to_blob(symbol.embedding);
                stmt.execute(params![
                    symbol.symbol_id,
                    path,
                    symbol.lang,
                    symbol.symbol_kind,
                    symbol.symbol_name,
                    symbol.start_line,
                    symbol.end_line,
                    file_hash,
                    symbol.content_hash,
                    embedding_blob,
                    created_at
                ])?;
            }
        }

        tx.execute(
            r#"
            INSERT INTO symbol_files (path, file_hash, last_modified, symbol_count)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                file_hash = excluded.file_hash,
                last_modified = excluded.last_modified,
                symbol_count = excluded.symbol_count
            "#,
            params![path, file_hash, last_modified, symbol_ids.len() as u32],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Retrieves all symbol embeddings for a given file path.
    pub fn get_symbols_for_path(&self, path: &str) -> Result<Vec<SymbolEmbedding>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, path, lang, symbol_kind, symbol_name, start_line, end_line,
                   file_hash, content_hash, embedding, created_at
            FROM symbol_embeddings
            WHERE path = ?1
            ORDER BY start_line
            "#,
        )?;

        let symbols = stmt
            .query_map(params![path], |row| {
                let embedding_blob: Vec<u8> = row.get(9)?;
                Ok(SymbolEmbedding {
                    symbol_id: row.get(0)?,
                    path: row.get(1)?,
                    lang: row.get(2)?,
                    symbol_kind: row.get(3)?,
                    symbol_name: row.get(4)?,
                    start_line: row.get(5)?,
                    end_line: row.get(6)?,
                    file_hash: row.get(7)?,
                    content_hash: row.get(8)?,
                    embedding: Self::blob_to_embedding(&embedding_blob),
                    created_at: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query symbols")?;

        Ok(symbols)
    }

    /// Retrieves a specific symbol embedding by ID.
    pub fn get_symbol(&self, symbol_id: &str) -> Result<Option<SymbolEmbedding>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, path, lang, symbol_kind, symbol_name, start_line, end_line,
                   file_hash, content_hash, embedding, created_at
            FROM symbol_embeddings
            WHERE symbol_id = ?1
            "#,
        )?;

        let symbol = stmt
            .query_row(params![symbol_id], |row| {
                let embedding_blob: Vec<u8> = row.get(9)?;
                Ok(SymbolEmbedding {
                    symbol_id: row.get(0)?,
                    path: row.get(1)?,
                    lang: row.get(2)?,
                    symbol_kind: row.get(3)?,
                    symbol_name: row.get(4)?,
                    start_line: row.get(5)?,
                    end_line: row.get(6)?,
                    file_hash: row.get(7)?,
                    content_hash: row.get(8)?,
                    embedding: Self::blob_to_embedding(&embedding_blob),
                    created_at: row.get(10)?,
                })
            })
            .optional()
            .context("Failed to query symbol")?;

        Ok(symbol)
    }

    /// Deletes all symbol embeddings for a given file path.
    pub fn delete_file_symbols(&self, path: &str) -> Result<usize> {
        let deleted = self
            .conn
            .execute(
                "DELETE FROM symbol_embeddings WHERE path = ?1",
                params![path],
            )
            .context("Failed to delete symbols")?;

        self.conn
            .execute("DELETE FROM symbol_files WHERE path = ?1", params![path])?;

        Ok(deleted)
    }

    /// Deletes all embeddings from the database.
    pub fn clear_all(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
            DELETE FROM symbol_embeddings;
            DELETE FROM symbol_files;
            "#,
            )
            .context("Failed to clear all embeddings")?;

        Ok(())
    }

    /// Checks if a file needs its embeddings updated based on file hash.
    pub fn file_needs_update(&self, path: &str, current_hash: &str) -> Result<bool> {
        let stored_hash: Option<String> = self
            .conn
            .query_row(
                "SELECT file_hash FROM symbol_files WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query file hash")?;

        Ok(stored_hash.as_deref() != Some(current_hash))
    }

    /// Lists all files that have embeddings.
    pub fn list_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT path
            FROM symbol_files
            ORDER BY path
            "#,
        )?;

        let paths = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to list paths")?;

        Ok(paths)
    }

    /// Lists symbol content hashes for a given file path.
    pub fn list_symbol_hashes_for_path(&self, path: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, content_hash
            FROM symbol_embeddings
            WHERE path = ?1
            "#,
        )?;

        let rows = stmt
            .query_map(params![path], |row| {
                let symbol_id: String = row.get(0)?;
                let content_hash: String = row.get(1)?;
                Ok((symbol_id, content_hash))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Performs brute-force similarity search across all embeddings.
    ///
    /// Returns symbols sorted by descending cosine similarity.
    pub fn search_similar(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<SimilarityResult>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, path, lang, symbol_kind, symbol_name, start_line, end_line,
                   file_hash, content_hash, embedding, created_at
            FROM symbol_embeddings
            "#,
        )?;

        let mut results: Vec<SimilarityResult> = stmt
            .query_map([], |row| {
                let embedding_blob: Vec<u8> = row.get(9)?;
                let embedding = Self::blob_to_embedding(&embedding_blob);
                let score = Self::cosine_similarity(query_embedding, &embedding);
                Ok(SimilarityResult {
                    symbol: SymbolEmbedding {
                        symbol_id: row.get(0)?,
                        path: row.get(1)?,
                        lang: row.get(2)?,
                        symbol_kind: row.get(3)?,
                        symbol_name: row.get(4)?,
                        start_line: row.get(5)?,
                        end_line: row.get(6)?,
                        file_hash: row.get(7)?,
                        content_hash: row.get(8)?,
                        embedding,
                        created_at: row.get(10)?,
                    },
                    score,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Sort by score (descending)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);

        Ok(results)
    }

    /// Counts total number of symbol embeddings.
    pub fn count_symbols(&self) -> Result<u64> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM symbol_embeddings", [], |row| {
                    row.get(0)
                })?;
        Ok(count as u64)
    }

    /// Gets metadata value by key.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to get meta")?;
        Ok(value)
    }

    /// Sets metadata value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO meta (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    /// Sets metadata value only if key doesn't exist.
    #[allow(dead_code)]
    fn set_meta_if_absent(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Converts an embedding vector to a compact blob.
    fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Converts a blob back to an embedding vector.
    fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
        blob.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    /// Computes cosine similarity between two vectors.
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let magnitude_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let magnitude_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if magnitude_a == 0.0 || magnitude_b == 0.0 {
            return 0.0;
        }

        dot_product / (magnitude_a * magnitude_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_embedding(dim: usize, value: f32) -> Vec<f32> {
        vec![value; dim]
    }

    #[test]
    fn test_storage_create_and_open() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("embeddings.sqlite");

        // Create new database
        let storage = EmbeddingStorage::open(&db_path).unwrap();
        assert!(db_path.exists());
        storage.close().unwrap();

        // Open existing database
        let storage = EmbeddingStorage::open(&db_path).unwrap();
        storage.close().unwrap();
    }

    #[test]
    fn test_store_and_retrieve_symbol() {
        let dir = tempdir().unwrap();
        let mut storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        let embedding = create_test_embedding(384, 0.5);
        let input = SymbolEmbeddingInput {
            symbol_id: "sym1",
            lang: "rust",
            symbol_kind: "function",
            symbol_name: "main",
            start_line: 1,
            end_line: 3,
            content_hash: "h1",
            embedding: &embedding,
        };

        storage
            .replace_file_symbols("src/main.rs", "hash", 1000, &[input])
            .unwrap();

        let symbol = storage.get_symbol("sym1").unwrap().unwrap();
        assert_eq!(symbol.path, "src/main.rs");
        assert_eq!(symbol.start_line, 1);
        assert_eq!(symbol.end_line, 3);
        assert_eq!(symbol.embedding.len(), 384);
    }

    #[test]
    fn test_file_needs_update() {
        let dir = tempdir().unwrap();
        let mut storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        // New file always needs update
        assert!(storage.file_needs_update("src/main.rs", "hash1").unwrap());

        let embedding = create_test_embedding(4, 0.1);
        let input = SymbolEmbeddingInput {
            symbol_id: "sym1",
            lang: "rust",
            symbol_kind: "function",
            symbol_name: "main",
            start_line: 1,
            end_line: 3,
            content_hash: "h1",
            embedding: &embedding,
        };

        storage
            .replace_file_symbols("src/main.rs", "hash1", 1000, &[input])
            .unwrap();

        assert!(!storage.file_needs_update("src/main.rs", "hash1").unwrap());
        assert!(storage.file_needs_update("src/main.rs", "hash2").unwrap());
    }

    #[test]
    fn test_similarity_search() {
        let dir = tempdir().unwrap();
        let mut storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        let embedding_a = vec![1.0, 0.0, 0.0];
        let embedding_b = vec![0.0, 1.0, 0.0];
        let embedding_c = vec![0.9, 0.1, 0.0];

        let inputs = vec![
            SymbolEmbeddingInput {
                symbol_id: "a",
                lang: "rust",
                symbol_kind: "function",
                symbol_name: "a",
                start_line: 1,
                end_line: 1,
                content_hash: "h1",
                embedding: &embedding_a,
            },
            SymbolEmbeddingInput {
                symbol_id: "b",
                lang: "rust",
                symbol_kind: "function",
                symbol_name: "b",
                start_line: 2,
                end_line: 2,
                content_hash: "h2",
                embedding: &embedding_b,
            },
            SymbolEmbeddingInput {
                symbol_id: "c",
                lang: "rust",
                symbol_kind: "function",
                symbol_name: "c",
                start_line: 3,
                end_line: 3,
                content_hash: "h3",
                embedding: &embedding_c,
            },
        ];

        storage
            .replace_file_symbols("src/lib.rs", "hash", 1000, &inputs)
            .unwrap();

        let results = storage.search_similar(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].symbol.symbol_id, "a");
        assert!((results[0].score - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_delete_file_symbols() {
        let dir = tempdir().unwrap();
        let mut storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        let embedding = vec![1.0, 0.0];
        let inputs = vec![
            SymbolEmbeddingInput {
                symbol_id: "a1",
                lang: "rust",
                symbol_kind: "function",
                symbol_name: "a",
                start_line: 1,
                end_line: 1,
                content_hash: "h1",
                embedding: &embedding,
            },
            SymbolEmbeddingInput {
                symbol_id: "a2",
                lang: "rust",
                symbol_kind: "function",
                symbol_name: "b",
                start_line: 2,
                end_line: 2,
                content_hash: "h2",
                embedding: &embedding,
            },
        ];

        storage
            .replace_file_symbols("a.rs", "hash", 1000, &inputs)
            .unwrap();

        let deleted = storage.delete_file_symbols("a.rs").unwrap();
        assert_eq!(deleted, 2);
        let remaining = storage.get_symbols_for_path("a.rs").unwrap();
        assert!(remaining.is_empty());
    }
}
