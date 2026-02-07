// SPDX-License-Identifier: MIT OR Apache-2.0

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn write_file(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write file");
}

#[test]
fn max_chars_per_snippet_truncates_json_output() {
    let dir = TempDir::new().expect("tempdir");
    let file_path = dir.path().join("sample.txt");
    write_file(
        &file_path,
        "needle very very very long snippet line for budget testing",
    );

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let assert = cmd
        .current_dir(dir.path())
        .args([
            "--format",
            "json",
            "search",
            "needle",
            "--no-index",
            "--max-chars-per-snippet",
            "12",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let json: Value = serde_json::from_str(&stdout).expect("json");
    let first = json
        .as_array()
        .and_then(|arr| arr.first())
        .expect("first result");
    let snippet = first
        .get("snippet")
        .and_then(Value::as_str)
        .expect("snippet");

    assert!(snippet.chars().count() <= 12);
}

#[test]
fn max_total_chars_sets_json2_budget_meta() {
    let dir = TempDir::new().expect("tempdir");
    let file_path = dir.path().join("sample.txt");
    let content = (0..20)
        .map(|i| format!("needle line {} with some repeated words", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    write_file(&file_path, &content);

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let assert = cmd
        .current_dir(dir.path())
        .args([
            "--format",
            "json2",
            "search",
            "needle",
            "--no-index",
            "--max-total-chars",
            "80",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let json: Value = serde_json::from_str(&stdout).expect("json2");

    assert_eq!(json["meta"]["max_total_chars"], 80);
    assert!(json["meta"]["truncated"].is_boolean());
}
