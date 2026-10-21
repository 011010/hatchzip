//! End to end tests: build real archives, run the real binary, compare what
//! lands on disk with what went into the archive.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

type Files = BTreeMap<String, Vec<u8>>;

fn noise(len: usize, seed: u64) -> Vec<u8> {
    // xorshift, so the data doesn't compress and tests stay deterministic.
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn sample_files() -> Files {
    let mut files = Files::new();
    files.insert("game/data/big.pak".into(), noise(3_000_000, 1));
    files.insert("game/data/small.pak".into(), noise(70_000, 2));
    files.insert(
        "game/config/settings.ini".into(),
        b"volume=7\n".repeat(20_000),
    );
    files.insert("game/empty.txt".into(), Vec::new());
    files.insert(
        "game/naïve résumé/日本語.txt".into(),
        "unicode names survive\n".into(),
    );
    for i in 0..40 {
        files.insert(
            format!("game/many/file{i:02}.txt"),
            format!("file number {i}\n").into(),
        );
    }
    files
}

fn build_zip(dir: &Path, files: &Files) -> PathBuf {
    build_zip_with(dir, files, false)
}

/// Each file gets a compression method in turn, so every decoder is covered.
/// `zip64` forces ZIP64 records on every entry, as archives over 4 GB have.
fn build_zip_with(dir: &Path, files: &Files, zip64: bool) -> PathBuf {
    let methods = [
        CompressionMethod::Stored,
        CompressionMethod::Deflated,
        CompressionMethod::Zstd,
        CompressionMethod::Bzip2,
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_directory("game/saves/", SimpleFileOptions::default())
        .unwrap();
    for (i, (name, data)) in files.iter().enumerate() {
        let opts = SimpleFileOptions::default()
            .compression_method(methods[i % methods.len()])
            .unix_permissions(0o640)
            .large_file(zip64);
        zip.start_file(name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    let path = dir.join("game.zip");
    fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    path
}

fn hatchzip(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hatchzip"));
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn extract(archive: &Path, out: &Path, env: &[(&str, &str)]) -> Output {
    let (a, o) = (archive.to_str().unwrap(), out.to_str().unwrap());
    hatchzip(&["extract", a, "-o", o, "--keep-free", "0", "--yes"], env)
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "hatchzip failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_tree(out: &Path, files: &Files) {
    for (name, data) in files {
        let got = fs::read(out.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs from the archive");
    }
    assert!(out.join("game/saves").is_dir());
    let mut stray = Vec::new();
    for entry in walk(out) {
        let rel = entry
            .strip_prefix(out)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if entry.is_file() && !files.contains_key(&rel) {
            stray.push(rel);
        }
    }
    assert!(stray.is_empty(), "unexpected files left behind: {stray:?}");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            all.extend(walk(&p));
        }
        all.push(p);
    }
    all
}

fn side_files(archive: &Path) -> [PathBuf; 2] {
    let s = archive.as_os_str().to_string_lossy();
    [
        PathBuf::from(format!("{s}.hatchzip")),
        PathBuf::from(format!("{s}.hatchzip-tail")),
    ]
}

fn assert_consumed(archive: &Path) {
    assert!(!archive.exists(), "the archive should be gone");
    for p in side_files(archive) {
        assert!(!p.exists(), "{} should be cleaned up", p.display());
    }
}

#[test]
fn extracts_every_method_and_deletes_the_archive() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip(tmp.path(), &files);
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

#[test]
fn extracts_zip64_archives_and_resumes_them() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip_with(tmp.path(), &files, true);
    let out = tmp.path().join("out");
    let crash = [
        ("HATCHZIP_BATCH_ENTRIES", "1"),
        ("HATCHZIP_CRASH_AT", "after-journal"),
        ("HATCHZIP_CRASH_AFTER", "5"),
    ];

    assert!(!extract(&archive, &out, &crash).status.success());
    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

#[test]
fn many_small_batches_give_the_same_result() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip(tmp.path(), &files);
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[("HATCHZIP_BATCH_ENTRIES", "1")]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

#[test]
fn resumes_after_a_crash_at_any_point() {
    for point in ["after-extract", "after-journal"] {
        for after in [0, 1, 7, 30] {
            let tmp = TempDir::new().unwrap();
            let files = sample_files();
            let archive = build_zip(tmp.path(), &files);
            let original = fs::metadata(&archive).unwrap().len();
            let out = tmp.path().join("out");
            let after = after.to_string();

            let crashed = extract(
                &archive,
                &out,
                &[
                    ("HATCHZIP_BATCH_ENTRIES", "1"),
                    ("HATCHZIP_CRASH_AT", point),
                    ("HATCHZIP_CRASH_AFTER", &after),
                ],
            );
            assert!(
                !crashed.status.success(),
                "{point}#{after} should have crashed"
            );
            let left = fs::metadata(&archive).unwrap().len();
            assert!(left <= original, "{point}#{after}: archive grew");
            for p in side_files(&archive) {
                assert!(p.exists(), "{point}#{after}: {} is missing", p.display());
            }

            assert_ok(&extract(&archive, &out, &[]));
            assert_tree(&out, &files);
            assert_consumed(&archive);
        }
    }
}

#[test]
fn resuming_twice_in_a_row_is_harmless() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip(tmp.path(), &files);
    let out = tmp.path().join("out");
    let env = [
        ("HATCHZIP_BATCH_ENTRIES", "1"),
        ("HATCHZIP_CRASH_AT", "after-journal"),
    ];

    assert!(!extract(&archive, &out, &env).status.success());
    assert!(!extract(&archive, &out, &env).status.success());
    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files);
}

#[test]
fn check_and_test_change_nothing() {
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &sample_files());
    let before = fs::read(&archive).unwrap();
    let a = archive.to_str().unwrap();

    assert_ok(&hatchzip(&["check", a, "--keep-free", "0"], &[]));
    assert_ok(&hatchzip(&["test", a], &[]));
    assert_eq!(fs::read(&archive).unwrap(), before);
    for p in side_files(&archive) {
        assert!(!p.exists());
    }
}

#[test]
fn refuses_to_start_without_enough_space() {
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &sample_files());
    let before = fs::read(&archive).unwrap();
    let (a, o) = (archive.to_str().unwrap(), tmp.path().join("out"));

    let run = hatchzip(
        &[
            "extract",
            a,
            "-o",
            o.to_str().unwrap(),
            "--keep-free",
            "900TB",
            "-y",
        ],
        &[],
    );
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("Not enough space"));
    assert_eq!(fs::read(&archive).unwrap(), before);
    assert!(!o.exists());
}

