//! The command line contract: exit codes scripts can rely on, quiet output,
//! listing, and the generated completions and man page.

mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use common::*;
use tempfile::TempDir;
use zip::write::{SimpleFileOptions, ZipWriter};

fn small_zip(dir: &Path) -> (PathBuf, Files) {
    let files = sample_files();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_directory(format!("{EMPTY_DIR}/"), SimpleFileOptions::default())
        .unwrap();
    for (name, data) in &files {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    let path = dir.join("game.zip");
    fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    (path, files)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn not_enough_space_exits_with_3() {
    let tmp = TempDir::new().unwrap();
    let (archive, _) = small_zip(tmp.path());
    let a = archive.to_str().unwrap();

    let check = hatchzip(&["check", a, "--keep-free", "900TB"], &[]);
    assert_eq!(check.status.code(), Some(3));
    let run = hatchzip(&["extract", a, "--keep-free", "900TB", "-y"], &[]);
    assert_eq!(run.status.code(), Some(3));
    assert!(archive.exists());
}

#[test]
fn damage_exits_with_4() {
    let tmp = TempDir::new().unwrap();
    let (archive, files) = small_zip(tmp.path());
    let mut bytes = fs::read(&archive).unwrap();
    let needle = &files["game/data/big.pak"][5000..5032];
    let at = bytes.windows(32).position(|w| w == needle).unwrap();
    bytes[at] ^= 0xFF;
    fs::write(&archive, &bytes).unwrap();
    let a = archive.to_str().unwrap();

    assert_eq!(hatchzip(&["test", a], &[]).status.code(), Some(4));
    let run = hatchzip(&["x", a, "--keep-free", "0", "--verify-first", "-y"], &[]);
    assert_eq!(run.status.code(), Some(4));
    assert_eq!(fs::read(&archive).unwrap(), bytes);
}

#[test]
fn quiet_prints_nothing_on_success() {
    let tmp = TempDir::new().unwrap();
    let (archive, files) = small_zip(tmp.path());
    let out = tmp.path().join("out");
    let run = hatchzip(
        &[
            "-q",
            "extract",
            archive.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--keep-free",
            "0",
            "-y",
        ],
        &[],
    );
    assert_ok(&run);
    assert_eq!(stdout(&run), "");
    assert_tree(&out, &files);
}

#[test]
fn refusing_to_run_unattended_without_yes_changes_nothing() {
    let tmp = TempDir::new().unwrap();
    let (archive, _) = small_zip(tmp.path());
    let before = fs::read(&archive).unwrap();
    let run = hatchzip(
        &["extract", archive.to_str().unwrap(), "--keep-free", "0"],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("--yes"));
    assert_eq!(fs::read(&archive).unwrap(), before);
    assert!(!side_files(&archive)[0].exists());
}

#[test]
fn list_shows_every_entry_without_touching_anything() {
    let tmp = TempDir::new().unwrap();
    let (archive, files) = small_zip(tmp.path());
    let before = fs::read(&archive).unwrap();
    let run = hatchzip(&["list", archive.to_str().unwrap()], &[]);
    assert_ok(&run);
    let text = stdout(&run);
    for name in files.keys() {
        assert!(
            text.contains(name.as_str()),
            "{name} missing from the listing"
        );
    }
    assert!(text.contains(&format!("{EMPTY_DIR}/")));
    assert_eq!(fs::read(&archive).unwrap(), before);
}

#[test]
fn completions_exist_for_every_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let run = hatchzip(&["completions", shell], &[]);
        assert_ok(&run);
        let script = stdout(&run);
        assert!(
            script.contains("extract") && script.contains("keep-free"),
            "{shell} script looks wrong"
        );
    }
}

#[test]
fn man_page_renders() {
    let run = hatchzip(&["man"], &[]);
    assert_ok(&run);
    let page = stdout(&run);
    assert!(page.starts_with(".ie") || page.contains(".TH"), "not roff");
    assert!(page.contains("hatchzip") && page.contains("Exit status"));
}

#[test]
fn help_and_version_work() {
    assert_ok(&hatchzip(&["--version"], &[]));
    let help = hatchzip(&["--help"], &[]);
    assert_ok(&help);
    assert!(stdout(&help).contains("Examples:"));
    assert_eq!(hatchzip(&["extract"], &[]).status.code(), Some(2));
}
