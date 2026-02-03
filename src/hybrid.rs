// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hybrid search combining BM25 and embedding-based similarity.
//!
//! This module provides a hybrid search implementation that combines
//! keyword-based BM25 scoring with semantic embedding similarity.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::embedding::EmbeddingStorage;

/// Search mode for queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// BM25 keyword search only
    #[default]
    Keyword,
    /// Embedding-based semantic search only
    Semantic,
    /// Combined BM25 + embedding search
    Hybrid,
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchMode::Keyword => write!(f, "keyword"),
            SearchMode::Semantic => write!(f, "semantic"),
            SearchMode::Hybrid => write!(f, "hybrid"),
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "keyword" | "k" => Ok(SearchMode::Keyword),
            "semantic" | "s" => Ok(SearchMode::Semantic),
            "hybrid" | "h" => Ok(SearchMode::Hybrid),
            _ => Err(format!("Unknown search mode: {}", s)),
        }
    }
}

/// Configuration for hybrid search
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// Weight for text/BM25 score (0.0-1.0)
    pub weight_text: f32,
    /// Weight for vector/embedding score (0.0-1.0)
    pub weight_vector: f32,
    /// Number of BM25 candidates to fetch for reranking
    pub candidate_k: usize,
    /// Maximum results to return
    pub max_results: usize,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            weight_text: 0.7,
            weight_vector: 0.3,
            candidate_k: 200,
            max_results: 20,
        }
    }
}

impl HybridConfig {
    /// Create a new hybrid config with specified weights
    pub fn new(weight_text: f32, weight_vector: f32) -> Self {
        Self {
            weight_text,
            weight_vector,
            ..Default::default()
        }
    }

    /// Set candidate k for BM25 pre-fetching
    pub fn with_candidate_k(mut self, k: usize) -> Self {
        self.candidate_k = k;
        self
    }

    /// Set maximum results
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Calculate candidate_k based on max_results if not explicitly set
    pub fn effective_candidate_k(&self) -> usize {
        if self.candidate_k > 0 {
            self.candidate_k
        } else {
            (self.max_results * 20).clamp(50, 500)
        }
    }
}

/// A BM25 search result from Tantivy
#[derive(Debug, Clone)]
pub struct BM25Result {
    /// File path relative to repository root
    pub path: String,
    /// BM25 score (raw Tantivy score)
    pub score: f32,
    /// Matched snippet
    pub snippet: String,
    /// Line number of match (1-indexed)
    pub line: Option<usize>,
    /// Chunk start line (for embedding lookup)
    pub chunk_start: Option<u32>,
    /// Chunk end line
    pub chunk_end: Option<u32>,
}

/// A hybrid search result with both text and vector scores
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridResult {
    /// File path relative to repository root
    pub path: String,
    /// Final combined score (0.0-1.0)
    pub score: f32,
    /// Raw BM25/text score
    pub text_score: f32,
    /// Raw vector/embedding score (cosine similarity, -1.0 to 1.0)
    pub vector_score: f32,
    /// Normalized text score (0.0-1.0)
    pub text_norm: f32,
    /// Normalized vector score (0.0-1.0)
    pub vector_norm: f32,
    /// Snippet text
    pub snippet: String,
    /// Line number of match (1-indexed)
    pub line: Option<usize>,
    /// Chunk start line (for context)
    pub chunk_start: Option<u32>,
    /// Chunk end line
    pub chunk_end: Option<u32>,
    /// Stable result ID (blake3 hash)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
}

/// Hybrid search engine combining BM25 and embedding search
pub struct HybridSearcher {
    config: HybridConfig,
}

