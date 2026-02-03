// SPDX-License-Identifier: MIT OR Apache-2.0

//! SQLite-based storage for code embedding vectors.
//!
//! This module provides persistent storage for embedding vectors associated with
//! code chunks. It supports CRUD operations, incremental updates based on file
//! hashes, and brute-force cosine similarity search.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// Default embedding dimension (commonly used by models like text-embedding-ada-002)
pub const DEFAULT_EMBEDDING_DIM: usize = 1536;

/// Represents a code chunk with its embedding vector.
#[derive(Debug, Clone)]
pub struct EmbeddingChunk {
    /// Unique identifier for this chunk
    pub id: i64,
    /// Path to the source file (relative to repository root)
    pub path: String,
    /// Starting line number (1-indexed)
    pub start_line: u32,
    /// Ending line number (1-indexed, inclusive)
    pub end_line: u32,
    /// Hash of the chunk content for change detection
    pub content_hash: String,
    /// Embedding vector (f32 values)
    pub embedding: Vec<f32>,
    /// Unix timestamp when this embedding was created
    pub created_at: i64,
}

/// Metadata about a file's embeddings.
#[derive(Debug, Clone)]
pub struct FileEmbeddingInfo {
    /// Path to the source file
    pub path: String,
    /// Hash of the entire file content
    pub file_hash: String,
    /// Last modification timestamp
    pub last_modified: i64,
    /// Number of chunks in this file
    pub chunk_count: u32,
}

/// A search result from similarity search.
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    /// The matching chunk
    pub chunk: EmbeddingChunk,
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

