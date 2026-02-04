// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedding module - handles vector embeddings for semantic search
//!
//! This module provides storage and retrieval of embedding vectors for symbols,
//! enabling semantic similarity search on top of the BM25 text search.

pub mod chunker;
pub mod provider;
pub mod storage;

pub use chunker::{ChunkConfig, EmbeddingChunker, TextChunk};
pub use provider::{
    CommandProvider, DummyProvider, EmbeddingProvider, EmbeddingProviderConfig, FastEmbedder,
};
pub use storage::{
    EmbeddingStorage, SimilarityResult, SymbolEmbedding, SymbolEmbeddingInput,
    DEFAULT_EMBEDDING_DIM,
};
