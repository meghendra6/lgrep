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
    #[arg(long, default_value = "text", global = true)]
    pub format: OutputFormat,

    #[command(subcommand)]
    pub command: Commands,
}

/// Output format for results
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
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
        #[arg(short, long, default_value = "20")]
        max_results: usize,

        /// Show N lines before and after each match (like grep -C)
        #[arg(short = 'C', long, default_value = "0")]
        context: usize,

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
    },

    /// Watch for file changes and update index
    Watch {
        /// Path to watch (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,
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
