// SPDX-License-Identifier: MIT OR Apache-2.0

//! Agent-oriented query helpers.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::indexer::scanner::FileScanner;
use cgrep::output::print_json;

#[derive(Debug, Serialize)]
struct AgentExpandMeta {
    schema_version: &'static str,
    stage: &'static str,
    requested_ids: usize,
    resolved_ids: usize,
    context: usize,
    search_root: String,
}

#[derive(Debug, Serialize)]
struct AgentExpandResult {
    id: String,
    path: String,
    line: usize,
    start_line: usize,
    end_line: usize,
    snippet: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_after: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AgentExpandPayload {
    meta: AgentExpandMeta,
    results: Vec<AgentExpandResult>,
}

/// Expand stable result IDs into richer context windows for agent workflows.
pub fn run_expand(ids: &[String], path: Option<&str>, context: usize, compact: bool) -> Result<()> {
    let search_root = resolve_search_root(path)?;
    let scanner = FileScanner::new(&search_root);
    let files = scanner.scan()?;
    let wanted: HashSet<String> = ids.iter().cloned().collect();
    let mut results: Vec<AgentExpandResult> = Vec::new();

    for file in files {
        let rel_path = file
            .path
            .strip_prefix(&search_root)
            .unwrap_or(&file.path)
            .display()
            .to_string();

        let lines: Vec<&str> = file.content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            let snippet = line_to_snippet(line);
            let id = stable_result_id(&rel_path, line_num, &snippet);
            if !wanted.contains(&id) {
                continue;
            }

            let (context_before, context_after) = context_from_lines(&lines, line_num, context);
            let start_line = line_num.saturating_sub(context_before.len());
            let end_line = line_num + context_after.len();

            results.push(AgentExpandResult {
                id,
                path: rel_path.clone(),
                line: line_num,
                start_line,
                end_line,
                snippet,
                context_before,
                context_after,
            });
        }
    }

    results.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));

    let payload = AgentExpandPayload {
        meta: AgentExpandMeta {
            schema_version: "1",
            stage: "expand",
            requested_ids: wanted.len(),
            resolved_ids: results.len(),
            context,
            search_root: search_root.display().to_string(),
        },
        results,
    };
    print_json(&payload, compact)?;

    Ok(())
}

fn resolve_search_root(path: Option<&str>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Cannot determine current directory")?;
    let requested = path.map(PathBuf::from).unwrap_or_else(|| cwd.clone());
    let absolute = if requested.is_absolute() {
        requested
    } else {
        cwd.join(requested)
    };
    Ok(normalize_path(&absolute))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cleaned.pop();
            }
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => {
                cleaned.push(component.as_os_str());
            }
        }
    }
    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

fn line_to_snippet(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.len() <= 150 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..150])
    }
}

fn stable_result_id(path: &str, line: usize, snippet: &str) -> String {
    let payload = format!("{}:{}:{}", path, line, snippet);
    let hash = blake3::hash(payload.as_bytes());
    hash.to_hex()[..16].to_string()
}

fn context_from_lines(
    lines: &[&str],
    line_num: usize,
    context: usize,
) -> (Vec<String>, Vec<String>) {
    if context == 0 || lines.is_empty() {
        return (vec![], vec![]);
    }

    let idx = line_num.saturating_sub(1);
    let start = idx.saturating_sub(context);
    let end = (idx + context + 1).min(lines.len());

    let before = lines[start..idx].iter().map(|l| (*l).to_string()).collect();
    let after = if idx + 1 < end {
        lines[idx + 1..end]
            .iter()
            .map(|l| (*l).to_string())
            .collect()
    } else {
        vec![]
    };
    (before, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_result_id_is_deterministic() {
        let a = stable_result_id("src/lib.rs", 10, "fn alpha() {}");
        let b = stable_result_id("src/lib.rs", 10, "fn alpha() {}");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }
}
