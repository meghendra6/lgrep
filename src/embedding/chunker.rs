// SPDX-License-Identifier: MIT OR Apache-2.0

//! Text chunker for embedding generation.
//!
//! This module splits source code into overlapping chunks suitable for
//! embedding generation. The chunking strategy uses line-based sliding
//! windows with configurable overlap.

use anyhow::{bail, Result};

/// Default number of lines per chunk.
pub const DEFAULT_CHUNK_LINES: usize = 80;

/// Default overlap between consecutive chunks.
pub const DEFAULT_CHUNK_OVERLAP: usize = 20;

/// Minimum chunk size in characters (chunks smaller than this are skipped).
pub const MIN_CHUNK_SIZE: usize = 30;

/// Maximum file size in bytes for embedding generation.
pub const DEFAULT_MAX_FILE_BYTES: usize = 2_000_000;

/// Configuration for the text chunker.
#[derive(Debug, Clone)]
pub struct ChunkConfig {
    /// Number of lines per chunk.
    pub chunk_lines: usize,
    /// Number of overlapping lines between consecutive chunks.
    pub chunk_overlap: usize,
    /// Minimum chunk size in characters.
    pub min_chunk_size: usize,
    /// Maximum file size in bytes.
    pub max_file_bytes: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            chunk_lines: DEFAULT_CHUNK_LINES,
            chunk_overlap: DEFAULT_CHUNK_OVERLAP,
            min_chunk_size: MIN_CHUNK_SIZE,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

impl ChunkConfig {
    /// Creates a new ChunkConfig with the specified parameters.
    pub fn new(chunk_lines: usize, chunk_overlap: usize) -> Result<Self> {
        if chunk_overlap >= chunk_lines {
            bail!("chunk_overlap ({}) must be less than chunk_lines ({})", chunk_overlap, chunk_lines);
        }
        if chunk_lines == 0 {
            bail!("chunk_lines must be greater than 0");
        }
        Ok(Self {
            chunk_lines,
            chunk_overlap,
            ..Default::default()
        })
    }

    /// Sets the minimum chunk size.
    pub fn with_min_chunk_size(mut self, size: usize) -> Self {
        self.min_chunk_size = size;
        self
    }

    /// Sets the maximum file size.
    pub fn with_max_file_bytes(mut self, size: usize) -> Self {
        self.max_file_bytes = size;
        self
    }
}

/// Represents a text chunk with its position information.
#[derive(Debug, Clone)]
pub struct TextChunk {
    /// Starting line number (1-indexed).
    pub start_line: u32,
    /// Ending line number (1-indexed, inclusive).
    pub end_line: u32,
    /// The chunk text content.
    pub text: String,
}

/// Splits text into overlapping chunks for embedding generation.
pub struct EmbeddingChunker {
    config: ChunkConfig,
}

impl EmbeddingChunker {
    /// Creates a new chunker with the given configuration.
    pub fn new(config: ChunkConfig) -> Self {
        Self { config }
    }

    /// Creates a chunker with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ChunkConfig::default())
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &ChunkConfig {
        &self.config
    }

    /// Checks if a file is too large for embedding.
    pub fn is_file_too_large(&self, content: &str) -> bool {
        content.len() > self.config.max_file_bytes
    }

