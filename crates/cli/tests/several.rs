//! Several archives in one command: extracted one after another, each
//! checked just before it starts, and the first one that fails stops the
//! run with a summary and its exit status.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::*;
use tempfile::TempDir;

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn arg(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// `a.zip`, `b.tar` and `c.tar.gz`, each extracting to a folder named
/// after it.
fn three(dir: &Path, files: &Files) -> [PathBuf; 3] {
    [
        zip_of(dir, "a.zip", files),
        tar_of(dir, "b.tar", files),
        targz_of(dir, "c.tar.gz", files),
    ]
}

/// Flips a byte inside `name`'s data, so the archive fails its checksum.
/// Only for ZIP: tar has no checksum over data.
fn damage(archive: &Path, files: &Files, name: &str) {
    let mut bytes = fs::read(archive).unwrap();
    let needle = &files[name][5000..5032];
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("stored data to damage");
    bytes[at] ^= 0xFF;
    fs::write(archive, bytes).unwrap();
}

#[test]
fn extracts_several_archives_one_after_another() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let [a, b, c] = three(tmp.path(), &files);

    let run = hatchzip(
        &[
            "extract",
            arg(&a),
            arg(&b),
            arg(&c),
            "--keep-free",
            "0",
            "-y",
        ],
        &[],
    );
    assert_ok(&run);
    let out = text(&run);
    assert!(out.contains("Archive 1 of 3:"), "{out}");
    assert!(out.contains("Archive 3 of 3:"), "{out}");
    assert!(out.contains("All 3 archives extracted."), "{out}");
    for (archive, folder) in [(&a, "a"), (&b, "b"), (&c, "c")] {
        assert_tree(&tmp.path().join(folder), &files);
        assert_consumed(archive);
    }
}

#[test]
fn several_archives_can_all_be_kept() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let archives = three(tmp.path(), &files);
    let before: Vec<_> = archives.iter().map(|a| snapshot(a)).collect();
    let mut args = vec!["extract", "--keep-archive", "--keep-free", "0"];
    args.extend(archives.iter().map(|a| arg(a)));

    assert_ok(&hatchzip(&args, &[]));
    for (archive, folder) in archives.iter().zip(["a", "b", "c"]) {
        assert_tree(&tmp.path().join(folder), &files);
        assert_no_side_files(archive);
    }
    for b in &before {
        assert_unchanged(b);
    }
}

/// Refused before anything is extracted: the second archive is damaged,
/// and --verify-first finds it before it is touched.
#[test]
fn a_failure_in_the_middle_stops_with_a_summary() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let a = tar_of(tmp.path(), "a.tar", &files);
    let b = zip_of(tmp.path(), "b.zip", &files);
    let c = zip_of(tmp.path(), "c.zip", &files);
    damage(&b, &files, "game/data/big.pak");
    let (b_before, c_before) = (snapshot(&b), snapshot(&c));

    let run = hatchzip(
        &[
            "extract",
            arg(&a),
            arg(&b),
            arg(&c),
            "--verify-first",
            "--keep-free",
            "0",
            "-y",
        ],
        &[],
    );
    // The status of the archive that stopped the run: damaged.
    assert_eq!(run.status.code(), Some(4), "{}", text(&run));
    let out = text(&run);
    assert!(out.contains("Stopped at archive 2 of 3."), "{out}");
    assert!(
        out.contains(&format!("{}  (extracted)", a.display())),
        "{out}"
    );
    assert!(
        out.contains(&format!("{}  (not extracted)", b.display())),
        "{out}"
    );
    assert!(
        out.contains(&format!("{}  (not started)", c.display())),
        "{out}"
    );

    assert_tree(&tmp.path().join("a"), &files);
    assert_consumed(&a);
    assert_unchanged(&b_before);
    assert_no_side_files(&b);
    assert_unchanged(&c_before);
    assert!(!tmp.path().join("c").exists());
}

/// Damage found halfway through an archive stops the run there; its
/// progress is saved and the summary says so.
#[test]
fn a_failure_halfway_through_an_archive_keeps_its_progress() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let a = tar_of(tmp.path(), "a.tar", &files);
    let b = zip_of(tmp.path(), "b.zip", &files);
    let c = zip_of(tmp.path(), "c.zip", &files);
    damage(&b, &files, "game/data/big.pak");
    let c_before = snapshot(&c);

    let run = hatchzip(
        &[
            "extract",
            arg(&a),
            arg(&b),
            arg(&c),
            "--keep-free",
            "0",
            "-y",
        ],
        &[("HATCHZIP_BATCH_ENTRIES", "1")],
    );
    assert_eq!(run.status.code(), Some(1), "{}", text(&run));
    let out = text(&run);
    assert!(
        out.contains(&format!("{}  (not extracted, progress saved)", b.display())),
        "{out}"
    );
    assert!(out.contains("(not started)"), "{out}");
    assert_consumed(&a);
    assert!(side_files(&b)[0].exists(), "b's journal is kept");
    assert_unchanged(&c_before);
}

#[test]
fn check_test_and_list_go_through_every_archive() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let a = tar_of(tmp.path(), "a.tar", &files);
    let b = zip_of(tmp.path(), "b.zip", &files);
    let c = zip_of(tmp.path(), "c.zip", &files);
    let missing = tmp.path().join("missing.zip");

    // The missing one fails; the one after it is still checked.
    let check = hatchzip(
        &["check", arg(&a), arg(&missing), arg(&c), "--keep-free", "0"],
        &[],
    );
    assert_eq!(check.status.code(), Some(1));
    let out = text(&check);
    assert!(out.contains("c.zip  "), "{out}");
    assert!(out.contains("Each archive is checked against the space available now"));

    damage(&b, &files, "game/data/big.pak");
    let test = hatchzip(&["test", arg(&a), arg(&b), arg(&c)], &[]);
    assert_eq!(test.status.code(), Some(4));
    assert_eq!(text(&test).matches("Test passed:").count(), 2);

    let list = hatchzip(&["list", arg(&a), arg(&c)], &[]);
    assert_ok(&list);
    let out = text(&list);
    assert!(out.contains(&a.display().to_string()) && out.contains(&c.display().to_string()));
    // Windows lists entries with backslashes.
    assert_eq!(
        out.replace('\\', "/").matches("game/data/big.pak").count(),
        2
    );
}

/// A misspelled name is found before the first archive is removed.
#[test]
fn a_missing_archive_is_refused_before_anything_starts() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let a = zip_of(tmp.path(), "a.zip", &files);
    let before = snapshot(&a);
    let run = hatchzip(
        &[
            "extract",
            arg(&a),
            arg(&tmp.path().join("b.zip")),
            "--keep-free",
            "0",
            "-y",
        ],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(text(&run).contains("does not exist"), "{}", text(&run));
    assert_unchanged(&before);
    assert!(!tmp.path().join("a").exists());
}

/// A shell glob such as `game.7z.*` names every volume of one archive.
#[test]
fn volumes_of_one_split_archive_count_once() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let first = split_up(&sevenz_of(tmp.path(), "game.7z", &files, true), 3);
    let volumes: Vec<PathBuf> = snapshot(&first).into_iter().map(|(p, _)| p).collect();
    let mut args = vec!["extract", "--keep-free", "0", "-y"];
    args.extend(volumes.iter().map(|v| arg(v)));

    let run = hatchzip(&args, &[]);
    assert_ok(&run);
    assert!(!text(&run).contains("Archive 1 of"), "{}", text(&run));
    assert_tree(&tmp.path().join("game"), &files);
    for v in &volumes {
        assert!(!v.exists());
    }
}
