//! Helpers shared by the end to end suites: sample content, running the real
//! binary, and checking what landed on disk.

// Each suite is its own crate and uses a different subset.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub type Files = BTreeMap<String, Vec<u8>>;

/// Every builder also adds this as an explicit directory entry.
pub const EMPTY_DIR: &str = "game/saves";

pub fn noise(len: usize, seed: u64) -> Vec<u8> {
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

pub fn sample_files() -> Files {
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

pub fn hatchzip(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hatchzip"));
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

pub fn extract(archive: &Path, out: &Path, env: &[(&str, &str)]) -> Output {
    let (a, o) = (archive.to_str().unwrap(), out.to_str().unwrap());
    hatchzip(&["extract", a, "-o", o, "--keep-free", "0", "--yes"], env)
}

pub fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "hatchzip failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn assert_tree(out: &Path, files: &Files) {
    for (name, data) in files {
        let got = fs::read(out.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(got == *data, "{name} differs from the archive");
    }
    assert!(out.join(EMPTY_DIR).is_dir(), "{EMPTY_DIR} is missing");
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
        if p.is_dir() && !p.is_symlink() {
            all.extend(walk(&p));
        }
        all.push(p);
    }
    all
}

pub fn side_files(archive: &Path) -> [PathBuf; 2] {
    let s = archive.as_os_str().to_string_lossy();
    [
        PathBuf::from(format!("{s}.hatchzip")),
        PathBuf::from(format!("{s}.hatchzip-tail")),
    ]
}

pub fn assert_consumed(archive: &Path) {
    assert!(!archive.exists(), "the archive should be gone");
    for p in side_files(archive) {
        assert!(!p.exists(), "{} should be cleaned up", p.display());
    }
}

/// Kills a run at both crash points after `after` commits, resumes it, and
/// checks the result. `build` makes a fresh archive in the given folder and
/// returns the path to pass to `extract`.
pub fn crash_and_resume(build: impl Fn(&Path) -> PathBuf, files: &Files, afters: &[u32]) {
    for point in ["after-extract", "after-journal"] {
        for after in afters {
            let tmp = tempfile::TempDir::new().unwrap();
            let archive = build(tmp.path());
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
            for p in side_files(&archive) {
                assert!(p.exists(), "{point}#{after}: {} is missing", p.display());
            }

            assert_ok(&extract(&archive, &out, &[]));
            assert_tree(&out, files);
            assert_consumed(&archive);
        }
    }
}

/// Cuts `bytes` into `parts` volumes named `<base>.001`, `<base>.002`, ...
pub fn split(bytes: &[u8], base: &Path, parts: usize) -> Vec<PathBuf> {
    let size = bytes.len().div_ceil(parts);
    bytes
        .chunks(size)
        .enumerate()
        .map(|(i, chunk)| {
            let mut name = base.as_os_str().to_owned();
            name.push(format!(".{:03}", i + 1));
            let p = PathBuf::from(name);
            fs::write(&p, chunk).unwrap();
            p
        })
        .collect()
}
