// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedding provider interface and implementations.
//!
//! This module provides a trait for embedding generation and implements
//! the command-based provider that calls an external embedding service.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

/// Default model ID when not specified.
pub const DEFAULT_MODEL_ID: &str = "local-model";

/// Configuration for the embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderConfig {
    /// Provider type: "command" or "builtin"
    pub provider: String,
    /// Model identifier
    pub model: String,
    /// Command to execute (for command provider)
    pub command: Option<String>,
    /// Whether to normalize embeddings
    #[serde(default = "default_true")]
    pub normalize: bool,
}

fn default_true() -> bool {
    true
}

impl Default for EmbeddingProviderConfig {
    fn default() -> Self {
        Self {
            provider: "command".to_string(),
            model: DEFAULT_MODEL_ID.to_string(),
            command: None,
            normalize: true,
        }
    }
}

/// Request format for the command provider.
#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    texts: Vec<String>,
    normalize: bool,
}

/// Response format from the command provider.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    #[allow(dead_code)]
    model: String,
    dimension: usize,
    vectors: Vec<Vec<f32>>,
}

/// Result of embedding generation.
#[derive(Debug, Clone)]
pub struct EmbeddingResult {
    /// Model identifier used
    pub model: String,
    /// Embedding dimension
    pub dimension: usize,
    /// Generated embedding vectors (one per input text)
    pub vectors: Vec<Vec<f32>>,
}

/// Trait for embedding providers.
pub trait EmbeddingProvider: Send + Sync {
    /// Returns the model identifier.
    fn model(&self) -> &str;

    /// Returns the embedding dimension.
    fn dimension(&self) -> Option<usize>;

    /// Generates embeddings for the given texts.
    fn embed(&self, texts: &[String]) -> Result<EmbeddingResult>;

    /// Generates an embedding for a single text.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let result = self.embed(&[text.to_string()])?;
        result
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }
}

/// Command-based embedding provider.
///
/// Calls an external command to generate embeddings.
/// The command receives JSON input on stdin and outputs JSON on stdout.
///
/// Input format:
/// ```json
/// {"model": "<id>", "texts": ["...", "..."], "normalize": true}
/// ```
///
/// Output format:
/// ```json
/// {"model": "<id>", "dimension": <n>, "vectors": [[f32...], [f32...]]}
/// ```
pub struct CommandProvider {
    command: String,
    model: String,
    normalize: bool,
    cached_dimension: std::sync::Mutex<Option<usize>>,
}

impl CommandProvider {
    /// Creates a new command provider.
    pub fn new(command: String, model: String, normalize: bool) -> Self {
        Self {
            command,
            model,
            normalize,
            cached_dimension: std::sync::Mutex::new(None),
        }
    }

    /// Creates a command provider from configuration.
    pub fn from_config(config: &EmbeddingProviderConfig) -> Result<Self> {
        let command = config
            .command
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Command provider requires 'command' field"))?
            .clone();
        Ok(Self::new(command, config.model.clone(), config.normalize))
    }

    /// Checks if the command is available.
    pub fn is_available(&self) -> bool {
        // Try to find the command in PATH
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        if parts.is_empty() {
            return false;
        }
        which::which(parts[0]).is_ok()
    }
}

impl EmbeddingProvider for CommandProvider {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimension(&self) -> Option<usize> {
        *self.cached_dimension.lock().unwrap()
    }

    fn embed(&self, texts: &[String]) -> Result<EmbeddingResult> {
        if texts.is_empty() {
            return Ok(EmbeddingResult {
                model: self.model.clone(),
                dimension: 0,
                vectors: Vec::new(),
            });
        }

        let request = EmbeddingRequest {
            model: self.model.clone(),
            texts: texts.to_vec(),
            normalize: self.normalize,
        };

        let request_json =
            serde_json::to_string(&request).context("Failed to serialize embedding request")?;

        // Parse command with arguments
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        if parts.is_empty() {
            bail!("Empty command");
        }

        let mut cmd = Command::new(parts[0]);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn command: {}", self.command))?;

        // Write request to stdin
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Failed to open stdin"))?;
            stdin
                .write_all(request_json.as_bytes())
                .context("Failed to write to stdin")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to wait for command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Embedding command failed: {}", stderr);
        }

        let response: EmbeddingResponse =
            serde_json::from_slice(&output.stdout).with_context(|| {
                let stdout = String::from_utf8_lossy(&output.stdout);
                format!("Failed to parse embedding response: {}", stdout)
            })?;