#[test]
fn refuses_a_non_empty_output_folder() {
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &sample_files());
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("mine.txt"), "keep me").unwrap();

    assert!(!extract(&archive, &out, &[]).status.success());
    assert_eq!(fs::read_to_string(out.join("mine.txt")).unwrap(), "keep me");
    assert!(archive.exists());
}

#[test]
fn refuses_paths_that_escape_the_output_folder() {
    let tmp = TempDir::new().unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("../escaped.txt", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"nope").unwrap();
    let archive = tmp.path().join("evil.zip");
    fs::write(&archive, zip.finish().unwrap().into_inner()).unwrap();
    let before = fs::read(&archive).unwrap();

    let run = extract(&archive, &tmp.path().join("out"), &[]);
    assert!(!run.status.success());
    assert!(!tmp.path().join("escaped.txt").exists());
    assert_eq!(fs::read(&archive).unwrap(), before);
}

#[test]
fn refuses_encrypted_archives_before_touching_them() {
    let tmp = TempDir::new().unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "pw");
    zip.start_file("secret.txt", opts).unwrap();
    zip.write_all(b"hidden").unwrap();
    let archive = tmp.path().join("locked.zip");
    fs::write(&archive, zip.finish().unwrap().into_inner()).unwrap();
    let before = fs::read(&archive).unwrap();

    let run = extract(&archive, &tmp.path().join("out"), &[]);
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("password"));
    assert_eq!(fs::read(&archive).unwrap(), before);
}

