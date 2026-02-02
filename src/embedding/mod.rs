// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedding module - handles vector embeddings for semantic search
//!
//! This module provides storage and retrieval of embedding vectors for code chunks,
//! enabling semantic similarity search on top of the BM25 text search.

pub mod chunker;
pub mod provider;
pub mod storage;

pub use chunker::{ChunkConfig, EmbeddingChunker, TextChunk};
pub use provider::{EmbeddingProvider, EmbeddingProviderConfig, EmbeddingResult};
pub use storage::{
    EmbeddingChunk, EmbeddingStorage, FileEmbeddingInfo, SimilarityResult, DEFAULT_EMBEDDING_DIM,
};
