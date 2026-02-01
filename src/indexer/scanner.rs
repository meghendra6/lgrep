// SPDX-License-Identifier: MIT OR Apache-2.0

//! File scanner using the ignore crate (same as ripgrep)

use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Scanned file with content
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub content: String,
    pub language: Option<String>,
}

/// File scanner that respects .gitignore
pub struct FileScanner {
    root: PathBuf,
    extensions: Vec<String>,
}

impl FileScanner {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            extensions: vec![
                "rs".into(), "ts".into(), "tsx".into(), "js".into(), "jsx".into(),
                "py".into(), "go".into(), "java".into(), "c".into(), "cpp".into(),
                "h".into(), "hpp".into(), "cs".into(), "rb".into(), "php".into(),
                "swift".into(), "kt".into(), "scala".into(), "lua".into(),
                "md".into(), "txt".into(), "json".into(), "yaml".into(), "toml".into(),
            ],
        }
    }

    /// Scan all files in the directory
    pub fn scan(&self) -> Result<Vec<ScannedFile>> {
        let (tx, rx) = mpsc::channel();

        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(|name| name != ".cgrep")
                    .unwrap_or(true)
            })
            .build_parallel();

        let extensions = self.extensions.clone();

        walker.run(|| {
            let tx = tx.clone();
            let extensions = extensions.clone();

            Box::new(move |entry| {
                if let Ok(entry) = entry {
                    let path = entry.path();

                    if path.is_file() {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if extensions.contains(&ext.to_lowercase()) {
                                if let Ok(content) = std::fs::read_to_string(path) {
                                    let language = detect_language(ext);
                                    let _ = tx.send(ScannedFile {
                                        path: path.to_path_buf(),
                                        content,
                                        language,
                                    });
                                }
                            }
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });

        drop(tx);
        Ok(rx.into_iter().collect())
    }

    /// Get list of file paths only (faster)
    #[allow(dead_code)]
    pub fn list_files(&self) -> Result<Vec<PathBuf>> {
        let (tx, rx) = mpsc::channel();

        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(|name| name != ".cgrep")
                    .unwrap_or(true)
            })
            .build_parallel();

        let extensions = self.extensions.clone();

        walker.run(|| {
            let tx = tx.clone();
            let extensions = extensions.clone();

            Box::new(move |entry| {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if extensions.contains(&ext.to_lowercase()) {
                                let _ = tx.send(path.to_path_buf());
                            }
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });

        drop(tx);
        Ok(rx.into_iter().collect())
    }
}

/// Detect language from file extension
pub fn detect_language(ext: &str) -> Option<String> {
    match ext.to_lowercase().as_str() {
        "rs" => Some("rust".into()),
        "ts" | "tsx" => Some("typescript".into()),
        "js" | "jsx" => Some("javascript".into()),
        "py" => Some("python".into()),
        "go" => Some("go".into()),
        "java" => Some("java".into()),
        "c" | "h" => Some("c".into()),
        "cpp" | "cc" | "hpp" => Some("cpp".into()),
        "cs" => Some("csharp".into()),
        "rb" => Some("ruby".into()),
        "php" => Some("php".into()),
        "swift" => Some("swift".into()),
        "kt" | "kts" => Some("kotlin".into()),
        "scala" => Some("scala".into()),
        "lua" => Some("lua".into()),
        _ => None,
    }
}
