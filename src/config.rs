// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration file support for cgrep
//!
//! Loads configuration from .cgreprc.toml in current directory or ~/.config/cgrep/config.toml

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Output format for results (mirrored from cli for library use)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigOutputFormat {
    #[default]
    Text,
    Json,
    Json2,
}

/// Search mode for queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Keyword,
    Semantic,
    Hybrid,
}

/// Embedding feature enablement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingEnabled {
    Off,
    #[default]
    Auto,
    On,
}

/// Embedding provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProviderType {
    #[default]
    Command,
    Builtin,
    Dummy,
}

/// Search configuration
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Default search mode (keyword, semantic, hybrid)
    pub default_mode: Option<SearchMode>,
    /// Number of candidates to fetch for reranking in hybrid mode
    pub candidate_k: Option<usize>,
    /// Weight for text/keyword scoring in hybrid mode (0.0-1.0)
    pub weight_text: Option<f32>,
    /// Weight for vector/semantic scoring in hybrid mode (0.0-1.0)
    pub weight_vector: Option<f32>,
}

impl SearchConfig {
    /// Get default search mode (defaults to Keyword)
    pub fn mode(&self) -> SearchMode {
        self.default_mode.unwrap_or_default()
    }

    /// Get candidate k for hybrid search (defaults to 200)
    pub fn candidate_k(&self) -> usize {
        self.candidate_k.unwrap_or(200)
    }

    /// Get text weight for hybrid scoring (defaults to 0.7)
    pub fn weight_text(&self) -> f32 {
        self.weight_text.unwrap_or(0.7)
    }

    /// Get vector weight for hybrid scoring (defaults to 0.3)
    pub fn weight_vector(&self) -> f32 {
        self.weight_vector.unwrap_or(0.3)
    }
}

/// Embedding configuration
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// Whether embeddings are enabled (off, auto, on)
    pub enabled: Option<EmbeddingEnabled>,
    /// Provider type (command, builtin, dummy)
    pub provider: Option<EmbeddingProviderType>,
    /// Model identifier for the embedding provider
    pub model: Option<String>,
    /// Command to execute for command provider
    pub command: Option<String>,
    /// Number of lines per chunk
    pub chunk_lines: Option<usize>,
    /// Number of overlap lines between chunks
    pub chunk_overlap: Option<usize>,
    /// Maximum file size in bytes to process
    pub max_file_bytes: Option<usize>,
    /// Maximum number of chunks for semantic search
    pub semantic_max_chunks: Option<usize>,
}

impl EmbeddingConfig {
    /// Get enabled mode (defaults to Auto)
    pub fn enabled(&self) -> EmbeddingEnabled {
        self.enabled.unwrap_or_default()
    }

    /// Get provider type (defaults to Command)
    pub fn provider(&self) -> EmbeddingProviderType {
        self.provider.unwrap_or_default()
    }

    /// Get model identifier (defaults to "local-model-id")
    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("local-model-id")
    }

    /// Get command (defaults to "embedder")
    pub fn command(&self) -> &str {
        self.command.as_deref().unwrap_or("embedder")
    }

    /// Get chunk lines (defaults to 80)
    pub fn chunk_lines(&self) -> usize {
        self.chunk_lines.unwrap_or(80)
    }

    /// Get chunk overlap (defaults to 20)
    pub fn chunk_overlap(&self) -> usize {
        self.chunk_overlap.unwrap_or(20)
    }

    /// Get max file bytes (defaults to 2MB)
    pub fn max_file_bytes(&self) -> usize {
        self.max_file_bytes.unwrap_or(2_000_000)
    }

    /// Get semantic max chunks (defaults to 200000)
    pub fn semantic_max_chunks(&self) -> usize {
        self.semantic_max_chunks.unwrap_or(200_000)
    }
}

/// Indexing configuration
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    /// Paths/patterns to exclude from indexing
    pub exclude_paths: Vec<String>,
    /// Maximum file size in bytes to index (default: 1MB)
    pub max_file_size: Option<u64>,
}

impl IndexConfig {
    /// Get exclude paths
    pub fn exclude_paths(&self) -> &[String] {
        &self.exclude_paths
    }

    /// Get max file size (default: 1MB)
    pub fn max_file_size(&self) -> u64 {
        self.max_file_size.unwrap_or(1024 * 1024)
    }
}

/// Cache configuration
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Whether caching is enabled
    pub enabled: Option<bool>,
    /// Cache TTL in milliseconds
    pub ttl_ms: Option<u64>,
}

impl CacheConfig {
    /// Get enabled (defaults to true)
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// Get TTL in milliseconds (defaults to 600000 = 10 minutes)
    pub fn ttl_ms(&self) -> u64 {
        self.ttl_ms.unwrap_or(600_000)
    }
}

/// Profile configuration for different usage modes
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    /// Output format for this profile
    pub format: Option<ConfigOutputFormat>,
    /// Context lines around matches
    pub context: Option<usize>,
    /// Context pack size for agent mode
    pub context_pack: Option<usize>,
    /// Maximum results for this profile
    pub max_results: Option<usize>,
    /// Search mode for this profile
    pub mode: Option<SearchMode>,
    /// Whether to use agent caching (for agent profile)
    pub agent_cache: Option<bool>,
}

