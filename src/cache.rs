// SPDX-License-Identifier: MIT OR Apache-2.0

//! Agent session cache for search results.
//!
//! Caches search results to avoid recomputation for identical queries.
//! Cache is stored in `.cgrep/cache/search/<hash>.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Cache key components for generating cache hash
#[derive(Debug, Clone, Serialize)]
pub struct CacheKey {
    /// Search query
    pub query: String,
    /// Search mode (keyword, semantic, hybrid)
    pub mode: String,
    /// Maximum results requested
    pub max_results: usize,
    /// Context lines
    pub context: usize,
    /// File type filter
    pub file_type: Option<String>,
    /// Glob pattern filter
    pub glob: Option<String>,
    /// Exclude pattern
    pub exclude: Option<String>,
    /// Profile name if used
    pub profile: Option<String>,
    /// Index hash for cache invalidation
    pub index_hash: Option<String>,
    /// Embedding model for cache invalidation
    pub embedding_model: Option<String>,
    /// Search root for scoping results
    pub search_root: Option<String>,
}

impl CacheKey {
    /// Generate a cache hash from the key
    pub fn hash(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        let hash = blake3::hash(json.as_bytes());
        hash.to_hex()[..32].to_string()
    }
}

/// Cached search result entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<T> {
    /// The cached data
    pub data: T,
    /// Cache creation timestamp (Unix epoch milliseconds)
    pub created_at: u64,
    /// Cache key hash for verification
    pub key_hash: String,
    /// Search mode used
    pub mode: String,
}

impl<T> CacheEntry<T> {
    /// Create a new cache entry
    pub fn new(data: T, key: &CacheKey) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            data,
            created_at: now,
            key_hash: key.hash(),
            mode: key.mode.clone(),
        }
    }

    /// Check if the cache entry is still valid
    pub fn is_valid(&self, ttl_ms: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        now.saturating_sub(self.created_at) < ttl_ms
    }

    /// Get the age of this cache entry in milliseconds
    pub fn age_ms(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        now.saturating_sub(self.created_at)
    }
}

/// Agent session cache manager
pub struct SearchCache {
    /// Cache directory path
    cache_dir: PathBuf,
    /// Cache TTL in milliseconds
    ttl_ms: u64,
}

impl SearchCache {
    /// Default cache TTL (10 minutes)
    pub const DEFAULT_TTL_MS: u64 = 600_000;

    /// Create a new cache manager
    pub fn new<P: AsRef<Path>>(repo_root: P, ttl_ms: u64) -> Result<Self> {
        let cache_dir = repo_root
            .as_ref()
            .join(".cgrep")
            .join("cache")
            .join("search");
        fs::create_dir_all(&cache_dir).with_context(|| {
            format!("Failed to create cache directory: {}", cache_dir.display())
        })?;

        Ok(Self { cache_dir, ttl_ms })
    }

    /// Create with default TTL
    pub fn with_default_ttl<P: AsRef<Path>>(repo_root: P) -> Result<Self> {
        Self::new(repo_root, Self::DEFAULT_TTL_MS)
    }

