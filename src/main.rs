// SPDX-License-Identifier: MIT OR Apache-2.0

//! cgrep - Local semantic code search tool
//!
//! A high-performance, AST-aware search tool combining tree-sitter
//! for code structure analysis and tantivy for BM25 text ranking.

mod cli;
mod indexer;
mod install;
mod parser;
mod query;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{Cli, Commands};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // Initialize tracing with CGREP_LOG env var (e.g., CGREP_LOG=debug cgrep search "query")
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CGREP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let format = cli.format;

    match cli.command {
        Commands::Search {
            query,
            path,
            max_results,
            context,
            file_type,
            glob,
            exclude,
            quiet,
            fuzzy,
            no_index,
            regex,
            case_sensitive,
            mode: _,
            keyword: _,
            semantic: _,
            hybrid: _,
            profile: _,
            context_pack: _,
            agent_cache: _,
            cache_ttl: _,
        } => {
            query::search::run(
                &query,
                path.as_deref(),
                max_results,
                context,
                file_type.as_deref(),
                glob.as_deref(),
                exclude.as_deref(),
                quiet,
                fuzzy,
                no_index,
                regex,
                case_sensitive,
                format,
            )?;
        }
        Commands::Symbols {
            name,
            symbol_type,
            lang,
            file_type,
            glob,
            exclude,
            quiet,
        } => {
            query::symbols::run(
                &name,
                symbol_type.as_deref(),
                lang.as_deref(),
                file_type.as_deref(),
                glob.as_deref(),
                exclude.as_deref(),
                quiet,
                format,
            )?;
        }
        Commands::Definition { name } => {
            query::definition::run(&name, format)?;
        }
        Commands::Callers { function } => {
            query::callers::run(&function, format)?;
        }
        Commands::References {
            name,
            path,
            max_results,
        } => {
            query::references::run(&name, path.as_deref(), max_results, format)?;
        }
        Commands::Dependents { file } => {
            query::dependents::run(&file, format)?;
        }
        Commands::Index { path, force, embeddings: _, embeddings_force: _ } => {
            indexer::index::run(path.as_deref(), force)?;
        }
        Commands::Watch { path } => {
            indexer::watch::run(path.as_deref())?;
        }

        // Agent installation commands
        Commands::InstallClaudeCode => {
            install::claude_code::install()?;
        }
        Commands::UninstallClaudeCode => {
            install::claude_code::uninstall()?;
        }
        Commands::InstallCodex => {
            install::codex::install()?;
        }
        Commands::UninstallCodex => {
            install::codex::uninstall()?;
        }
        Commands::InstallCopilot => {
            install::copilot::install()?;
        }
        Commands::UninstallCopilot => {
            install::copilot::uninstall()?;
        }
        Commands::InstallOpencode => {
            install::opencode::install()?;
        }
        Commands::UninstallOpencode => {
            install::opencode::uninstall()?;
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "cgrep", &mut std::io::stdout());
        }
    }

    Ok(())
}