        // Cache the dimension
        *self.cached_dimension.lock().unwrap() = Some(response.dimension);

        Ok(EmbeddingResult {
            model: self.model.clone(),
            dimension: response.dimension,
            vectors: response.vectors,
        })
    }
}

/// Dummy provider that returns zero vectors (for testing/fallback).
pub struct DummyProvider {
    model: String,
    dimension: usize,
}

impl DummyProvider {
    /// Creates a new dummy provider with specified dimension.
    pub fn new(dimension: usize) -> Self {
        Self {
            model: "dummy".to_string(),
            dimension,
        }
    }
}

impl EmbeddingProvider for DummyProvider {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimension(&self) -> Option<usize> {
        Some(self.dimension)
    }

    fn embed(&self, texts: &[String]) -> Result<EmbeddingResult> {
        let vectors: Vec<Vec<f32>> = texts.iter().map(|_| vec![0.0; self.dimension]).collect();

        Ok(EmbeddingResult {
            model: self.model.clone(),
            dimension: self.dimension,
            vectors,
        })
    }
}

/// Creates an embedding provider based on configuration.
pub fn create_provider(config: &EmbeddingProviderConfig) -> Result<Box<dyn EmbeddingProvider>> {
    match config.provider.as_str() {
        "command" => {
            let provider = CommandProvider::from_config(config)?;
            if !provider.is_available() {
                bail!(
                    "Embedding command '{}' not found in PATH. \
                    Semantic search requires an embedding provider. \
                    See documentation for setup instructions.",
                    config.command.as_deref().unwrap_or("(not set)")
                );
            }
            Ok(Box::new(provider))
        }
        "dummy" => Ok(Box::new(DummyProvider::new(384))),
        other => {
            bail!("Unknown embedding provider type: {}", other);
        }
    }
}

/// Streaming embedding provider that processes texts in batches.
pub struct BatchingProvider<P: EmbeddingProvider> {
    inner: P,
    batch_size: usize,
}

impl<P: EmbeddingProvider> BatchingProvider<P> {
    /// Creates a new batching provider.
    pub fn new(inner: P, batch_size: usize) -> Self {
        Self { inner, batch_size }
    }

    /// Embeds texts in batches and returns all results.
    pub fn embed_batched(&self, texts: &[String]) -> Result<EmbeddingResult> {
        if texts.is_empty() {
            return Ok(EmbeddingResult {
                model: self.inner.model().to_string(),
                dimension: self.inner.dimension().unwrap_or(0),
                vectors: Vec::new(),
            });
        }

        let mut all_vectors = Vec::with_capacity(texts.len());
        let mut dimension = 0;

        for batch in texts.chunks(self.batch_size) {
            let result = self.inner.embed(batch)?;
            dimension = result.dimension;
            all_vectors.extend(result.vectors);
        }

        Ok(EmbeddingResult {
            model: self.inner.model().to_string(),
            dimension,
            vectors: all_vectors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dummy_provider() {
        let provider = DummyProvider::new(384);
        assert_eq!(provider.model(), "dummy");
        assert_eq!(provider.dimension(), Some(384));

        let result = provider
            .embed(&["hello".to_string(), "world".to_string()])
            .unwrap();
        assert_eq!(result.vectors.len(), 2);
        assert_eq!(result.vectors[0].len(), 384);
        assert!(result.vectors[0].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_empty_embed() {
        let provider = DummyProvider::new(384);
        let result = provider.embed(&[]).unwrap();
        assert!(result.vectors.is_empty());
    }

    #[test]
    fn test_embed_one() {
        let provider = DummyProvider::new(128);
        let vector = provider.embed_one("test").unwrap();
        assert_eq!(vector.len(), 128);
    }

    #[test]
    fn test_batching_provider() {
        let inner = DummyProvider::new(64);
        let batching = BatchingProvider::new(inner, 2);

        let texts: Vec<String> = (0..5).map(|i| format!("text {}", i)).collect();
        let result = batching.embed_batched(&texts).unwrap();

        assert_eq!(result.vectors.len(), 5);
        assert_eq!(result.dimension, 64);
    }

    #[test]
    fn test_default_config() {
        let config = EmbeddingProviderConfig::default();
        assert_eq!(config.provider, "command");
        assert!(config.normalize);
    }
}
