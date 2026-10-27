//! Every file is first written as `<name>.hatchzip-part` and renamed once it
//! is complete, and the progress files next to the archive are written
//! through a temporary file too. Whatever is already at one of those names
//! was not made by the run, so it is never written through, replaced or
//! deleted.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

use common::*;
use tempfile::TempDir;

fn files() -> Files {
    let mut files = Files::new();
    files.insert("one.txt".into(), noise(20_000, 1));
    files.insert("sub/two.txt".into(), noise(20_000, 2));
    files.insert("three.txt".into(), noise(20_000, 3));
    files
}

fn keep_run(archive: &Path, out: &Path) -> Output {
    let (a, o) = (archive.to_str().unwrap(), out.to_str().unwrap());
    hatchzip(
        &["extract", a, "-o", o, "--keep-free", "0", "--keep-archive"],
        &[],
    )
}

fn assert_failed(run: &Output, says: &str) {
    // Windows prints paths with backslashes.
    let stderr = String::from_utf8_lossy(&run.stderr).replace('\\', "/");
    assert!(!run.status.success(), "should have failed");
    assert!(stderr.contains(says), "{stderr}");
}

/// A file at the name an entry is first written under is refused before
/// anything is touched, in place and keeping the archive, and stays as it
/// was. It may be a person's file, or one a run stopped long ago left.
#[test]
fn a_file_at_a_part_file_name_is_refused_and_kept() {
    let tmp = TempDir::new().unwrap();
    let archive = tar_of(tmp.path(), "game.tar", &files());
    let before = snapshot(&archive);
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("sub")).unwrap();
    let part = out.join("sub/two.txt.hatchzip-part");
    fs::write(&part, b"a file of the person's").unwrap();

    for run in [extract(&archive, &out, &[]), keep_run(&archive, &out)] {
        assert_failed(&run, "two.txt.hatchzip-part already exists");
        assert_failed(&run, "Nothing was modified");
    }
    assert_eq!(fs::read(&part).unwrap(), b"a file of the person's");
    assert!(!out.join("sub/two.txt").exists());
    assert_unchanged(&before);
    assert_no_side_files(&archive);
}

/// A link at a part file's name, left by another program or an earlier
/// archive, is refused up front, and nothing reaches the file it points to
/// or creates the one a dangling link names.
#[cfg(unix)]
#[test]
fn a_link_at_a_part_file_name_is_refused() {
    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, b"outside the output folder").unwrap();
    let nowhere = tmp.path().join("nowhere.txt");
    for (target, name) in [(&outside, "one.txt"), (&nowhere, "three.txt")] {
        let archive = tar_of(tmp.path(), "game.tar", &files());
        let before = snapshot(&archive);
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let part = out.join(format!("{name}.hatchzip-part"));
        std::os::unix::fs::symlink(target, &part).unwrap();

        for run in [extract(&archive, &out, &[]), keep_run(&archive, &out)] {
            assert_failed(&run, &format!("{name}.hatchzip-part already exists"));
        }
        assert_unchanged(&before);
        assert_no_side_files(&archive);
        fs::remove_dir_all(&out).unwrap();
    }
    assert_eq!(fs::read(&outside).unwrap(), b"outside the output folder");
    assert!(!nowhere.exists());
}

/// A resume deletes the files of a crashed batch, but never a link, and a
/// link that appeared at a part file's name since stops the run when that
/// file's turn comes instead of being written through. Once it is moved,
/// the extraction completes.
#[cfg(unix)]
#[test]
fn a_link_at_a_part_file_name_stops_a_resume() {
    let tmp = TempDir::new().unwrap();
    let archive = tar_of(tmp.path(), "game.tar", &files());
    let out = tmp.path().join("out");
    let crash = [
        ("HATCHZIP_BATCH_ENTRIES", "1"),
        ("HATCHZIP_CRASH_AT", "after-journal"),
    ];
    assert!(!extract(&archive, &out, &crash).status.success());
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, b"outside the output folder").unwrap();
    let part = out.join("one.txt.hatchzip-part");
    std::os::unix::fs::symlink(&outside, &part).unwrap();

    assert_failed(
        &extract(&archive, &out, &[]),
        "one.txt.hatchzip-part already exists",
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside the output folder");
    assert!(fs::symlink_metadata(&part).unwrap().is_symlink());

    fs::remove_file(&part).unwrap();
    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files());
    assert_consumed(&archive);
}

/// The same for a compressed tar, whose files are opened one after another
/// from the stream, after a crash at its first resume point.
#[cfg(unix)]
#[test]
fn a_link_at_a_part_file_name_stops_a_resumed_stream() {
    let mut files = Files::new();
    files.insert("a.bin".into(), noise(3 << 20, 4));
    files.insert("z.txt".into(), noise(20_000, 5));
    let tmp = TempDir::new().unwrap();
    let archive = targz_of(tmp.path(), "game.tar.gz", &files);
    let out = tmp.path().join("out");
    let step = ("HATCHZIP_STREAM_STEP", "1048576");
    let crash = [step, ("HATCHZIP_CRASH_AT", "stream-after-journal")];
    assert!(!extract(&archive, &out, &crash).status.success());
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, b"outside the output folder").unwrap();
    let part = out.join("z.txt.hatchzip-part");
    std::os::unix::fs::symlink(&outside, &part).unwrap();

    assert_failed(
        &extract(&archive, &out, &[step]),
        "z.txt.hatchzip-part already exists",
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside the output folder");

    fs::remove_file(&part).unwrap();
    assert_ok(&extract(&archive, &out, &[step]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

/// The journal and the saved tail are each written through a temporary file
/// of their own. A link at one of those names is replaced, not written
/// through, and a file at the name both used to share is left alone.
#[cfg(unix)]
#[test]
fn the_progress_files_are_never_written_through_a_link() {
    let tmp = TempDir::new().unwrap();
    let archive = tar_of(tmp.path(), "game.tar", &files());
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, b"outside the output folder").unwrap();
    for side in side_files(&archive) {
        let mut tmp_name = side.into_os_string();
        tmp_name.push(".tmp");
        std::os::unix::fs::symlink(&outside, tmp_name).unwrap();
    }
    let shared = tmp.path().join("game.tar.tmp");
    fs::write(&shared, b"a file of the person's").unwrap();

    let out = tmp.path().join("out");
    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files());
    assert_eq!(fs::read(&outside).unwrap(), b"outside the output folder");
    assert_eq!(fs::read(&shared).unwrap(), b"a file of the person's");
}