/// Input chunk data for bulk embedding writes.
pub struct EmbeddingChunkInput<'a> {
    pub start_line: u32,
    pub end_line: u32,
    pub content_hash: &'a str,
    pub embedding: &'a [f32],
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

        let storage = Self { conn, path };
        storage.init_schema()?;

        Ok(storage)
    }

    /// Opens an embedding storage in the default location for a repository.
    pub fn open_default<P: AsRef<Path>>(repo_root: P) -> Result<Self> {
        let db_path = repo_root.as_ref().join(".cgrep").join("embeddings.sqlite");
        Self::open(db_path)
    }

    /// Initializes the database schema if it does not exist.
    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
            CREATE TABLE IF NOT EXISTS embedding_chunks (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                content_hash TEXT NOT NULL,
                embedding BLOB NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_embedding_chunks_path_line 
                ON embedding_chunks(path, start_line, end_line);

            CREATE TABLE IF NOT EXISTS embedding_files (
                path TEXT PRIMARY KEY,
                file_hash TEXT NOT NULL,
                last_modified INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS embeddings_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
            )
            .context("Failed to initialize database schema")?;

        self.set_meta_if_absent("schema_version", "1")?;
        Ok(())
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

    /// Stores an embedding chunk in the database.
    pub fn store_chunk(
        &self,
        path: &str,
        start_line: u32,
        end_line: u32,
        content_hash: &str,
        embedding: &[f32],
    ) -> Result<i64> {
        let embedding_blob = Self::embedding_to_blob(embedding);
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            r#"
            INSERT INTO embedding_chunks (path, start_line, end_line, content_hash, embedding, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![path, start_line, end_line, content_hash, embedding_blob, created_at],
        ).context("Failed to store embedding chunk")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Replaces all embeddings for a file in a single transaction.
    ///
    /// This deletes any existing chunks for the file and then inserts the new chunks and file info.
    pub fn replace_file_embeddings(
        &mut self,
        path: &str,
        file_hash: &str,
        last_modified: i64,
        chunks: &[EmbeddingChunkInput<'_>],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;

        tx.execute(
            "DELETE FROM embedding_chunks WHERE path = ?1",
            params![path],
        )?;
        tx.execute("DELETE FROM embedding_files WHERE path = ?1", params![path])?;

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO embedding_chunks (path, start_line, end_line, content_hash, embedding, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )?;
            for chunk in chunks {
                let embedding_blob = Self::embedding_to_blob(chunk.embedding);
                stmt.execute(params![
                    path,
                    chunk.start_line,
                    chunk.end_line,
                    chunk.content_hash,
                    embedding_blob,
                    created_at
                ])?;
            }
        }

        tx.execute(
            r#"
            INSERT INTO embedding_files (path, file_hash, last_modified, chunk_count)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                file_hash = excluded.file_hash,
                last_modified = excluded.last_modified,
                chunk_count = excluded.chunk_count
            "#,
            params![path, file_hash, last_modified, chunks.len() as u32],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Retrieves all embedding chunks for a given file path.
    pub fn get_chunks_for_path(&self, path: &str) -> Result<Vec<EmbeddingChunk>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, path, start_line, end_line, content_hash, embedding, created_at
            FROM embedding_chunks
            WHERE path = ?1
            ORDER BY start_line
            "#,
        )?;

        let chunks = stmt
            .query_map(params![path], |row| {
                let embedding_blob: Vec<u8> = row.get(5)?;
                Ok(EmbeddingChunk {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    content_hash: row.get(4)?,
                    embedding: Self::blob_to_embedding(&embedding_blob),
                    created_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query chunks")?;

        Ok(chunks)
    }

    /// Retrieves a specific embedding chunk by ID.
    pub fn get_chunk(&self, id: i64) -> Result<Option<EmbeddingChunk>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, path, start_line, end_line, content_hash, embedding, created_at
            FROM embedding_chunks
            WHERE id = ?1
            "#,
        )?;

        let chunk = stmt
            .query_row(params![id], |row| {
                let embedding_blob: Vec<u8> = row.get(5)?;
                Ok(EmbeddingChunk {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    content_hash: row.get(4)?,
                    embedding: Self::blob_to_embedding(&embedding_blob),
                    created_at: row.get(6)?,
                })
            })
            .optional()
            .context("Failed to query chunk")?;

        Ok(chunk)
    }

    /// Deletes all embedding chunks for a given file path.
    pub fn delete_file_chunks(&self, path: &str) -> Result<usize> {
        let deleted = self
            .conn
            .execute(
                "DELETE FROM embedding_chunks WHERE path = ?1",
                params![path],
            )
            .context("Failed to delete chunks")?;

        self.conn
            .execute("DELETE FROM embedding_files WHERE path = ?1", params![path])?;

        Ok(deleted)
    }

    /// Deletes all embeddings from the database.
    pub fn clear_all(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
            DELETE FROM embedding_chunks;
            DELETE FROM embedding_files;
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
                "SELECT file_hash FROM embedding_files WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query file hash")?;

        Ok(stored_hash.as_deref() != Some(current_hash))
    }

    /// Gets information about a file's embeddings.
    pub fn get_file_info(&self, path: &str) -> Result<Option<FileEmbeddingInfo>> {
        let info = self
            .conn
            .query_row(
                r#"
                SELECT path, file_hash, last_modified, chunk_count
                FROM embedding_files
                WHERE path = ?1
                "#,
                params![path],
                |row| {
                    Ok(FileEmbeddingInfo {
                        path: row.get(0)?,
                        file_hash: row.get(1)?,
                        last_modified: row.get(2)?,
                        chunk_count: row.get(3)?,
                    })
                },
            )
            .optional()
            .context("Failed to query file info")?;

        Ok(info)
    }

    /// Updates or inserts file embedding metadata.
    pub fn upsert_file_info(
        &self,
        path: &str,
        file_hash: &str,
        last_modified: i64,
        chunk_count: u32,
    ) -> Result<()> {
        self.conn
            .execute(
                r#"
            INSERT INTO embedding_files (path, file_hash, last_modified, chunk_count)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                file_hash = excluded.file_hash,
                last_modified = excluded.last_modified,
                chunk_count = excluded.chunk_count
            "#,
                params![path, file_hash, last_modified, chunk_count],
            )
            .context("Failed to upsert file info")?;

        Ok(())
    }

    /// Lists all files that have embeddings.
    pub fn list_files(&self) -> Result<Vec<FileEmbeddingInfo>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT path, file_hash, last_modified, chunk_count
            FROM embedding_files
            ORDER BY path
            "#,
        )?;

        let files = stmt
            .query_map([], |row| {
                Ok(FileEmbeddingInfo {
                    path: row.get(0)?,
                    file_hash: row.get(1)?,
                    last_modified: row.get(2)?,
                    chunk_count: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to list files")?;

        Ok(files)
    }

    /// Finds the chunk containing a specific line in a file.
    pub fn get_chunk_for_line(&self, path: &str, line: u32) -> Result<Option<EmbeddingChunk>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, path, start_line, end_line, content_hash, embedding, created_at
            FROM embedding_chunks
            WHERE path = ?1 AND start_line <= ?2 AND end_line >= ?2
            ORDER BY start_line
            LIMIT 1
            "#,
        )?;

        let chunk = stmt
            .query_row(params![path, line], |row| {
                let embedding_blob: Vec<u8> = row.get(5)?;
                Ok(EmbeddingChunk {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    content_hash: row.get(4)?,
                    embedding: Self::blob_to_embedding(&embedding_blob),
                    created_at: row.get(6)?,
                })
            })
            .optional()
            .context("Failed to query chunk for line")?;

        Ok(chunk)
    }

    /// Performs brute-force similarity search across all embeddings.
    ///
    /// Returns chunks sorted by descending cosine similarity.
    pub fn search_similar(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<SimilarityResult>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, path, start_line, end_line, content_hash, embedding, created_at
            FROM embedding_chunks
            "#,
        )?;

        let mut results: Vec<SimilarityResult> = stmt
            .query_map([], |row| {
                let embedding_blob: Vec<u8> = row.get(5)?;
                let embedding = Self::blob_to_embedding(&embedding_blob);
                let score = Self::cosine_similarity(query_embedding, &embedding);
                Ok(SimilarityResult {
                    chunk: EmbeddingChunk {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        start_line: row.get(2)?,
                        end_line: row.get(3)?,
                        content_hash: row.get(4)?,
                        embedding,
                        created_at: row.get(6)?,
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

    /// Counts total number of embedding chunks.
    pub fn count_chunks(&self) -> Result<u64> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM embedding_chunks", [], |row| {
                    row.get(0)
                })?;
        Ok(count as u64)
    }

    /// Gets metadata value by key.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM embeddings_meta WHERE key = ?1",
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
            INSERT INTO embeddings_meta (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    /// Sets metadata value only if key doesn't exist.
    fn set_meta_if_absent(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO embeddings_meta (key, value) VALUES (?1, ?2)",
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
    fn test_store_and_retrieve_chunk() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        let embedding = create_test_embedding(384, 0.5);
        let id = storage
            .store_chunk("src/main.rs", 1, 40, "abc123", &embedding)
            .unwrap();

        let chunk = storage.get_chunk(id).unwrap().unwrap();
        assert_eq!(chunk.path, "src/main.rs");
        assert_eq!(chunk.start_line, 1);
        assert_eq!(chunk.end_line, 40);
        assert_eq!(chunk.embedding.len(), 384);
    }

    #[test]
    fn test_file_info() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        // Initially no file info
        assert!(storage.get_file_info("src/main.rs").unwrap().is_none());

        // Add file info
        storage
            .upsert_file_info("src/main.rs", "hash1", 1000, 5)
            .unwrap();

        let info = storage.get_file_info("src/main.rs").unwrap().unwrap();
        assert_eq!(info.file_hash, "hash1");
        assert_eq!(info.chunk_count, 5);

        // Update file info
        storage
            .upsert_file_info("src/main.rs", "hash2", 2000, 10)
            .unwrap();

        let info = storage.get_file_info("src/main.rs").unwrap().unwrap();
        assert_eq!(info.file_hash, "hash2");
        assert_eq!(info.chunk_count, 10);
    }

    #[test]
    fn test_file_needs_update() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        // New file always needs update
        assert!(storage.file_needs_update("src/main.rs", "hash1").unwrap());

        // Add file
        storage
            .upsert_file_info("src/main.rs", "hash1", 1000, 1)
            .unwrap();

        // Same hash doesn't need update
        assert!(!storage.file_needs_update("src/main.rs", "hash1").unwrap());

        // Different hash needs update
        assert!(storage.file_needs_update("src/main.rs", "hash2").unwrap());
    }

    #[test]
    fn test_similarity_search() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        // Store test embeddings
        storage
            .store_chunk("a.rs", 1, 10, "h1", &[1.0, 0.0, 0.0])
            .unwrap();
        storage
            .store_chunk("b.rs", 1, 10, "h2", &[0.0, 1.0, 0.0])
            .unwrap();
        storage
            .store_chunk("c.rs", 1, 10, "h3", &[0.9, 0.1, 0.0])
            .unwrap();

        // Query similar to a.rs
        let results = storage.search_similar(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].chunk.path, "a.rs");
        assert!((results[0].score - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity() {
        // Identical vectors
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = EmbeddingStorage::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.0001);

        // Orthogonal vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = EmbeddingStorage::cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.0001);

        // Opposite vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        let sim = EmbeddingStorage::cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 0.0001);
    }

    #[test]
    fn test_delete_file_chunks() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        storage
            .store_chunk("a.rs", 1, 10, "h1", &[1.0, 0.0])
            .unwrap();
        storage
            .store_chunk("a.rs", 11, 20, "h2", &[0.0, 1.0])
            .unwrap();
        storage
            .store_chunk("b.rs", 1, 10, "h3", &[1.0, 1.0])
            .unwrap();
        storage.upsert_file_info("a.rs", "hash", 1000, 2).unwrap();

        let deleted = storage.delete_file_chunks("a.rs").unwrap();
        assert_eq!(deleted, 2);

        assert!(storage.get_file_info("a.rs").unwrap().is_none());
        let a_chunks = storage.get_chunks_for_path("a.rs").unwrap();
        assert!(a_chunks.is_empty());

        let b_chunks = storage.get_chunks_for_path("b.rs").unwrap();
        assert_eq!(b_chunks.len(), 1);
    }

    #[test]
    fn test_get_chunk_for_line() {
        let dir = tempdir().unwrap();
        let storage = EmbeddingStorage::open(dir.path().join("test.sqlite")).unwrap();

        storage.store_chunk("a.rs", 1, 40, "h1", &[1.0]).unwrap();
        storage.store_chunk("a.rs", 21, 60, "h2", &[0.5]).unwrap();
        storage.store_chunk("a.rs", 41, 80, "h3", &[0.0]).unwrap();

        // Line 10 is in chunk 1
        let chunk = storage.get_chunk_for_line("a.rs", 10).unwrap().unwrap();
        assert_eq!(chunk.start_line, 1);

        // Line 30 is in chunks 1 and 2 (overlap), returns first
        let chunk = storage.get_chunk_for_line("a.rs", 30).unwrap().unwrap();
        assert_eq!(chunk.start_line, 1);

        // Line 100 is not in any chunk
        let chunk = storage.get_chunk_for_line("a.rs", 100).unwrap();
        assert!(chunk.is_none());
    }
}
