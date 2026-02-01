//! File filtering utilities for search and symbol commands
//!
//! Provides shared filtering logic for file type matching, glob patterns,
//! and exclusion patterns with precompiled regex support.

use regex::Regex;

/// A precompiled glob pattern for efficient repeated matching
#[derive(Debug, Clone)]
pub struct CompiledGlob {
    regex: Regex,
}

impl CompiledGlob {
    /// Compile a glob pattern to a regex
    pub fn new(pattern: &str) -> Option<Self> {
        // Handle glob patterns properly:
        // - `**` matches zero or more path segments
        // - `*` matches any characters except `/`
        let regex_pattern = pattern
            .replace(".", "\\.")
            .replace("**/", "{{DOUBLESTARSLASH}}")  // Match zero or more dirs
            .replace("/**", "{{SLASHDOUBLESTAR}}")  // Match zero or more suffix
            .replace("**", ".*")                     // Standalone ** matches anything
            .replace("*", "[^/]*")
            .replace("{{DOUBLESTARSLASH}}", "(.*/)?")
            .replace("{{SLASHDOUBLESTAR}}", "(/.*)?");

        Regex::new(&format!("(?i){}", regex_pattern))
            .ok()
            .map(|regex| Self { regex })
    }

    /// Check if a path matches this glob pattern
    pub fn is_match(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }
}

/// Check if file matches the given type filter
pub fn matches_file_type(path: &str, file_type: Option<&str>) -> bool {
    let Some(filter) = file_type else { return true };
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match filter.to_lowercase().as_str() {
        "rust" | "rs" => ext == "rs",
        "typescript" | "ts" => ext == "ts" || ext == "tsx",
        "javascript" | "js" => ext == "js" || ext == "jsx",
        "python" | "py" => ext == "py",
        "go" => ext == "go",
        "c" => ext == "c" || ext == "h",
        "cpp" | "c++" => ext == "cpp" || ext == "hpp" || ext == "cc" || ext == "cxx",
        "java" => ext == "java",
        "ruby" | "rb" => ext == "rb",
        "php" => ext == "php",
        "swift" => ext == "swift",
        "kotlin" | "kt" => ext == "kt" || ext == "kts",
        "scala" => ext == "scala",
        "lua" => ext == "lua",
        "shell" | "sh" | "bash" => ext == "sh" || ext == "bash",
        "yaml" | "yml" => ext == "yaml" || ext == "yml",
        "json" => ext == "json",
        "toml" => ext == "toml",
        "md" | "markdown" => ext == "md" || ext == "markdown",
        _ => ext == filter,
    }
}

/// Check if file matches glob pattern using precompiled regex
pub fn matches_glob_compiled(path: &str, glob: Option<&CompiledGlob>) -> bool {
    match glob {
        Some(g) => g.is_match(path),
        None => true,
    }
}

/// Check if file matches glob pattern (simple implementation)
/// Prefer using CompiledGlob for repeated matching in hot paths
pub fn matches_glob(path: &str, glob_pattern: Option<&str>) -> bool {
    let Some(pattern) = glob_pattern else { return true };

    let regex_pattern = pattern
        .replace(".", "\\.")
        .replace("**/", "{{DOUBLESTARSLASH}}")
        .replace("/**", "{{SLASHDOUBLESTAR}}")
        .replace("**", ".*")
        .replace("*", "[^/]*")
        .replace("{{DOUBLESTARSLASH}}", "(.*/)?")
        .replace("{{SLASHDOUBLESTAR}}", "(/.*)?");

    Regex::new(&format!("(?i){}", regex_pattern))
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}

/// Check if file should be excluded using precompiled glob
pub fn should_exclude_compiled(path: &str, exclude: Option<&CompiledGlob>) -> bool {
    matches_glob_compiled(path, exclude) && exclude.is_some()
}

/// Check if file should be excluded
/// Prefer using CompiledGlob for repeated matching in hot paths
pub fn should_exclude(path: &str, exclude_pattern: Option<&str>) -> bool {
    let Some(pattern) = exclude_pattern else { return false };
    matches_glob(path, Some(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_file_type() {
        assert!(matches_file_type("src/main.rs", Some("rust")));
        assert!(matches_file_type("src/main.rs", Some("rs")));
        assert!(!matches_file_type("src/main.rs", Some("python")));
        assert!(matches_file_type("src/main.rs", None));
    }

    #[test]
    fn test_compiled_glob() {
        let glob = CompiledGlob::new("src/**/*.rs").unwrap();
        assert!(glob.is_match("src/main.rs"));
        assert!(glob.is_match("src/query/search.rs"));
        assert!(!glob.is_match("tests/main.rs"));
    }

    #[test]
    fn test_should_exclude() {
        assert!(should_exclude("target/debug/main", Some("target/**")));
        assert!(!should_exclude("src/main.rs", Some("target/**")));
        assert!(!should_exclude("src/main.rs", None));
    }
}
