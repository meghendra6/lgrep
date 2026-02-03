// SPDX-License-Identifier: MIT OR Apache-2.0

//! Utility functions for cgrep

use std::path::{Path, PathBuf};

/// The name of the index directory
pub const INDEX_DIR: &str = ".cgrep";

/// Result of finding an index root
#[derive(Debug)]
pub struct IndexRoot {
    /// The directory containing the .cgrep folder
    pub root: PathBuf,
    /// The full path to the .cgrep folder
    pub index_path: PathBuf,
    /// Whether this is the current directory or a parent
    pub is_parent: bool,
}

/// Find the nearest .cgrep index directory by walking up from the given path.
/// Returns None if no .cgrep directory is found.
pub fn find_index_root(start: impl AsRef<Path>) -> Option<IndexRoot> {
    let mut current = start.as_ref().to_path_buf();
    
    // Canonicalize to handle relative paths
    if let Ok(canonical) = current.canonicalize() {
        current = canonical;
    }
    
    let original = current.clone();
    
    loop {
        let index_path = current.join(INDEX_DIR);
        if index_path.exists() && index_path.is_dir() {
            return Some(IndexRoot {
                root: current.clone(),
                index_path,
                is_parent: current != original,
            });
        }
        
        if !current.pop() {
            break;
        }
    }
    
    None
}

/// Get the index path for the current directory, walking up to find parent indexes.
/// Falls back to the given path if no index is found anywhere.
pub fn get_index_path(path: impl AsRef<Path>) -> PathBuf {
    match find_index_root(&path) {
        Some(root) => root.index_path,
        None => path.as_ref().join(INDEX_DIR),
    }
}

/// Get the root directory that contains the index.
/// Falls back to the given path if no index is found.
pub fn get_root_with_index(path: impl AsRef<Path>) -> PathBuf {
    match find_index_root(&path) {
        Some(root) => root.root,
        None => path.as_ref().to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn find_index_root_in_current_dir() {
        let dir = TempDir::new().unwrap();
        let index_dir = dir.path().join(INDEX_DIR);
        fs::create_dir(&index_dir).unwrap();
        
        let result = find_index_root(dir.path()).unwrap();
        assert_eq!(result.root, dir.path().canonicalize().unwrap());
        assert!(!result.is_parent);
    }

    #[test]
    fn find_index_root_in_parent() {
        let dir = TempDir::new().unwrap();
        let index_dir = dir.path().join(INDEX_DIR);
        fs::create_dir(&index_dir).unwrap();
        
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        
        let result = find_index_root(&subdir).unwrap();
        assert_eq!(result.root, dir.path().canonicalize().unwrap());
        assert!(result.is_parent);
    }

    #[test]
    fn find_index_root_none() {
        let dir = TempDir::new().unwrap();
        assert!(find_index_root(dir.path()).is_none());
    }

    #[test]
    fn get_index_path_fallback() {
        let dir = TempDir::new().unwrap();
        let path = get_index_path(dir.path());
        assert_eq!(path, dir.path().join(INDEX_DIR));
    }
}
