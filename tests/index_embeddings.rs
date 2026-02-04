// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::path::Path;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin_cmd;
use cgrep::embedding::EmbeddingStorage;
use predicates::str::contains;
use rusqlite::Connection;
use tempfile::TempDir;

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn write_dummy_embeddings_config(repo_root: &Path) {
    fs::write(
        repo_root.join(".cgreprc.toml"),
        r#"
[embeddings]
provider = "dummy"
"#,
    )
    .unwrap();
}

fn run_index(repo_root: &Path, extra_args: &[&str]) {
    let mut cmd = cargo_bin_cmd!("cgrep");
    cmd.arg("index")
        .arg("--path")
        .arg(repo_root)
        .args(extra_args);
    cmd.assert().success();
}

#[test]
fn index_precompute_creates_embeddings_db() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\nfn beta() {}\n");

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let file_path_str = file_path.to_string_lossy();

    let symbols = storage
        .get_symbols_for_path(file_path_str.as_ref())
        .unwrap();
    assert!(!symbols.is_empty());
    assert!(symbols.iter().any(|s| s.symbol_name == "alpha"));
}

#[test]
fn index_precompute_skips_up_to_date_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\nfn beta() {}\n");

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let file_path_str = file_path.to_string_lossy().to_string();
    let first_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_symbols_for_path(&file_path_str).unwrap()[0].created_at
    };

    run_index(dir.path(), &["--embeddings", "precompute"]);

    let second_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_symbols_for_path(&file_path_str).unwrap()[0].created_at
    };

    assert_eq!(second_created_at, first_created_at);
}

#[test]
fn index_embeddings_force_regenerates() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\nfn beta() {}\n");

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let file_path_str = file_path.to_string_lossy().to_string();
    let first_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_symbols_for_path(&file_path_str).unwrap()[0].created_at
    };

    std::thread::sleep(Duration::from_millis(1100));
    run_index(
        dir.path(),
        &["--embeddings", "precompute", "--embeddings-force"],
    );

    let second_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_symbols_for_path(&file_path_str).unwrap()[0].created_at
    };

    assert!(second_created_at > first_created_at);
}

#[test]
fn index_removes_embeddings_for_deleted_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let a_path = dir.path().join("src").join("a.rs");
    let b_path = dir.path().join("src").join("b.rs");
    write_file(&a_path, "fn alpha() {}\n");
    write_file(&b_path, "fn beta() {}\n");

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    // Remove b.rs and re-index.
    fs::remove_file(&b_path).unwrap();
    run_index(dir.path(), &["--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let a_path_str = a_path.to_string_lossy().to_string();
    let b_path_str = b_path.to_string_lossy().to_string();
    assert!(!storage.get_symbols_for_path(&a_path_str).unwrap().is_empty());
    assert!(storage.get_symbols_for_path(&b_path_str).unwrap().is_empty());
}

#[test]
fn index_removes_embeddings_for_binary_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\n");

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    // Overwrite with a binary-ish file (contains NUL).
    write_bytes(&file_path, b"hello\0world");
    run_index(dir.path(), &["--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let file_path_str = file_path.to_string_lossy().to_string();
    assert!(storage
        .get_symbols_for_path(&file_path_str)
        .unwrap()
        .is_empty());
}

#[test]
fn index_precompute_errors_on_schema_mismatch() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\n");

    // Create index first so .cgrep exists.
    run_index(dir.path(), &["--force", "--embeddings", "off"]);

    let db_path = dir.path().join(".cgrep").join("embeddings.sqlite");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);\n\
         INSERT INTO meta (key, value) VALUES ('unit', 'chunk');",
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("cgrep");
    cmd.arg("index")
        .arg("--path")
        .arg(dir.path())
        .args(["--embeddings", "precompute"]);
    cmd.assert()
        .failure()
        .stderr(contains("embeddings-force"));
}

#[test]
fn index_auto_skips_on_schema_mismatch() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(&file_path, "fn alpha() {}\n");

    // Create index first so .cgrep exists.
    run_index(dir.path(), &["--force", "--embeddings", "off"]);

    let db_path = dir.path().join(".cgrep").join("embeddings.sqlite");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);\n\
         INSERT INTO meta (key, value) VALUES ('unit', 'chunk');",
    )
    .unwrap();

    run_index(dir.path(), &["--embeddings", "auto"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    assert_eq!(storage.count_symbols().unwrap(), 0);
}
