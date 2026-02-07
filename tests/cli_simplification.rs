// SPDX-License-Identifier: MIT OR Apache-2.0

use assert_cmd::Command;
use predicates::prelude::*;
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
fn search_help_advanced_prints_hidden_options() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    cmd.args(["search", "--help-advanced"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Advanced search options:"))
        .stdout(predicate::str::contains("--max-total-chars"));
}

#[test]
fn deprecated_mode_alias_prints_warning() {
    let dir = TempDir::new().expect("tempdir");
    write_file(&dir.path().join("sample.txt"), "needle\n");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    cmd.current_dir(dir.path())
        .args([
            "--format",
            "json",
            "search",
            "needle",
            "--keyword",
            "--no-index",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("`--keyword` is deprecated"));
}

#[test]
fn agent_locate_and_expand_roundtrip() {
    let dir = TempDir::new().expect("tempdir");
    write_file(
        &dir.path().join("src/lib.rs"),
        "pub fn auth_flow() {}\npub fn call() { auth_flow(); }\n",
    );

    let mut locate_cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let locate_assert = locate_cmd
        .current_dir(dir.path())
        .args(["agent", "locate", "auth_flow"])
        .assert()
        .success();
    let locate_stdout = String::from_utf8(locate_assert.get_output().stdout.clone()).expect("utf8");
    let locate_json: Value = serde_json::from_str(&locate_stdout).expect("json2");
    let first_id = locate_json["results"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .expect("result id")
        .to_string();

    let mut expand_cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let expand_assert = expand_cmd
        .current_dir(dir.path())
        .args(["agent", "expand", "--id", &first_id, "-C", "1"])
        .assert()
        .success();
    let expand_stdout = String::from_utf8(expand_assert.get_output().stdout.clone()).expect("utf8");
    let expand_json: Value = serde_json::from_str(&expand_stdout).expect("expand json");

    assert_eq!(expand_json["meta"]["stage"], "expand");
    assert!(expand_json["meta"]["resolved_ids"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(expand_json["results"][0]["id"], first_id);
}
