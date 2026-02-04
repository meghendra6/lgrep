// SPDX-License-Identifier: MIT OR Apache-2.0

//! Embedding provider interface and implementations.
//!
//! This module provides a fastembed-based provider optimized for CPU throughput.

use anyhow::{bail, Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde_json::Value;
use std::borrow::Cow;
use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

const DEFAULT_FASTEMBED_MODEL: &str = "minilm";
const DEFAULT_FASTEMBED_BATCH_SIZE: usize = 512;
const MAX_FASTEMBED_BATCH_SIZE: usize = 1024;
const DEFAULT_FASTEMBED_MAX_CHARS: usize = 2000;
const DEFAULT_COMMAND_BATCH_SIZE: usize = 64;

/// Configuration for the embedding provider.
#[derive(Debug, Clone)]
pub struct EmbeddingProviderConfig {
    pub model: EmbeddingModel,
    pub batch_size: usize,
    pub max_chars: usize,
    pub normalize: bool,
}

impl EmbeddingProviderConfig {
    pub fn from_env() -> Result<Self> {
        let model = parse_model_env()?;
        let mut batch_size = parse_usize_env("FASTEMBED_BATCH_SIZE", DEFAULT_FASTEMBED_BATCH_SIZE)?;
        if batch_size == 0 {
            batch_size = DEFAULT_FASTEMBED_BATCH_SIZE;
        }
        if batch_size > MAX_FASTEMBED_BATCH_SIZE {
            eprintln!(
                "Warning: FASTEMBED_BATCH_SIZE={} exceeds max {}; clamping.",
                batch_size, MAX_FASTEMBED_BATCH_SIZE
            );
            batch_size = MAX_FASTEMBED_BATCH_SIZE;
        }

        let mut max_chars = parse_usize_env("FASTEMBED_MAX_CHARS", DEFAULT_FASTEMBED_MAX_CHARS)?;
        if max_chars == 0 {
            max_chars = DEFAULT_FASTEMBED_MAX_CHARS;
        }

        let normalize = parse_bool_env("FASTEMBED_NORMALIZE", true)?;

        Ok(Self {
            model,
            batch_size,
            max_chars,
            normalize,
        })
    }

    pub fn has_env_overrides() -> bool {
        env::var_os("FASTEMBED_MODEL").is_some()
            || env::var_os("FASTEMBED_BATCH_SIZE").is_some()
            || env::var_os("FASTEMBED_MAX_CHARS").is_some()
            || env::var_os("FASTEMBED_NORMALIZE").is_some()
    }
}

impl Default for EmbeddingProviderConfig {
    fn default() -> Self {
        Self {
            model: EmbeddingModel::AllMiniLML6V2,
            batch_size: DEFAULT_FASTEMBED_BATCH_SIZE,
            max_chars: DEFAULT_FASTEMBED_MAX_CHARS,
            normalize: true,
        }
    }
}

/// Trait for embedding providers.
pub trait EmbeddingProvider: Send {
    /// Returns the model identifier.
    fn model_id(&self) -> &str;

    /// Returns the batch size used by the provider.
    fn batch_size(&self) -> usize;

    /// Generates embeddings for the given texts.
    fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Generates an embedding for a single text.
    fn embed_one(&mut self, text: &str) -> Result<Vec<f32>> {
        let mut result = self.embed_texts(&[text.to_string()])?;
        result
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }
}

/// FastEmbed provider using sentence-transformers/all-MiniLM-L6-v2.
pub struct FastEmbedder {
    embedder: TextEmbedding,
    config: EmbeddingProviderConfig,
    model_id: String,
}

impl FastEmbedder {
    pub fn new(config: EmbeddingProviderConfig) -> Result<Self> {
        let model = config.model.clone();
        let model_id = model.to_string();
        let init = InitOptions::new(model);
        let embedder =
            TextEmbedding::try_new(init).context("Failed to initialize fastembed model")?;

        Ok(Self {
            embedder,
            config,
            model_id,
        })
    }

    pub fn from_env() -> Result<Self> {
        Self::new(EmbeddingProviderConfig::from_env()?)
    }
}

impl EmbeddingProvider for FastEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let prepared = truncate_texts(texts, self.config.max_chars);
        let mut embeddings = self
            .embedder
            .embed(&prepared, Some(self.config.batch_size))?;

        if self.config.normalize {
            for embedding in embeddings.iter_mut() {
                l2_normalize(embedding);
            }
        }

        Ok(embeddings)
    }
}

/// Command provider that shells out to an external process.
pub struct CommandProvider {
    command: String,
    model: String,
    batch_size: usize,
}

impl CommandProvider {
    pub fn new(command: String, model: String) -> Self {
        Self {
            command,
            model,
            batch_size: DEFAULT_COMMAND_BATCH_SIZE,
        }
    }