impl HybridSearcher {
    /// Create a new hybrid searcher with the given configuration
    pub fn new(config: HybridConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HybridConfig::default())
    }

    /// Get the configuration
    pub fn config(&self) -> &HybridConfig {
        &self.config
    }

    /// Normalize BM25 scores to 0-1 range
    fn normalize_text_scores(results: &[BM25Result]) -> Vec<f32> {
        if results.is_empty() {
            return Vec::new();
        }

        let max_score = results
            .iter()
            .map(|r| r.score)
            .fold(f32::NEG_INFINITY, f32::max);

        if max_score <= 0.0 {
            return vec![0.0; results.len()];
        }

        results.iter().map(|r| r.score / max_score).collect()
    }

    /// Normalize cosine similarity to 0-1 range
    /// Cosine similarity is in [-1, 1], we map to [0, 1]
    fn normalize_vector_score(cos_sim: f32) -> f32 {
        (cos_sim + 1.0) / 2.0
    }

    /// Combine text and vector scores using configured weights
    fn combine_scores(&self, text_norm: f32, vector_norm: f32) -> f32 {
        self.config.weight_text * text_norm + self.config.weight_vector * vector_norm
    }

    /// Perform hybrid search by reranking BM25 results with embeddings
    pub fn rerank_with_embeddings(
        &self,
        bm25_results: Vec<BM25Result>,
        query_embedding: &[f32],
        storage: &EmbeddingStorage,
    ) -> Result<Vec<HybridResult>> {
        if bm25_results.is_empty() {
            return Ok(Vec::new());
        }

        // Normalize text scores
        let text_norms = Self::normalize_text_scores(&bm25_results);

        // Build hybrid results with embedding lookup
        let mut hybrid_results: Vec<HybridResult> = Vec::with_capacity(bm25_results.len());

        for (i, bm25) in bm25_results.into_iter().enumerate() {
            let text_norm = text_norms[i];

            // Look up embedding for this result's line
            let (vector_score, vector_norm, chunk_start, chunk_end) = if let Some(line) = bm25.line
            {
                match storage.get_chunk_for_line(&bm25.path, line as u32) {
                    Ok(Some(chunk)) => {
                        let cos_sim = Self::cosine_similarity(query_embedding, &chunk.embedding);
                        let norm = Self::normalize_vector_score(cos_sim);
                        (cos_sim, norm, Some(chunk.start_line), Some(chunk.end_line))
                    }
                    _ => (0.0, 0.5, bm25.chunk_start, bm25.chunk_end),
                }
            } else {
                (0.0, 0.5, bm25.chunk_start, bm25.chunk_end)
            };

            let hybrid_score = self.combine_scores(text_norm, vector_norm);

            // Generate stable result ID
            let result_id = Self::generate_result_id(&bm25.path, bm25.line, &bm25.snippet);

            hybrid_results.push(HybridResult {
                path: bm25.path,
                score: hybrid_score,
                text_score: bm25.score,
                vector_score,
                text_norm,
                vector_norm,
                snippet: bm25.snippet,
                line: bm25.line,
                chunk_start,
                chunk_end,
                result_id: Some(result_id),
            });
        }

        // Sort by hybrid score (descending)
        hybrid_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.text_norm
                        .partial_cmp(&a.text_norm)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    b.vector_norm
                        .partial_cmp(&a.vector_norm)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.path.cmp(&b.path))
        });

        // Truncate to max results
        hybrid_results.truncate(self.config.max_results);

        Ok(hybrid_results)
    }

    /// Perform pure semantic search using embeddings only
    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        storage: &EmbeddingStorage,
    ) -> Result<Vec<HybridResult>> {
        let similarity_results =
            storage.search_similar(query_embedding, self.config.max_results)?;

        let results: Vec<HybridResult> = similarity_results
            .into_iter()
            .map(|sim| {
                let vector_norm = Self::normalize_vector_score(sim.score);
                let result_id = Self::generate_result_id(
                    &sim.chunk.path,
                    Some(sim.chunk.start_line as usize),
                    "", // No snippet for semantic search
                );

                HybridResult {
                    path: sim.chunk.path,
                    score: vector_norm, // Score is just the vector score in semantic mode
                    text_score: 0.0,
                    vector_score: sim.score,
                    text_norm: 0.0,
                    vector_norm,
                    snippet: String::new(), // Will be filled later
                    line: Some(sim.chunk.start_line as usize),
                    chunk_start: Some(sim.chunk.start_line),
                    chunk_end: Some(sim.chunk.end_line),
                    result_id: Some(result_id),
                }
            })
            .collect();

        Ok(results)
    }

    /// Generate a stable result ID using blake3 hash
    fn generate_result_id(path: &str, line: Option<usize>, snippet: &str) -> String {
        let input = format!(
            "{}:{}:{}",
            path,
            line.map(|l| l.to_string()).unwrap_or_default(),
            snippet
        );
        let hash = blake3::hash(input.as_bytes());
        hash.to_hex()[..16].to_string()
    }

    /// Compute cosine similarity between two vectors
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

/// Context pack for AI agent consumption
#[derive(Debug, Clone, Serialize)]
pub struct ContextPack {
    /// File path
    pub path: String,
    /// Code blocks with merged ranges
    pub blocks: Vec<ContextBlock>,
}

/// A single code block within a context pack
#[derive(Debug, Clone, Serialize)]
pub struct ContextBlock {
    /// Starting line (1-indexed)
    pub start_line: usize,
    /// Ending line (1-indexed, inclusive)
    pub end_line: usize,
    /// The text content
    pub text: String,
}

/// Builds context packs from search results by merging overlapping ranges
pub struct ContextPackBuilder {
    /// Number of context lines around each match
    context_lines: usize,
}

impl ContextPackBuilder {
    /// Create a new context pack builder
    pub fn new(context_lines: usize) -> Self {
        Self { context_lines }
    }

