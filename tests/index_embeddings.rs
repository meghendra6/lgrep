// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

use assert_cmd::cargo::cargo_bin_cmd;
use cgrep::embedding::EmbeddingStorage;

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
chunk_lines = 2
chunk_overlap = 0
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
    write_file(
        &file_path,
        "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbb\ncccccccccccccccccccc\n",
    );

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let file_path_str = file_path.to_string_lossy();

    let info = storage
        .get_file_info(file_path_str.as_ref())
        .unwrap()
        .unwrap();
    assert!(info.chunk_count > 0);

    let chunks = storage.get_chunks_for_path(file_path_str.as_ref()).unwrap();
    assert!(!chunks.is_empty());
}

#[test]
fn index_precompute_skips_up_to_date_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(
        &file_path,
        "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbb\ncccccccccccccccccccc\n",
    );

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let file_path_str = file_path.to_string_lossy().to_string();
    let first_chunk_id = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_chunks_for_path(&file_path_str).unwrap()[0].id
    };

    run_index(dir.path(), &["--embeddings", "precompute"]);

    let second_chunk_id = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_chunks_for_path(&file_path_str).unwrap()[0].id
    };

    assert_eq!(second_chunk_id, first_chunk_id);
}

#[test]
fn index_embeddings_force_regenerates() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(
        &file_path,
        "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbb\ncccccccccccccccccccc\n",
    );

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    let file_path_str = file_path.to_string_lossy().to_string();
    let first_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_chunks_for_path(&file_path_str).unwrap()[0].created_at
    };

    std::thread::sleep(Duration::from_millis(1100));
    run_index(
        dir.path(),
        &["--embeddings", "precompute", "--embeddings-force"],
    );

    let second_created_at = {
        let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
        storage.get_chunks_for_path(&file_path_str).unwrap()[0].created_at
    };

    assert!(second_created_at > first_created_at);
}

#[test]
fn index_removes_embeddings_for_deleted_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let a_path = dir.path().join("src").join("a.rs");
    let b_path = dir.path().join("src").join("b.rs");
    write_file(
        &a_path,
        "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbb\ncccccccccccccccccccc\n",
    );
    write_file(
        &b_path,
        "dddddddddddddddddddd\neeeeeeeeeeeeeeeeeeee\nffffffffffffffffffff\n",
    );

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    // Remove b.rs and re-index.
    fs::remove_file(&b_path).unwrap();
    run_index(dir.path(), &["--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let a_path_str = a_path.to_string_lossy().to_string();
    let b_path_str = b_path.to_string_lossy().to_string();
    assert!(storage.get_file_info(&b_path_str).unwrap().is_none());
    assert!(storage.get_file_info(&a_path_str).unwrap().is_some());
}

#[test]
fn index_removes_embeddings_for_binary_files() {
    let dir = TempDir::new().unwrap();
    write_dummy_embeddings_config(dir.path());

    let file_path = dir.path().join("src").join("lib.rs");
    write_file(
        &file_path,
        "aaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbb\ncccccccccccccccccccc\n",
    );

    run_index(dir.path(), &["--force", "--embeddings", "precompute"]);

    // Overwrite with a binary-ish file (contains NUL).
    write_bytes(&file_path, b"hello\0world");
    run_index(dir.path(), &["--embeddings", "precompute"]);

    let storage = EmbeddingStorage::open_default(dir.path()).unwrap();
    let file_path_str = file_path.to_string_lossy().to_string();
    assert!(storage.get_file_info(&file_path_str).unwrap().is_none());
    assert!(storage
        .get_chunks_for_path(&file_path_str)
        .unwrap()
        .is_empty());
}
