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
    /// Structured JSON for AI agents (`meta` + `results`)
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

/// Output budget preset for token-efficient responses
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CliBudgetPreset {
    Tight,
    Balanced,
    Full,
    Off,
}

/// Agent provider for install/uninstall commands
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentProvider {
    ClaudeCode,
    Codex,
    Copilot,
    Opencode,
}

#[derive(Subcommand, Debug)]
pub enum AgentCommands {
    /// Stage 1: locate candidate code regions with minimal payload
    Locate {
        /// Search query (natural language or keywords)
        query: String,

        /// Path to search in (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Limit search to files changed since revision (default: HEAD)
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD")]
        changed: Option<String>,

        /// Maximum number of results to return
        #[arg(short = 'm', long = "limit")]
        limit: Option<usize>,

        /// Search mode: keyword, semantic, or hybrid
        #[arg(long, value_enum)]
        mode: Option<CliSearchMode>,

        /// Output budget preset (default: balanced)
        #[arg(long, value_enum)]
        budget: Option<CliBudgetPreset>,
    },

    /// Stage 2: expand selected locate result IDs into richer context
    Expand {
        /// Result ID from `agent locate` (repeatable)
        #[arg(long = "id", required = true)]
        ids: Vec<String>,

        /// Path to search in (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Context lines to return around each ID match
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },

    /// Install cgrep instructions for an AI agent provider
    Install {
        #[arg(value_enum)]
        provider: AgentProvider,
    },

    /// Uninstall cgrep instructions for an AI agent provider
    Uninstall {
        #[arg(value_enum)]
        provider: AgentProvider,
    },
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Full-text search with BM25 ranking
    #[command(alias = "s")]
    Search {
        /// Search query (natural language or keywords)
        #[arg(required_unless_present = "help_advanced")]
        query: Option<String>,

        /// Path to search in (defaults to current directory)
        #[arg(short, long, help_heading = "Core")]
        path: Option<String>,

        /// Maximum number of results
        #[arg(
            short = 'm',
            long = "limit",
            visible_alias = "max-results",
            help_heading = "Core"
        )]
        limit: Option<usize>,

        /// Show N lines before and after each match (like grep -C)
        #[arg(short = 'C', long, help_heading = "Core")]
        context: Option<usize>,

        /// Filter by file type/language (e.g., rust, ts, python)
        #[arg(short = 't', long = "type", help_heading = "Core")]
        file_type: Option<String>,

        /// Filter files matching glob pattern (e.g., "*.rs", "src/**/*.ts")
        #[arg(short = 'g', long, help_heading = "Core")]
        glob: Option<String>,

        /// Exclude files matching pattern
        #[arg(long, help_heading = "Core")]
        exclude: Option<String>,

        /// Limit search to files changed since revision (default: HEAD)
        #[arg(
            long,
            num_args = 0..=1,
            default_missing_value = "HEAD",
            help_heading = "Core"
        )]
        changed: Option<String>,

        /// Output budget preset (tight, balanced, full, off)
        #[arg(long, value_enum, help_heading = "Core")]
        budget: Option<CliBudgetPreset>,

        /// Use a preset profile (human, agent, fast)
        #[arg(long, help_heading = "Core")]
        profile: Option<String>,

        /// Suppress statistics output
        #[arg(short = 'q', long, help_heading = "Core")]
        quiet: bool,

        /// Treat query as a regular expression (scan mode)
        #[arg(long, help_heading = "Mode")]
        regex: bool,

        /// Case-sensitive search (scan mode)
        #[arg(long, help_heading = "Mode")]
        case_sensitive: bool,

        /// Search mode: keyword, semantic, or hybrid
        #[arg(long, value_enum, help_heading = "Mode")]
        mode: Option<CliSearchMode>,

        /// Deprecated: use `--mode keyword`
        #[arg(
            long,
            hide = true,
            conflicts_with = "semantic",
            conflicts_with = "hybrid"
        )]
        keyword: bool,

        /// Deprecated: use `--mode semantic`
        #[arg(
            long,
            hide = true,
            conflicts_with = "keyword",
            conflicts_with = "hybrid"
        )]
        semantic: bool,

        /// Deprecated: use `--mode hybrid`
        #[arg(
            long,
            hide = true,
            conflicts_with = "keyword",
            conflicts_with = "semantic"
        )]
        hybrid: bool,

        /// Print advanced options for search and exit
        #[arg(long, help_heading = "Help")]
        help_advanced: bool,

        /// Context pack size for agent mode (merges overlapping context)
        #[arg(long, hide = true)]
        context_pack: Option<usize>,

        /// Enable agent session caching
        #[arg(long, hide = true)]
        agent_cache: bool,

        /// Cache TTL in milliseconds (default: 600000 = 10 minutes)
        #[arg(long, hide = true)]
        cache_ttl: Option<u64>,

        /// Maximum characters per snippet in output
        #[arg(long, hide = true)]
        max_chars_per_snippet: Option<usize>,

        /// Maximum total characters across returned results
        #[arg(long, hide = true)]
        max_total_chars: Option<usize>,

        /// Maximum context characters per result (before+after)
        #[arg(long, hide = true)]
        max_context_chars: Option<usize>,

        /// Remove duplicated context lines across results
        #[arg(long, hide = true)]
        dedupe_context: bool,

        /// Use short path aliases (p1, p2, ...) in json2 output with lookup table in meta
        #[arg(long, hide = true)]
        path_alias: bool,

        /// Suppress repeated boilerplate lines (imports/headers) in snippets and context
        #[arg(long, hide = true)]
        suppress_boilerplate: bool,

        /// Enable fuzzy matching (allows 1-2 character differences)
        #[arg(short = 'f', long, hide = true)]
        fuzzy: bool,

        /// Do not use the index; scan files directly
        #[arg(long, hide = true)]
        no_index: bool,
    },

    /// Agent-optimized workflow: locate/expand/install/uninstall
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
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

        /// Limit symbol search to files changed since revision (default: HEAD)
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD")]
        changed: Option<String>,

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
        #[arg(
            short = 'm',
            long = "limit",
            visible_alias = "max-results",
            default_value = "50"
        )]
        max_results: usize,

        /// Limit references to files changed since revision (default: HEAD)
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD")]
        changed: Option<String>,
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
    #[command(name = "install-claude-code", hide = true)]
    InstallClaudeCode,

    /// Uninstall cgrep from Claude Code
    #[command(name = "uninstall-claude-code", hide = true)]
    UninstallClaudeCode,

    /// Install cgrep for Codex
    #[command(name = "install-codex", hide = true)]
    InstallCodex,

    /// Uninstall cgrep from Codex
    #[command(name = "uninstall-codex", hide = true)]
    UninstallCodex,

    /// Install cgrep for GitHub Copilot
    #[command(name = "install-copilot", hide = true)]
    InstallCopilot,

    /// Uninstall cgrep from GitHub Copilot
    #[command(name = "uninstall-copilot", hide = true)]
    UninstallCopilot,

    /// Install cgrep for OpenCode
    #[command(name = "install-opencode", hide = true)]
    InstallOpencode,

    /// Uninstall cgrep from OpenCode
    #[command(name = "uninstall-opencode", hide = true)]
    UninstallOpencode,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}
