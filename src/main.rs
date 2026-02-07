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
use cli::{AgentProvider, Cli, CliBudgetPreset, Commands};
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

fn cli_search_mode_to_hybrid(mode: cli::CliSearchMode) -> cgrep::hybrid::SearchMode {
    match mode {
        cli::CliSearchMode::Keyword => cgrep::hybrid::SearchMode::Keyword,
        cli::CliSearchMode::Semantic => cgrep::hybrid::SearchMode::Semantic,
        cli::CliSearchMode::Hybrid => cgrep::hybrid::SearchMode::Hybrid,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BudgetDefaults {
    max_chars_per_snippet: Option<usize>,
    max_total_chars: Option<usize>,
    max_context_chars: Option<usize>,
    dedupe_context: bool,
    path_alias: bool,
    suppress_boilerplate: bool,
}

fn budget_defaults(preset: Option<CliBudgetPreset>) -> BudgetDefaults {
    match preset {
        Some(CliBudgetPreset::Tight) => BudgetDefaults {
            max_chars_per_snippet: Some(120),
            max_total_chars: Some(2_400),
            max_context_chars: Some(320),
            dedupe_context: true,
            path_alias: true,
            suppress_boilerplate: true,
        },
        Some(CliBudgetPreset::Balanced) => BudgetDefaults {
            max_chars_per_snippet: Some(220),
            max_total_chars: Some(6_000),
            max_context_chars: Some(1_200),
            dedupe_context: true,
            path_alias: true,
            suppress_boilerplate: true,
        },
        Some(CliBudgetPreset::Full) => BudgetDefaults {
            max_chars_per_snippet: Some(500),
            max_total_chars: Some(15_000),
            max_context_chars: Some(4_000),
            dedupe_context: true,
            path_alias: false,
            suppress_boilerplate: true,
        },
        Some(CliBudgetPreset::Off) | None => BudgetDefaults::default(),
    }
}

fn print_search_advanced_help() {
    println!("Advanced search options:");
    println!("  --no-index                     Force scan mode (no index)");
    println!("  --fuzzy                        Fuzzy matching (index mode only)");
    println!("  --context-pack <n>             Merge overlapping context windows");
    println!("  --agent-cache                  Enable search result caching");
    println!("  --cache-ttl <ms>               Cache TTL (milliseconds)");
    println!("  --max-chars-per-snippet <n>    Manual snippet character cap");
    println!("  --max-context-chars <n>        Manual context character cap");
    println!("  --max-total-chars <n>          Manual total payload cap");
    println!("  --dedupe-context               Remove duplicate context lines");
    println!("  --path-alias                   Use p1/p2 path aliases in json2");
    println!("  --suppress-boilerplate         Suppress repeated import/header lines");
    println!();
    println!("Deprecated mode aliases (compatibility only):");
    println!("  --keyword | --semantic | --hybrid  (use --mode instead)");
}

fn install_for_provider(provider: AgentProvider) -> Result<()> {
    match provider {
        AgentProvider::ClaudeCode => install::claude_code::install(),
        AgentProvider::Codex => install::codex::install(),
        AgentProvider::Copilot => install::copilot::install(),
        AgentProvider::Opencode => install::opencode::install(),
    }
}

fn uninstall_for_provider(provider: AgentProvider) -> Result<()> {
    match provider {
        AgentProvider::ClaudeCode => install::claude_code::uninstall(),
        AgentProvider::Codex => install::codex::uninstall(),
        AgentProvider::Copilot => install::copilot::uninstall(),
        AgentProvider::Opencode => install::opencode::uninstall(),
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
            limit,
            context,
            file_type,
            glob,
            exclude,
            changed,
            budget,
            profile,
            quiet,
            regex,
            case_sensitive,
            mode,
            keyword,
            semantic,
            hybrid,
            help_advanced,
            context_pack,
            agent_cache,
            cache_ttl,
            max_chars_per_snippet,
            max_total_chars,
            max_context_chars,
            dedupe_context,
            path_alias,
            suppress_boilerplate,
            fuzzy,
            no_index,
        } => {
            if help_advanced {
                print_search_advanced_help();
                return Ok(());
            }

            let query = query.ok_or_else(|| {
                anyhow::anyhow!("search query is required (use `cgrep search --help`)")
            })?;
            let config = path
                .as_deref()
                .map(cgrep::config::Config::load_for_dir)
                .unwrap_or_else(cgrep::config::Config::load);
            let profile_config = profile.as_deref().map(|name| config.profile(name));
            let agent_profile_active = profile.as_deref() == Some("agent");
            let budget_preset = budget.or(if agent_profile_active {
                Some(CliBudgetPreset::Balanced)
            } else {
                None
            });
            let budget_defaults = budget_defaults(budget_preset);

            let effective_format = cli_format
                .or_else(|| {
                    profile_config
                        .as_ref()
                        .and_then(|p| p.format)
                        .map(config_output_to_cli)
                })
                .or_else(|| config.output_format().map(config_output_to_cli))
                .unwrap_or(cli::OutputFormat::Text);

            let effective_max_results = limit
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
                    .unwrap_or(false)
                || agent_profile_active;
            let effective_cache_ttl = cache_ttl.or(Some(config.cache.ttl_ms()));
            let effective_max_chars_per_snippet =
                max_chars_per_snippet.or(budget_defaults.max_chars_per_snippet);
            let effective_max_total_chars = max_total_chars.or(budget_defaults.max_total_chars);
            let effective_max_context_chars =
                max_context_chars.or(budget_defaults.max_context_chars);
            let effective_dedupe_context = dedupe_context || budget_defaults.dedupe_context;
            let effective_path_alias = path_alias || budget_defaults.path_alias;
            let effective_suppress_boilerplate =
                suppress_boilerplate || budget_defaults.suppress_boilerplate;

            if keyword {
                eprintln!("Warning: `--keyword` is deprecated; use `--mode keyword`");
            }
            if semantic {
                eprintln!("Warning: `--semantic` is deprecated; use `--mode semantic`");
            }
            if hybrid {
                eprintln!("Warning: `--hybrid` is deprecated; use `--mode hybrid`");
            }

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
                mode.map(cli_search_mode_to_hybrid)
                    .or(if agent_profile_active {
                        Some(cgrep::hybrid::SearchMode::Keyword)
                    } else {
                        profile_mode
                    })
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
                changed.as_deref(),
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
                effective_max_chars_per_snippet,
                effective_max_total_chars,
                effective_max_context_chars,
                effective_dedupe_context,
                effective_path_alias,
                effective_suppress_boilerplate,
            )?;
        }
        Commands::Agent { command } => match command {
            cli::AgentCommands::Locate {
                query,
                path,
                changed,
                limit,
                mode,
                budget,
            } => {
                let config = path
                    .as_deref()
                    .map(cgrep::config::Config::load_for_dir)
                    .unwrap_or_else(cgrep::config::Config::load);
                let effective_limit = limit.or(config.max_results).unwrap_or(20);
                let defaults = budget_defaults(Some(budget.unwrap_or(CliBudgetPreset::Balanced)));
                let effective_mode = mode
                    .map(cli_search_mode_to_hybrid)
                    .or(Some(cgrep::hybrid::SearchMode::Keyword));

                query::search::run(
                    &query,
                    path.as_deref(),
                    effective_limit,
                    0,
                    None,
                    None,
                    None,
                    changed.as_deref(),
                    true,
                    false,
                    false,
                    false,
                    false,
                    cli::OutputFormat::Json2,
                    compact,
                    effective_mode,
                    Some(2),
                    true,
                    Some(config.cache.ttl_ms()),
                    defaults.max_chars_per_snippet,
                    defaults.max_total_chars,
                    defaults.max_context_chars,
                    true,
                    true,
                    true,
                )?;
            }
            cli::AgentCommands::Expand { ids, path, context } => {
                query::agent::run_expand(&ids, path.as_deref(), context.unwrap_or(8), compact)?;
            }
            cli::AgentCommands::Install { provider } => {
                install_for_provider(provider)?;
            }
            cli::AgentCommands::Uninstall { provider } => {
                uninstall_for_provider(provider)?;
            }
        },
        Commands::Symbols {
            name,
            symbol_type,
            lang,
            file_type,
            glob,
            exclude,
            changed,
            quiet,
        } => {
            query::symbols::run(
                &name,
                symbol_type.as_deref(),
                lang.as_deref(),
                file_type.as_deref(),
                glob.as_deref(),
                exclude.as_deref(),
                changed.as_deref(),
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
            changed,
        } => {
            query::references::run(
                &name,
                path.as_deref(),
                max_results,
                changed.as_deref(),
                global_format,
                compact,
            )?;
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

        // Legacy installation commands (deprecated)
        Commands::InstallClaudeCode => {
            eprintln!(
                "Warning: `install-claude-code` is deprecated; use `cgrep agent install claude-code`"
            );
            install_for_provider(AgentProvider::ClaudeCode)?;
        }
        Commands::UninstallClaudeCode => {
            eprintln!(
                "Warning: `uninstall-claude-code` is deprecated; use `cgrep agent uninstall claude-code`"
            );
            uninstall_for_provider(AgentProvider::ClaudeCode)?;
        }
        Commands::InstallCodex => {
            eprintln!("Warning: `install-codex` is deprecated; use `cgrep agent install codex`");
            install_for_provider(AgentProvider::Codex)?;
        }
        Commands::UninstallCodex => {
            eprintln!(
                "Warning: `uninstall-codex` is deprecated; use `cgrep agent uninstall codex`"
            );
            uninstall_for_provider(AgentProvider::Codex)?;
        }
        Commands::InstallCopilot => {
            eprintln!(
                "Warning: `install-copilot` is deprecated; use `cgrep agent install copilot`"
            );
            install_for_provider(AgentProvider::Copilot)?;
        }
        Commands::UninstallCopilot => {
            eprintln!(
                "Warning: `uninstall-copilot` is deprecated; use `cgrep agent uninstall copilot`"
            );
            uninstall_for_provider(AgentProvider::Copilot)?;
        }
        Commands::InstallOpencode => {
            eprintln!(
                "Warning: `install-opencode` is deprecated; use `cgrep agent install opencode`"
            );
            install_for_provider(AgentProvider::Opencode)?;
        }
        Commands::UninstallOpencode => {
            eprintln!(
                "Warning: `uninstall-opencode` is deprecated; use `cgrep agent uninstall opencode`"
            );
            uninstall_for_provider(AgentProvider::Opencode)?;
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "cgrep", &mut std::io::stdout());
        }
    }

    Ok(())
}
