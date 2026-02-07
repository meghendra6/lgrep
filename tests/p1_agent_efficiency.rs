// SPDX-License-Identifier: MIT OR Apache-2.0

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;
use tempfile::TempDir;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write file");
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(dir: &Path) {
    run_git(dir, &["init", "-q"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "test"]);
}

fn commit_all(dir: &Path, message: &str) {
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "--quiet", "-m", message]);
}

#[test]
fn keyword_mode_cache_hits_for_normalized_index_queries() {
    let dir = TempDir::new().expect("tempdir");
    write_file(&dir.path().join("src/lib.rs"), "pub fn needle_token() {}\n");

    let mut index_cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    index_cmd
        .current_dir(dir.path())
        .args(["index", "--embeddings", "off"])
        .assert()
        .success();

    let mut first = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let first_assert = first
        .current_dir(dir.path())
        .args([
            "--format",
            "json2",
            "search",
            "Needle   token",
            "--agent-cache",
        ])
        .assert()
        .success();
    let first_stdout = String::from_utf8(first_assert.get_output().stdout.clone()).expect("utf8");
    let first_json: Value = serde_json::from_str(&first_stdout).expect("json");
    assert_eq!(first_json["meta"]["cache_hit"], false);

    let mut second = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let second_assert = second
        .current_dir(dir.path())
        .args([
            "--format",
            "json2",
            "search",
            "needle token",
            "--agent-cache",
        ])
        .assert()
        .success();
    let second_stdout = String::from_utf8(second_assert.get_output().stdout.clone()).expect("utf8");
    let second_json: Value = serde_json::from_str(&second_stdout).expect("json");
    assert_eq!(second_json["meta"]["cache_hit"], true);
}

#[test]
fn search_changed_filters_to_modified_files() {
    let dir = TempDir::new().expect("tempdir");
    init_git_repo(dir.path());
    write_file(&dir.path().join("src/a.rs"), "pub fn needle() {}\n");
    write_file(&dir.path().join("src/b.rs"), "pub fn needle() {}\n");
    commit_all(dir.path(), "initial");

    write_file(
        &dir.path().join("src/a.rs"),
        "pub fn needle() { let _ = 1; }\n",
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
            "--changed",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let json: Value = serde_json::from_str(&stdout).expect("json");
    let results = json.as_array().expect("results");
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r["path"] == "src/a.rs"));
}

#[test]
fn symbols_and_references_honor_changed_filter() {
    let dir = TempDir::new().expect("tempdir");
    init_git_repo(dir.path());
    write_file(
        &dir.path().join("src/a.rs"),
        "pub fn target_fn() {}\npub fn call_a() { target_fn(); }\n",
    );
    write_file(
        &dir.path().join("src/b.rs"),
        "pub fn target_fn() {}\npub fn call_b() { target_fn(); }\n",
    );
    commit_all(dir.path(), "initial");

    write_file(
        &dir.path().join("src/a.rs"),
        "pub fn target_fn() {}\npub fn call_a() { target_fn(); let _ = 1; }\n",
    );

    let mut symbols_cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let symbols_assert = symbols_cmd
        .current_dir(dir.path())
        .args(["--format", "json", "symbols", "target_fn", "--changed"])
        .assert()
        .success();
    let symbols_stdout =
        String::from_utf8(symbols_assert.get_output().stdout.clone()).expect("utf8");
    let symbols_json: Value = serde_json::from_str(&symbols_stdout).expect("json");
    let symbols = symbols_json.as_array().expect("array");
    assert!(!symbols.is_empty());
    assert!(symbols.iter().all(|r| r["path"] == "src/a.rs"));

    let mut refs_cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let refs_assert = refs_cmd
        .current_dir(dir.path())
        .args(["--format", "json", "references", "target_fn", "--changed"])
        .assert()
        .success();
    let refs_stdout = String::from_utf8(refs_assert.get_output().stdout.clone()).expect("utf8");
    let refs_json: Value = serde_json::from_str(&refs_stdout).expect("json");
    let refs = refs_json.as_array().expect("array");
    assert!(!refs.is_empty());
    assert!(refs.iter().all(|r| r["path"] == "src/a.rs"));
}

#[test]
fn json2_path_alias_and_boilerplate_suppression_work() {
    let dir = TempDir::new().expect("tempdir");
    write_file(
        &dir.path().join("src/a.ts"),
        "import { x } from './shared';\nexport const a = x;\n",
    );
    write_file(
        &dir.path().join("src/b.ts"),
        "import { x } from './shared';\nexport const b = x;\n",
    );

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cgrep"));
    let assert = cmd
        .current_dir(dir.path())
        .args([
            "--format",
            "json2",
            "search",
            "import",
            "--no-index",
            "--path-alias",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let json: Value = serde_json::from_str(&stdout).expect("json");

    let aliases = json["meta"]["path_aliases"].as_object().expect("alias map");
    assert!(!aliases.is_empty());

    let results = json["results"].as_array().expect("results");
    assert!(!results.is_empty());
    for result in results {
        let alias = result["path"].as_str().expect("alias");
        assert!(aliases.contains_key(alias));
        assert_eq!(result["snippet"], "[boilerplate suppressed]");
    }
}
