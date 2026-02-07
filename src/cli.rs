// SPDX-License-Identifier: MIT OR Apache-2.0

//! CLI argument parsing using clap

use clap::{Parser, Subcommand};
use clap_complete::Shell;

/// cgrep - Local semantic code search tool
///
/// A high-performance search tool combining AST analysis with BM25 ranking.
/// Supports symbol search, dependency tracking, and full-text search.
#[derive(Parser, Debug)]
#[command(name = "cgrep")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Output format (text or json)
    #[arg(long, global = true)]
    pub format: Option<OutputFormat>,

    /// Compact JSON output (no pretty formatting)
    #[arg(long, global = true)]
    pub compact: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Output format for results
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    /// Structured JSON for AI agents (currently identical to json)
    Json2,
}

/// Search mode for queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum CliSearchMode {
    /// BM25 keyword search only
    #[default]
    Keyword,
    /// Embedding-based semantic search only
    Semantic,
    /// Combined BM25 + embedding search
    Hybrid,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Full-text search with BM25 ranking
    #[command(alias = "s")]
    Search {
        /// Search query (natural language or keywords)
        query: String,

        /// Path to search in (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Maximum number of results
        #[arg(short, long)]
        max_results: Option<usize>,

        /// Show N lines before and after each match (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,

        /// Filter by file type/language (e.g., rust, ts, python)
        #[arg(short = 't', long = "type")]
        file_type: Option<String>,

        /// Filter files matching glob pattern (e.g., "*.rs", "src/**/*.ts")
        #[arg(short = 'g', long)]
        glob: Option<String>,

        /// Exclude files matching pattern
        #[arg(long)]
        exclude: Option<String>,

        /// Suppress statistics output
        #[arg(short = 'q', long)]
        quiet: bool,

        /// Enable fuzzy matching (allows 1-2 character differences)
        #[arg(short = 'f', long)]
        fuzzy: bool,

        /// Do not use the index; scan files directly
        #[arg(long)]
        no_index: bool,

        /// Treat query as a regular expression (scan mode)
        #[arg(long)]
        regex: bool,

        /// Case-sensitive search (scan mode)
        #[arg(long)]
        case_sensitive: bool,

        /// Search mode: keyword, semantic, or hybrid
        #[arg(long, value_enum)]
        mode: Option<CliSearchMode>,

        /// Use keyword search only (alias for --mode keyword)
        #[arg(long, conflicts_with = "semantic", conflicts_with = "hybrid")]
        keyword: bool,

        /// Use semantic search only (alias for --mode semantic)
        #[arg(long, conflicts_with = "keyword", conflicts_with = "hybrid")]
        semantic: bool,

        /// Use hybrid search (alias for --mode hybrid)
        #[arg(long, conflicts_with = "keyword", conflicts_with = "semantic")]
        hybrid: bool,

        /// Use a preset profile (human, agent, fast)
        #[arg(long)]
        profile: Option<String>,

        /// Context pack size for agent mode (merges overlapping context)
        #[arg(long)]
        context_pack: Option<usize>,

        /// Enable agent session caching
        #[arg(long)]
        agent_cache: bool,

        /// Cache TTL in milliseconds (default: 600000 = 10 minutes)
        #[arg(long)]
        cache_ttl: Option<u64>,
    },

    /// Search for symbols (functions, classes, etc.)
    Symbols {
        /// Symbol name to search for
        name: String,

        /// Filter by symbol type (function, class, variable, etc.)
        #[arg(short = 'T', long = "type")]
        symbol_type: Option<String>,

        /// Filter by language (typescript, python, rust, etc.)
        #[arg(short, long)]
        lang: Option<String>,

        /// Filter by file type/language (e.g., rust, ts, python)
        #[arg(short = 't', long = "file-type")]
        file_type: Option<String>,

        /// Filter files matching glob pattern (e.g., "*.rs", "src/**/*.ts")
        #[arg(short = 'g', long)]
        glob: Option<String>,

        /// Exclude files matching pattern
        #[arg(long)]
        exclude: Option<String>,

        /// Suppress statistics output
        #[arg(short = 'q', long)]
        quiet: bool,
    },

    /// Find symbol definition location
    #[command(alias = "def")]
    Definition {
        /// Symbol name to find definition for
        name: String,
    },

    /// Find all callers of a function
    Callers {
        /// Function name to find callers for
        function: String,
    },

    /// Find all references to a symbol
    #[command(alias = "refs")]
    References {
        /// Symbol name to find references for
        name: String,

        /// Path to search in (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Maximum number of results
        #[arg(short, long, default_value = "50")]
        max_results: usize,
    },

    /// Find files that depend on a given file
    #[command(alias = "deps")]
    Dependents {
        /// File path to find dependents for
        file: String,
    },

    /// Build or rebuild the search index
    Index {
        /// Path to index (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Force full reindex
        #[arg(short, long)]
        force: bool,

        /// Embedding generation mode: auto, precompute, or off
        #[arg(long, default_value = "off")]
        embeddings: String,

        /// Force regeneration of all embeddings
        #[arg(long)]
        embeddings_force: bool,

        /// Use a high-memory index writer (1GiB budget)
        #[arg(long)]
        high_memory: bool,

        /// Paths/patterns to exclude (can be specified multiple times)
        #[arg(long = "exclude", short = 'e')]
        exclude_paths: Vec<String>,
    },

    /// Watch for file changes and update index
    Watch {
        /// Path to watch (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Debounce interval in seconds (default: 2)
        #[arg(long, default_value = "2")]
        debounce: u64,
    },

    /// Install cgrep for Claude Code
    #[command(name = "install-claude-code")]
    InstallClaudeCode,

    /// Uninstall cgrep from Claude Code
    #[command(name = "uninstall-claude-code")]
    UninstallClaudeCode,

    /// Install cgrep for Codex
    #[command(name = "install-codex")]
    InstallCodex,

    /// Uninstall cgrep from Codex
    #[command(name = "uninstall-codex")]
    UninstallCodex,

    /// Install cgrep for GitHub Copilot
    #[command(name = "install-copilot")]
    InstallCopilot,

    /// Uninstall cgrep from GitHub Copilot
    #[command(name = "uninstall-copilot")]
    UninstallCopilot,

    /// Install cgrep for OpenCode
    #[command(name = "install-opencode")]
    InstallOpencode,

    /// Uninstall cgrep from OpenCode
    #[command(name = "uninstall-opencode")]
    UninstallOpencode,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}
