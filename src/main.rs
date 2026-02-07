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

fn config_output_to_cli(format: cgrep::config::ConfigOutputFormat) -> cli::OutputFormat {
    match format {
        cgrep::config::ConfigOutputFormat::Text => cli::OutputFormat::Text,
        cgrep::config::ConfigOutputFormat::Json => cli::OutputFormat::Json,
        cgrep::config::ConfigOutputFormat::Json2 => cli::OutputFormat::Json2,
    }
}

fn config_search_mode_to_hybrid(mode: cgrep::config::SearchMode) -> cgrep::hybrid::SearchMode {
    match mode {
        cgrep::config::SearchMode::Keyword => cgrep::hybrid::SearchMode::Keyword,
        cgrep::config::SearchMode::Semantic => cgrep::hybrid::SearchMode::Semantic,
        cgrep::config::SearchMode::Hybrid => cgrep::hybrid::SearchMode::Hybrid,
    }
}

fn main() -> Result<()> {
    // Initialize tracing with CGREP_LOG env var (e.g., CGREP_LOG=debug cgrep search "query")
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CGREP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let global_config = cgrep::config::Config::load();
    let default_format = global_config
        .output_format()
        .map(config_output_to_cli)
        .unwrap_or(cli::OutputFormat::Text);
    let cli_format = cli.format;
    let compact = cli.compact;
    let global_format = cli_format.unwrap_or(default_format);

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
            let config = path
                .as_deref()
                .map(cgrep::config::Config::load_for_dir)
                .unwrap_or_else(cgrep::config::Config::load);
            let profile_config = profile.as_deref().map(|name| config.profile(name));

            let effective_format = cli_format
                .or_else(|| {
                    profile_config
                        .as_ref()
                        .and_then(|p| p.format)
                        .map(config_output_to_cli)
                })
                .or_else(|| config.output_format().map(config_output_to_cli))
                .unwrap_or(cli::OutputFormat::Text);

            let effective_max_results = max_results
                .or_else(|| profile_config.as_ref().and_then(|p| p.max_results))
                .or(config.max_results)
                .unwrap_or(20);
            let effective_context = context
                .or_else(|| profile_config.as_ref().and_then(|p| p.context))
                .unwrap_or(0);
            let effective_context_pack = context_pack.or_else(|| {
                profile_config
                    .as_ref()
                    .and_then(|p| p.context_pack.or(p.context))
            });
            let effective_agent_cache = agent_cache
                || profile_config
                    .as_ref()
                    .and_then(|p| p.agent_cache)
                    .unwrap_or(false);
            let effective_cache_ttl = cache_ttl.or(Some(config.cache.ttl_ms()));

            // Determine effective search mode from flags
            let profile_mode = profile_config
                .as_ref()
                .and_then(|p| p.mode)
                .map(config_search_mode_to_hybrid);
            let config_mode = config.search.default_mode.map(config_search_mode_to_hybrid);
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
                .or(profile_mode)
                .or(config_mode)
            };

            query::search::run(
                &query,
                path.as_deref(),
                effective_max_results,
                effective_context,
                file_type.as_deref(),
                glob.as_deref(),
                exclude.as_deref(),
                quiet,
                fuzzy,
                no_index,
                regex,
                case_sensitive,
                effective_format,
                compact,
                effective_mode,
                effective_context_pack,
                effective_agent_cache,
                effective_cache_ttl,
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
                global_format,
                compact,
            )?;
        }
        Commands::Definition { name } => {
            query::definition::run(&name, global_format, compact)?;
        }
        Commands::Callers { function } => {
            query::callers::run(&function, global_format, compact)?;
        }
        Commands::References {
            name,
            path,
            max_results,
        } => {
            query::references::run(&name, path.as_deref(), max_results, global_format, compact)?;
        }
        Commands::Dependents { file } => {
            query::dependents::run(&file, global_format, compact)?;
        }
        Commands::Index {
            path,
            force,
            embeddings,
            embeddings_force,
            high_memory,
            exclude_paths,
        } => {
            indexer::index::run(
                path.as_deref(),
                force,
                exclude_paths,
                high_memory,
                &embeddings,
                embeddings_force,
            )?;
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