    /// Get the cache file path for a key
    fn cache_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir.join(format!("{}.json", key.hash()))
    }

    /// Try to get a cached result
    pub fn get<T>(&self, key: &CacheKey) -> Result<Option<CacheEntry<T>>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let path = self.cache_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read cache: {}", path.display()))?;

        let entry: CacheEntry<T> = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse cache: {}", path.display()))?;

        // Check if cache is still valid
        if !entry.is_valid(self.ttl_ms) {
            // Cache expired, remove it
            let _ = fs::remove_file(&path);
            return Ok(None);
        }

        // Verify key hash matches
        if entry.key_hash != key.hash() {
            return Ok(None);
        }

        Ok(Some(entry))
    }

    /// Store a result in the cache
    pub fn put<T>(&self, key: &CacheKey, data: T) -> Result<()>
    where
        T: Serialize,
    {
        let entry = CacheEntry::new(data, key);
        let path = self.cache_path(key);

        let json =
            serde_json::to_string_pretty(&entry).context("Failed to serialize cache entry")?;

        fs::write(&path, json)
            .with_context(|| format!("Failed to write cache: {}", path.display()))?;

        Ok(())
    }

    /// Clear all cached entries
    pub fn clear(&self) -> Result<usize> {
        let mut count = 0;
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .map(|e| e == "json")
                .unwrap_or(false)
            {
                fs::remove_file(entry.path())?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Clear expired cache entries
    pub fn prune(&self) -> Result<usize> {
        let mut count = 0;
        let now = SystemTime::now();

        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // Check file modification time
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
                        if age.as_millis() as u64 > self.ttl_ms {
                            fs::remove_file(&path)?;
                            count += 1;
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// Get cache statistics
    pub fn stats(&self) -> Result<CacheStats> {
        let mut total_entries = 0;
        let mut total_bytes = 0;
        let mut expired = 0;
        let now = SystemTime::now();

        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                total_entries += 1;
                if let Ok(metadata) = entry.metadata() {
                    total_bytes += metadata.len();

                    if let Ok(modified) = metadata.modified() {
                        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
                        if age.as_millis() as u64 > self.ttl_ms {
                            expired += 1;
                        }
                    }
                }
            }
        }

        Ok(CacheStats {
            total_entries,
            expired_entries: expired,
            total_bytes,
        })
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Total number of cache entries
    pub total_entries: usize,
    /// Number of expired entries
    pub expired_entries: usize,
    /// Total size in bytes
    pub total_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_key(query: &str) -> CacheKey {
        CacheKey {
            query: query.to_string(),
            mode: "keyword".to_string(),
            max_results: 20,
            context: 2,
            file_type: None,
            glob: None,
            exclude: None,
            profile: None,
            index_hash: None,
            embedding_model: None,
            search_root: None,
        }
    }

    #[test]
    fn test_cache_key_hash() {
        let key1 = make_key("hello");
        let key2 = make_key("hello");
        let key3 = make_key("world");

        assert_eq!(key1.hash(), key2.hash());
        assert_ne!(key1.hash(), key3.hash());
    }

    #[test]
    fn test_cache_put_get() {
        let dir = tempdir().unwrap();
        let cache = SearchCache::new(dir.path(), 600_000).unwrap();

        let key = make_key("test query");
        let data = vec!["result1".to_string(), "result2".to_string()];

        cache.put(&key, &data).unwrap();

        let entry: Option<CacheEntry<Vec<String>>> = cache.get(&key).unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().data, data);
    }

    #[test]
    fn test_cache_miss() {
        let dir = tempdir().unwrap();
        let cache = SearchCache::new(dir.path(), 600_000).unwrap();

        let key = make_key("nonexistent");
        let entry: Option<CacheEntry<Vec<String>>> = cache.get(&key).unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_cache_ttl() {
        let dir = tempdir().unwrap();
        // Very short TTL for testing
        let cache = SearchCache::new(dir.path(), 1).unwrap();

        let key = make_key("test");
        cache.put(&key, "data").unwrap();

        // Wait for expiry
        std::thread::sleep(std::time::Duration::from_millis(10));

        let entry: Option<CacheEntry<String>> = cache.get(&key).unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_cache_clear() {
        let dir = tempdir().unwrap();
        let cache = SearchCache::new(dir.path(), 600_000).unwrap();

        cache.put(&make_key("a"), "data1").unwrap();
        cache.put(&make_key("b"), "data2").unwrap();

        let cleared = cache.clear().unwrap();
        assert_eq!(cleared, 2);

        let entry: Option<CacheEntry<String>> = cache.get(&make_key("a")).unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_cache_entry_age() {
        let key = make_key("test");
        let entry = CacheEntry::new("data", &key);

        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(entry.age_ms() >= 10);
    }
}