    /// Build context packs from hybrid results
    pub fn build_from_results(
        &self,
        results: &[HybridResult],
        root: &Path,
    ) -> Result<Vec<ContextPack>> {
        // Group results by file
        let mut file_ranges: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

        for result in results {
            if let Some(line) = result.line {
                let start = line.saturating_sub(self.context_lines);
                let end = line + self.context_lines;
                file_ranges
                    .entry(result.path.clone())
                    .or_default()
                    .push((start, end));
            }
        }

        let mut packs = Vec::new();

        for (path, mut ranges) in file_ranges {
            // Sort and merge overlapping ranges
            ranges.sort_by_key(|r| r.0);
            let merged = Self::merge_ranges(&ranges);

            // Read file and extract blocks
            let file_path = root.join(&path);
            let content = std::fs::read_to_string(&file_path)
                .with_context(|| format!("Failed to read file: {}", path))?;
            let lines: Vec<&str> = content.lines().collect();

            let blocks: Vec<ContextBlock> = merged
                .into_iter()
                .map(|(start, end)| {
                    let actual_start = start.max(1);
                    let actual_end = end.min(lines.len());
                    let text = lines
                        .get(actual_start.saturating_sub(1)..actual_end)
                        .map(|slice| slice.join("\n"))
                        .unwrap_or_default();

                    ContextBlock {
                        start_line: actual_start,
                        end_line: actual_end,
                        text,
                    }
                })
                .collect();

            packs.push(ContextPack { path, blocks });
        }

        Ok(packs)
    }

    /// Merge overlapping ranges
    fn merge_ranges(ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
        if ranges.is_empty() {
            return Vec::new();
        }

        let mut merged = Vec::new();
        let mut current = ranges[0];

        for &(start, end) in &ranges[1..] {
            if start <= current.1 + 1 {
                // Overlapping or adjacent, extend current
                current.1 = current.1.max(end);
            } else {
                // Gap, push current and start new
                merged.push(current);
                current = (start, end);
            }
        }
        merged.push(current);

        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_mode_parsing() {
        assert_eq!(
            "keyword".parse::<SearchMode>().unwrap(),
            SearchMode::Keyword
        );
        assert_eq!(
            "semantic".parse::<SearchMode>().unwrap(),
            SearchMode::Semantic
        );
        assert_eq!("hybrid".parse::<SearchMode>().unwrap(), SearchMode::Hybrid);
        assert_eq!("h".parse::<SearchMode>().unwrap(), SearchMode::Hybrid);
        assert!("invalid".parse::<SearchMode>().is_err());
    }

    #[test]
    fn test_normalize_text_scores() {
        let results = vec![
            BM25Result {
                path: "a.rs".into(),
                score: 10.0,
                snippet: String::new(),
                line: Some(1),
                chunk_start: None,
                chunk_end: None,
            },
            BM25Result {
                path: "b.rs".into(),
                score: 5.0,
                snippet: String::new(),
                line: Some(2),
                chunk_start: None,
                chunk_end: None,
            },
        ];

        let norms = HybridSearcher::normalize_text_scores(&results);
        assert_eq!(norms.len(), 2);
        assert!((norms[0] - 1.0).abs() < 0.001);
        assert!((norms[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_normalize_vector_score() {
        assert!((HybridSearcher::normalize_vector_score(1.0) - 1.0).abs() < 0.001);
        assert!((HybridSearcher::normalize_vector_score(0.0) - 0.5).abs() < 0.001);
        assert!((HybridSearcher::normalize_vector_score(-1.0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_combine_scores() {
        let searcher = HybridSearcher::new(HybridConfig::new(0.7, 0.3));
        let combined = searcher.combine_scores(1.0, 1.0);
        assert!((combined - 1.0).abs() < 0.001);

        let combined = searcher.combine_scores(0.5, 0.5);
        assert!((combined - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_result_id_stability() {
        let id1 = HybridSearcher::generate_result_id("src/main.rs", Some(42), "fn main()");
        let id2 = HybridSearcher::generate_result_id("src/main.rs", Some(42), "fn main()");
        assert_eq!(id1, id2);

        // Different input should produce different ID
        let id3 = HybridSearcher::generate_result_id("src/main.rs", Some(43), "fn main()");
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_merge_ranges() {
        let ranges = vec![(1, 5), (3, 8), (10, 15)];
        let merged = ContextPackBuilder::merge_ranges(&ranges);
        assert_eq!(merged, vec![(1, 8), (10, 15)]);

        // Adjacent ranges
        let ranges = vec![(1, 5), (6, 10)];
        let merged = ContextPackBuilder::merge_ranges(&ranges);
        assert_eq!(merged, vec![(1, 10)]);

        // Empty
        let merged = ContextPackBuilder::merge_ranges(&[]);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((HybridSearcher::cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(HybridSearcher::cosine_similarity(&a, &c).abs() < 0.001);
    }
}
