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
            mode,
            keyword,
            semantic,
            hybrid,
            profile,
            context_pack,
            agent_cache,
            cache_ttl,
        } => {
            // Determine effective search mode from flags
            let effective_mode = if hybrid {
                Some(cgrep::hybrid::SearchMode::Hybrid)
            } else if semantic {
                Some(cgrep::hybrid::SearchMode::Semantic)
            } else if keyword {
                Some(cgrep::hybrid::SearchMode::Keyword)
            } else {
                mode.map(|m| match m {
                    cli::CliSearchMode::Keyword => cgrep::hybrid::SearchMode::Keyword,
                    cli::CliSearchMode::Semantic => cgrep::hybrid::SearchMode::Semantic,
                    cli::CliSearchMode::Hybrid => cgrep::hybrid::SearchMode::Hybrid,
                })
            };
            
            // Apply profile settings if specified
            let _ = profile; // TODO: Apply profile settings
            
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
                effective_mode,
                context_pack,
                agent_cache,
                cache_ttl,
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
        Commands::Index { path, force, embeddings: _, embeddings_force: _, exclude_paths } => {
            // Load config for additional exclude paths
            let config = cgrep::config::Config::load();
            
            // Merge CLI excludes with config excludes (CLI takes precedence by being added first)
            let mut all_excludes = exclude_paths;
            all_excludes.extend(config.index().exclude_paths().iter().cloned());
            
            indexer::index::run(path.as_deref(), force, all_excludes)?;
        }
        Commands::Watch { path, debounce } => {
            indexer::watch::run(path.as_deref(), Some(debounce))?;
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