    /// Splits text into overlapping chunks.
    ///
    /// Algorithm:
    /// ```text
    /// start = 1
    /// step = chunk_lines - chunk_overlap
    /// while start <= total_lines:
    ///   end = min(start + chunk_lines - 1, total_lines)
    ///   chunk_text = lines[start..end].join("\n")
    ///   store (path, start, end, chunk_text)
    ///   start += step
    /// ```
    ///
    /// Chunks smaller than `min_chunk_size` characters are skipped.
    pub fn chunk_text(&self, content: &str) -> Vec<TextChunk> {
        if content.is_empty() {
            return Vec::new();
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        if total_lines == 0 {
            return Vec::new();
        }

        let step = self.config.chunk_lines.saturating_sub(self.config.chunk_overlap).max(1);
        let mut chunks = Vec::new();
        let mut start = 0_usize; // 0-indexed for slicing

        while start < total_lines {
            let end = (start + self.config.chunk_lines).min(total_lines);
            let chunk_text = lines[start..end].join("\n");

            // Skip small chunks
            if chunk_text.len() >= self.config.min_chunk_size {
                chunks.push(TextChunk {
                    start_line: (start + 1) as u32, // Convert to 1-indexed
                    end_line: end as u32,
                    text: chunk_text,
                });
            }

            start += step;

            // Prevent infinite loop when step would not advance
            if step == 0 {
                break;
            }
        }

        chunks
    }

    /// Chunks multiple files and returns results keyed by path.
    pub fn chunk_files<'a, I>(&self, files: I) -> Vec<(String, Vec<TextChunk>)>
    where
        I: Iterator<Item = (&'a str, &'a str)>,
    {
        files
            .filter(|(_, content)| !self.is_file_too_large(content))
            .map(|(path, content)| (path.to_string(), self.chunk_text(content)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ChunkConfig::default();
        assert_eq!(config.chunk_lines, 80);
        assert_eq!(config.chunk_overlap, 20);
    }

    #[test]
    fn test_config_validation() {
        // Valid config
        assert!(ChunkConfig::new(80, 20).is_ok());

        // Invalid: overlap >= lines
        assert!(ChunkConfig::new(20, 20).is_err());
        assert!(ChunkConfig::new(20, 30).is_err());

        // Invalid: zero lines
        assert!(ChunkConfig::new(0, 0).is_err());
    }

    #[test]
    fn test_empty_content() {
        let chunker = EmbeddingChunker::with_defaults();
        assert!(chunker.chunk_text("").is_empty());
    }

    #[test]
    fn test_single_line() {
        let chunker = EmbeddingChunker::new(
            ChunkConfig::new(5, 2).unwrap().with_min_chunk_size(1)
        );
        let chunks = chunker.chunk_text("hello world");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 1);
        assert_eq!(chunks[0].text, "hello world");
    }

    #[test]
    fn test_multiple_chunks() {
        let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let chunker = EmbeddingChunker::new(
            ChunkConfig::new(4, 1).unwrap().with_min_chunk_size(1)
        );

        let chunks = chunker.chunk_text(&content);

        // With 10 lines, chunk_lines=4, overlap=1, step=3
        // Chunks: [1-4], [4-7], [7-10], [10-10]
        assert_eq!(chunks.len(), 4);

        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 4);

        assert_eq!(chunks[1].start_line, 4);
        assert_eq!(chunks[1].end_line, 7);

        assert_eq!(chunks[2].start_line, 7);
        assert_eq!(chunks[2].end_line, 10);

        assert_eq!(chunks[3].start_line, 10);
        assert_eq!(chunks[3].end_line, 10);
    }

    #[test]
    fn test_small_chunk_filtering() {
        let content = "ab\ncd\nef";
        let chunker = EmbeddingChunker::new(
            ChunkConfig::new(2, 0).unwrap().with_min_chunk_size(10)
        );

        let chunks = chunker.chunk_text(content);
        // Each chunk would be 5 chars ("ab\ncd"), below min_chunk_size of 10
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_large_file_detection() {
        let chunker = EmbeddingChunker::new(
            ChunkConfig::default().with_max_file_bytes(100)
        );

        let small = "x".repeat(50);
        let large = "x".repeat(200);

        assert!(!chunker.is_file_too_large(&small));
        assert!(chunker.is_file_too_large(&large));
    }

    #[test]
    fn test_overlapping_content() {
        let content = "1\n2\n3\n4\n5\n6";
        let chunker = EmbeddingChunker::new(
            ChunkConfig::new(3, 1).unwrap().with_min_chunk_size(1)
        );

        let chunks = chunker.chunk_text(content);
        // Chunks: [1-3] (lines 1,2,3), [3-5] (lines 3,4,5), [5-6] (lines 5,6)
        assert_eq!(chunks.len(), 3);

        // Verify overlap: line 3 appears in chunks 0 and 1
        assert!(chunks[0].text.contains("3"));
        assert!(chunks[1].text.contains("3"));

        // Verify overlap: line 5 appears in chunks 1 and 2
        assert!(chunks[1].text.contains("5"));
        assert!(chunks[2].text.contains("5"));
    }
}