impl ProfileConfig {
    /// Create the "human" profile preset
    pub fn human() -> Self {
        Self {
            format: Some(ConfigOutputFormat::Text),
            context: Some(2),
            context_pack: None,
            max_results: Some(20),
            mode: Some(SearchMode::Keyword),
            agent_cache: None,
        }
    }

    /// Create the "agent" profile preset
    pub fn agent() -> Self {
        Self {
            format: Some(ConfigOutputFormat::Json2),
            context: Some(6),
            context_pack: Some(8),
            max_results: Some(50),
            mode: Some(SearchMode::Hybrid),
            agent_cache: Some(true),
        }
    }

    /// Create the "fast" profile preset (for quick exploration)
    pub fn fast() -> Self {
        Self {
            format: Some(ConfigOutputFormat::Text),
            context: Some(0),
            context_pack: None,
            max_results: Some(10),
            mode: Some(SearchMode::Keyword),
            agent_cache: None,
        }
    }

    /// Get format (defaults to Text)
    pub fn format(&self) -> ConfigOutputFormat {
        self.format.unwrap_or_default()
    }

    /// Get context lines (defaults to 2)
    pub fn context(&self) -> usize {
        self.context.unwrap_or(2)
    }

    /// Get context pack size (defaults to context value)
    pub fn context_pack(&self) -> usize {
        self.context_pack.unwrap_or_else(|| self.context())
    }

    /// Get max results (defaults to 20)
    pub fn max_results(&self) -> usize {
        self.max_results.unwrap_or(20)
    }

    /// Get search mode (defaults to Keyword)
    pub fn mode(&self) -> SearchMode {
        self.mode.unwrap_or_default()
    }

    /// Get agent cache setting (defaults to false)
    pub fn agent_cache(&self) -> bool {
        self.agent_cache.unwrap_or(false)
    }
}

/// Configuration loaded from .cgreprc.toml or ~/.config/cgrep/config.toml
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Maximum number of results to return
    pub max_results: Option<usize>,
    /// Default output format (text or json)
    pub default_format: Option<String>,
    /// Patterns to exclude from search
    pub exclude_patterns: Vec<String>,

    /// Search configuration
    #[serde(default)]
    pub search: SearchConfig,

    /// Embedding configuration
    #[serde(default)]
    pub embeddings: EmbeddingConfig,

    /// Cache configuration
    #[serde(default)]
    pub cache: CacheConfig,

    /// Index configuration
    #[serde(default)]
    pub index: IndexConfig,

    /// Named profiles (e.g., "human", "agent", "fast")
    #[serde(default, rename = "profile")]
    pub profiles: HashMap<String, ProfileConfig>,
}

impl Config {
    /// Load configuration from files
    ///
    /// Precedence (highest to lowest):
    /// 1. .cgreprc.toml in current directory
    /// 2. ~/.config/cgrep/config.toml
    pub fn load() -> Self {
        // Try current directory first
        if let Some(config) = Self::load_from_path(&PathBuf::from(".cgreprc.toml")) {
            return config;
        }

        // Try home directory config
        if let Some(home) = dirs::home_dir() {
            let config_path = home.join(".config").join("cgrep").join("config.toml");
            if let Some(config) = Self::load_from_path(&config_path) {
                return config;
            }
        }

        Self::default()
    }

    fn load_from_path(path: &PathBuf) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("Warning: Failed to parse {}: {}", path.display(), e);
                None
            }
        }
    }

    /// Get output format from config, parsing the string to ConfigOutputFormat
    pub fn output_format(&self) -> Option<ConfigOutputFormat> {
        self.default_format.as_ref().and_then(|s| match s.to_lowercase().as_str() {
            "json" => Some(ConfigOutputFormat::Json),
            "text" => Some(ConfigOutputFormat::Text),
            _ => None,
        })
    }

    /// Merge CLI options with config (CLI wins)
    pub fn merge_max_results(&self, cli_value: Option<usize>) -> usize {
        cli_value
            .or(self.max_results)
            .unwrap_or(10)
    }

    /// Get a profile by name, falling back to built-in presets
    pub fn profile(&self, name: &str) -> ProfileConfig {
        if let Some(profile) = self.profiles.get(name) {
            profile.clone()
        } else {
            // Built-in presets
            match name {
                "human" => ProfileConfig::human(),
                "agent" => ProfileConfig::agent(),
                "fast" => ProfileConfig::fast(),
                _ => ProfileConfig::default(),
            }
        }
    }

    /// Get the search configuration
    pub fn search(&self) -> &SearchConfig {
        &self.search
    }

    /// Get the embedding configuration
    pub fn embeddings(&self) -> &EmbeddingConfig {
        &self.embeddings
    }

    /// Get the cache configuration
    pub fn cache(&self) -> &CacheConfig {
        &self.cache
    }

    /// Get the index configuration
    pub fn index(&self) -> &IndexConfig {
        &self.index
    }

    /// Check if embeddings should be enabled based on configuration and environment
    pub fn embeddings_enabled(&self) -> bool {
        match self.embeddings.enabled() {
            EmbeddingEnabled::Off => false,
            EmbeddingEnabled::On => true,
            EmbeddingEnabled::Auto => {
                // Auto-detect: check if embedder command exists or builtin is available
                // For now, default to false in auto mode until provider is verified
                false
            }
        }
    }
}