    fn run_command(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let payload = serde_json::json!({
            "model": self.model,
            "texts": texts,
        });

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn embedding command: {}", self.command))?;

        if let Some(mut stdin) = child.stdin.take() {
            let payload_str = payload.to_string();
            stdin
                .write_all(payload_str.as_bytes())
                .context("Failed to write embeddings payload to stdin")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to read embeddings command output")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Embedding command failed (status {}): {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: Value = serde_json::from_str(stdout.trim())
            .with_context(|| "Failed to parse embeddings command output as JSON")?;

        let embeddings_value = match parsed {
            Value::Array(arr) => Value::Array(arr),
            Value::Object(ref obj) => {
                if let Some(value) = obj.get("embeddings") {
                    value.clone()
                } else if let Some(value) = obj.get("vectors") {
                    value.clone()
                } else if let Some(value) = obj.get("data") {
                    value.clone()
                } else {
                    bail!("Embeddings command output missing 'embeddings' field");
                }
            }
            _ => bail!("Embeddings command output must be JSON array or object"),
        };

        let vectors = embeddings_value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Embeddings output must be a JSON array"))?
            .iter()
            .map(|row| {
                row.as_array()
                    .ok_or_else(|| anyhow::anyhow!("Embedding row must be an array"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .ok_or_else(|| anyhow::anyhow!("Embedding value must be a number"))
                            .map(|v| v as f32)
                    })
                    .collect::<Result<Vec<f32>>>()
            })
            .collect::<Result<Vec<Vec<f32>>>>()?;

        Ok(vectors)
    }
}

impl EmbeddingProvider for CommandProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn batch_size(&self) -> usize {
        self.batch_size
    }

    fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.run_command(texts)
    }
}

/// Dummy provider that returns zero vectors (for testing/fallback).
pub struct DummyProvider {
    model: String,
    dimension: usize,
    batch_size: usize,
}

impl DummyProvider {
    /// Creates a new dummy provider with specified dimension.
    pub fn new(dimension: usize) -> Self {
        Self {
            model: "dummy".to_string(),
            dimension,
            batch_size: DEFAULT_FASTEMBED_BATCH_SIZE,
        }
    }
}

impl EmbeddingProvider for DummyProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn batch_size(&self) -> usize {
        self.batch_size
    }

    fn embed_texts(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let vectors: Vec<Vec<f32>> = texts.iter().map(|_| vec![0.0; self.dimension]).collect();

        Ok(vectors)
    }
}

fn truncate_texts<'a>(texts: &'a [String], max_chars: usize) -> Vec<Cow<'a, str>> {
    texts
        .iter()
        .map(|text| truncate_to_chars(text.as_str(), max_chars))
        .collect()
}

fn truncate_to_chars<'a>(input: &'a str, max_chars: usize) -> Cow<'a, str> {
    if max_chars == 0 {
        return Cow::Borrowed("");
    }

    let mut count = 0;
    for (idx, _) in input.char_indices() {
        if count == max_chars {
            return Cow::Owned(input[..idx].to_string());
        }
        count += 1;
    }

    Cow::Borrowed(input)
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return;
    }
    for value in vector.iter_mut() {
        *value /= norm;
    }
}

fn parse_model_env() -> Result<EmbeddingModel> {
    let raw = env::var("FASTEMBED_MODEL").unwrap_or_else(|_| DEFAULT_FASTEMBED_MODEL.to_string());
    let value = raw.trim();
    if value.is_empty() {
        return Ok(EmbeddingModel::AllMiniLML6V2);
    }

    match value.to_lowercase().as_str() {
        "minilm"
        | "all-minilm-l6-v2"
        | "allminilm-l6-v2"
        | "sentence-transformers/all-minilm-l6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
        other => bail!(
            "Unsupported FASTEMBED_MODEL '{}'. Supported value: {}",
            other,
            DEFAULT_FASTEMBED_MODEL
        ),
    }
}

fn parse_usize_env(name: &str, default: usize) -> Result<usize> {
    match env::var(name) {
        Ok(raw) => {
            let value = raw.trim();
            if value.is_empty() {
                Ok(default)
            } else {
                value
                    .parse::<usize>()
                    .with_context(|| format!("Invalid {} value: {}", name, value))
            }
        }
        Err(env::VarError::NotPresent) => Ok(default),
        Err(err) => Err(err).with_context(|| format!("Failed to read {}", name)),
    }
}

fn parse_bool_env(name: &str, default: bool) -> Result<bool> {
    match env::var(name) {
        Ok(raw) => {
            let value = raw.trim().to_lowercase();
            if value.is_empty() {
                return Ok(default);
            }
            match value.as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                other => bail!("Invalid {} value: {}", name, other),
            }
        }
        Err(env::VarError::NotPresent) => Ok(default),
        Err(err) => Err(err).with_context(|| format!("Failed to read {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dummy_provider() {
        let mut provider = DummyProvider::new(384);
        assert_eq!(provider.model_id(), "dummy");

        let result = provider
            .embed_texts(&["hello".to_string(), "world".to_string()])
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 384);
        assert!(result[0].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_empty_embed() {
        let mut provider = DummyProvider::new(384);
        let result = provider.embed_texts(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_embed_one() {
        let mut provider = DummyProvider::new(128);
        let vector = provider.embed_one("test").unwrap();
        assert_eq!(vector.len(), 128);
    }

    #[test]
    fn test_truncate_to_chars() {
        let input = "hello";
        assert_eq!(
            truncate_to_chars(input, 2),
            Cow::<str>::Owned("he".to_string())
        );
        assert_eq!(truncate_to_chars(input, 5), Cow::Borrowed(input));
    }
}