/// Flips a byte inside the stored data of `name`.
fn corrupt(archive: &Path, files: &Files, name: &str) {
    let mut bytes = fs::read(archive).unwrap();
    let needle = &files[name][1000..1032];
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("stored data")
        + 5;
    bytes[at] ^= 0xFF;
    fs::write(archive, bytes).unwrap();
}

#[test]
fn verify_first_catches_damage_before_anything_is_deleted() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip(tmp.path(), &files);
    // Sorted first in the map, so it's Stored and its bytes are findable.
    corrupt(&archive, &files, "game/config/settings.ini");
    let before = fs::read(&archive).unwrap();
    let (a, out) = (archive.to_str().unwrap(), tmp.path().join("out"));

    assert!(!hatchzip(&["test", a], &[]).status.success());
    let run = hatchzip(
        &[
            "extract",
            a,
            "-o",
            out.to_str().unwrap(),
            "--keep-free",
            "0",
            "--verify-first",
            "-y",
        ],
        &[],
    );
    assert!(!run.status.success());
    assert_eq!(fs::read(&archive).unwrap(), before);
}

#[test]
fn damage_found_midway_stops_without_losing_the_damaged_entry() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let archive = build_zip(tmp.path(), &files);
    corrupt(&archive, &files, "game/config/settings.ini");
    let out = tmp.path().join("out");

    let run = extract(&archive, &out, &[("HATCHZIP_BATCH_ENTRIES", "1")]);
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("settings.ini"));
    // Everything after the damaged entry was extracted and freed; the
    // damaged one and everything before it is still in the archive.
    assert!(archive.exists());
    assert!(
        fs::read(&archive)
            .unwrap()
            .windows(9)
            .any(|w| w == b"volume=7\n")
    );
    assert!(!out.join("game/config/settings.ini").exists());
    assert!(!out.join("game/config/settings.ini.hatchzip-part").exists());
}

#[test]
fn handles_a_self_extractor_stub_in_front() {
    let tmp = TempDir::new().unwrap();
    let files = sample_files();
    let plain = build_zip(tmp.path(), &files);
    let mut bytes = b"MZ pretend this is a self-extracting exe stub".repeat(30);
    bytes.extend(fs::read(&plain).unwrap());
    let archive = tmp.path().join("setup.exe");
    fs::write(&archive, bytes).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[("HATCHZIP_BATCH_ENTRIES", "3")]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

#[test]
fn a_resumed_run_stays_in_its_output_folder() {
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &sample_files());
    let out = tmp.path().join("out");
    let env = [
        ("HATCHZIP_BATCH_ENTRIES", "1"),
        ("HATCHZIP_CRASH_AT", "after-journal"),
    ];
    assert!(!extract(&archive, &out, &env).status.success());

    let elsewhere = extract(&archive, &tmp.path().join("other"), &[]);
    assert!(!elsewhere.status.success());
    assert!(String::from_utf8_lossy(&elsewhere.stderr).contains("already half extracted"));
}

#[cfg(unix)]
#[test]
fn restores_symlinks_and_permissions_but_not_setuid() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "bin/tool",
        SimpleFileOptions::default().unix_permissions(0o4755),
    )
    .unwrap();
    zip.write_all(b"#!/bin/sh\necho hi\n").unwrap();
    zip.add_symlink("bin/alias", "tool", SimpleFileOptions::default())
        .unwrap();
    let archive = tmp.path().join("tools.zip");
    fs::write(&archive, zip.finish().unwrap().into_inner()).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[]));
    let mode = fs::metadata(out.join("bin/tool"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o755);
    assert_eq!(
        fs::read_link(out.join("bin/alias")).unwrap(),
        Path::new("tool")
    );
}
