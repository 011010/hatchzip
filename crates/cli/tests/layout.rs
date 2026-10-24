//! Archives whose entries can't all be created where they say: two entries
//! with one name, a link where other entries need a folder, a link other
//! entries would be written through. Each must be refused before anything is
//! touched, since links are only made once the archive is gone.

mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use common::*;
use tempfile::TempDir;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

fn stored() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
}

fn write(dir: &Path, name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

enum Tar<'a> {
    File(&'a str, &'a [u8]),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
    HardLink(&'a str, &'a str),
}

fn tar_bytes(entries: &[Tar]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for e in entries {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        match *e {
            Tar::File(name, data) => {
                h.set_size(data.len() as u64);
                b.append_data(&mut h, name, data).unwrap();
            }
            Tar::Dir(name) => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
                b.append_data(&mut h, name, &[][..]).unwrap();
            }
            Tar::Symlink(name, target) => {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }
            Tar::HardLink(name, target) => {
                h.set_entry_type(tar::EntryType::Link);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }
        }
    }
    b.into_inner().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// `check` and `extract` both refuse, with and without `--keep-archive`,
/// saying `why`, and leave the archive exactly as it was, with no output
/// and no side files.
fn assert_refused(archive: &Path, why: &str) {
    let before = fs::read(archive).unwrap();
    let out = archive.with_file_name("out");
    let (a, o) = (archive.to_str().unwrap(), out.to_str().unwrap());
    let check = hatchzip(&["check", a], &[]);
    let run = extract(archive, &out, &[]);
    let keep_check = hatchzip(&["check", a, "--keep-archive"], &[]);
    let keep_run = hatchzip(
        &["extract", a, "-o", o, "--keep-free", "0", "--keep-archive"],
        &[],
    );
    for (what, o) in [
        ("check", &check),
        ("extract", &run),
        ("check --keep-archive", &keep_check),
        ("extract --keep-archive", &keep_run),
    ] {
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{what} should refuse");
        assert!(stderr.contains(why), "{what}: {stderr}");
        assert!(stderr.contains("refused"), "{what}: {stderr}");
    }
    assert_eq!(fs::read(archive).unwrap(), before, "the archive changed");
    for p in side_files(archive) {
        assert!(!p.exists(), "{} was created", p.display());
    }
    assert!(!out.exists(), "output was written");
}

/// The reporter's symlink.zip: a file below `d`, and a link at `d`.
#[test]
fn zip_link_where_a_file_needs_a_folder_is_refused() {
    let tmp = TempDir::new().unwrap();
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("d/inner.txt", stored()).unwrap();
    z.write_all(b"inner\n").unwrap();
    z.add_symlink("d", "elsewhere", stored()).unwrap();
    let archive = write(tmp.path(), "symlink.zip", z.finish().unwrap().into_inner());
    assert_refused(
        &archive,
        "'d' is a symbolic link, but 'd/inner.txt' requires a folder",
    );
}

/// The reporter's dircollide.zip: the folder has an entry of its own.
#[test]
fn zip_link_at_a_folder_entry_is_refused() {
    let tmp = TempDir::new().unwrap();
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    z.add_directory("d/", stored()).unwrap();
    z.start_file("d/inner.txt", stored()).unwrap();
    z.write_all(b"inner\n").unwrap();
    z.add_symlink("d", "elsewhere", stored()).unwrap();
    let archive = write(
        tmp.path(),
        "dircollide.zip",
        z.finish().unwrap().into_inner(),
    );
    assert_refused(&archive, "'d' is both a symbolic link and a folder");
}

/// A case-insensitive filesystem would put both in one place, so they are
/// refused everywhere, as duplicate names are.
#[test]
fn zip_link_differing_only_in_case_is_refused() {
    let tmp = TempDir::new().unwrap();
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("D/inner.txt", stored()).unwrap();
    z.write_all(b"inner\n").unwrap();
    z.add_symlink("d", "elsewhere", stored()).unwrap();
    let archive = write(tmp.path(), "case.zip", z.finish().unwrap().into_inner());
    assert_refused(
        &archive,
        "'d' is a symbolic link, but 'D/inner.txt' requires a folder",
    );
}

/// The reporter's symlink.tar.
#[test]
fn tar_symlink_where_a_file_needs_a_folder_is_refused() {
    let tmp = TempDir::new().unwrap();
    let bytes = tar_bytes(&[
        Tar::File("d/inner.txt", b"inner\n"),
        Tar::Symlink("d", "elsewhere"),
    ]);
    let archive = write(tmp.path(), "symlink.tar", bytes.clone());
    assert_refused(
        &archive,
        "'d' is a symbolic link, but 'd/inner.txt' requires a folder",
    );
    let archive = write(tmp.path(), "symlink.tar.gz", gzip(&bytes));
    assert_refused(
        &archive,
        "'d' is a symbolic link, but 'd/inner.txt' requires a folder",
    );
}

#[test]
fn tar_hard_link_where_a_file_needs_a_folder_is_refused() {
    let tmp = TempDir::new().unwrap();
    let bytes = tar_bytes(&[
        Tar::File("a.txt", b"a\n"),
        Tar::File("d/inner.txt", b"inner\n"),
        Tar::HardLink("d", "a.txt"),
    ]);
    let archive = write(tmp.path(), "hardlink.tar", bytes.clone());
    assert_refused(
        &archive,
        "'d' is a hard link, but 'd/inner.txt' requires a folder",
    );
    let archive = write(tmp.path(), "hardlink.tar.gz", gzip(&bytes));
    assert_refused(
        &archive,
        "'d' is a hard link, but 'd/inner.txt' requires a folder",
    );
}

#[test]
fn tar_link_at_a_folder_entry_is_refused() {
    let tmp = TempDir::new().unwrap();
    let archive = write(
        tmp.path(),
        "dir.tar",
        tar_bytes(&[Tar::Dir("d"), Tar::Symlink("d", "elsewhere")]),
    );
    assert_refused(&archive, "'d' is both a symbolic link and a folder");
}

#[test]
fn tar_file_where_another_file_needs_a_folder_is_refused() {
    let tmp = TempDir::new().unwrap();
    let archive = write(
        tmp.path(),
        "file.tar",
        tar_bytes(&[
            Tar::File("d", b"plain\n"),
            Tar::File("d/inner.txt", b"inner\n"),
        ]),
    );
    assert_refused(
        &archive,
        "'d' is a file, but 'd/inner.txt' requires a folder",
    );
}

/// Made in order, `a/b` would be created inside whatever `a` points to.
#[test]
fn tar_link_inside_a_link_is_refused() {
    let tmp = TempDir::new().unwrap();
    let archive = write(
        tmp.path(),
        "through.tar",
        tar_bytes(&[Tar::Symlink("a", "/tmp"), Tar::Symlink("a/b", "x")]),
    );
    assert_refused(
        &archive,
        "'a' is a symbolic link, but 'a/b' requires a folder",
    );
}

#[test]
fn tar_hard_link_to_anything_but_a_file_in_the_archive_is_refused() {
    let tmp = TempDir::new().unwrap();
    for (name, entries) in [
        (
            "to-folder.tar",
            vec![
                Tar::File("d/inner.txt", b"inner\n"),
                Tar::HardLink("x", "d"),
            ],
        ),
        ("to-missing.tar", vec![Tar::HardLink("x", "missing.txt")]),
        (
            "to-link.tar",
            vec![
                Tar::File("a.txt", b"a\n"),
                Tar::Symlink("s", "a.txt"),
                Tar::HardLink("x", "s"),
            ],
        ),
    ] {
        let archive = write(tmp.path(), name, tar_bytes(&entries));
        assert_refused(&archive, "which is not a file in the archive");
    }
}

/// Links that don't collide with anything still extract.
#[cfg(unix)]
#[test]
fn links_next_to_folders_still_extract() {
    let tmp = TempDir::new().unwrap();
    let archive = write(
        tmp.path(),
        "fine.tar",
        tar_bytes(&[
            Tar::File("d/inner.txt", b"inner\n"),
            Tar::Symlink("e", "d"),
            Tar::HardLink("d/hard.txt", "d/inner.txt"),
            Tar::Symlink("d/soft", "inner.txt"),
        ]),
    );
    let out = tmp.path().join("out");
    assert_ok(&extract(&archive, &out, &[("HATCHZIP_BATCH_ENTRIES", "1")]));
    assert_eq!(fs::read(out.join("e/inner.txt")).unwrap(), b"inner\n");
    assert_eq!(fs::read(out.join("d/hard.txt")).unwrap(), b"inner\n");
    assert_eq!(fs::read(out.join("d/soft")).unwrap(), b"inner\n");
    assert_consumed(&archive);
}

/// The reporter's collision.zip: two entries named `dup.txt`. The zip crate
/// keys entries by name and keeps the last, so this is checked on the raw
/// central directory.
fn duplicate_zip() -> Vec<u8> {
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("dup.txt", stored()).unwrap();
    z.write_all(b"one").unwrap();
    z.start_file("dux.txt", stored()).unwrap();
    z.write_all(b"two").unwrap();
    let mut bytes = z.finish().unwrap().into_inner();
    // The writer refuses a second `dup.txt`, so the name is patched in, in
    // the local header and the central directory alike.
    let mut patched = 0;
    for i in 0..bytes.len() - 7 {
        if &bytes[i..i + 7] == b"dux.txt" {
            bytes[i..i + 7].copy_from_slice(b"dup.txt");
            patched += 1;
        }
    }
    assert_eq!(patched, 2);
    bytes
}

#[test]
fn zip_duplicate_names_are_refused() {
    let tmp = TempDir::new().unwrap();
    let archive = write(tmp.path(), "collision.zip", duplicate_zip());
    let list = hatchzip(&["list", archive.to_str().unwrap()], &[]);
    assert!(!list.status.success());
    assert_refused(&archive, "'dup.txt' appears more than once in the archive");
}

#[test]
fn zip_duplicate_names_in_a_self_extractor_are_refused() {
    let tmp = TempDir::new().unwrap();
    let mut bytes = vec![0x90; 4096];
    bytes.extend(duplicate_zip());
    let archive = write(tmp.path(), "collision.exe", bytes);
    assert_refused(&archive, "'dup.txt' appears more than once in the archive");
}

/// The journal a run left next to `archive`.
fn journal(archive: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(&side_files(archive)[0]).unwrap()).unwrap()
}

/// An extraction an older release started on such an archive runs until
/// only its links are left, and then can't make them. That state must say
/// what failed and what to do, keep the links until then, and finish once
/// the way is clear; restore must not send the person back to extract in a
/// loop.
#[cfg(unix)]
#[test]
fn links_that_cannot_be_made_at_the_end_can_be_fixed_and_resumed() {
    let tmp = TempDir::new().unwrap();
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("a.txt", stored()).unwrap();
    z.write_all(b"a\n").unwrap();
    z.add_symlink("d", "a.txt", stored()).unwrap();
    z.add_symlink("sub/e", "a.txt", stored()).unwrap();
    let archive = write(tmp.path(), "late.zip", z.finish().unwrap().into_inner());
    let out = tmp.path().join("out");

    // Stop right after the last entry is committed, then put a folder where
    // one link goes and a link where the other one's folder goes, as an
    // archive with colliding entries would have.
    let crashed = extract(
        &archive,
        &out,
        &[
            ("HATCHZIP_BATCH_ENTRIES", "1"),
            ("HATCHZIP_CRASH_AT", "after-journal"),
            ("HATCHZIP_CRASH_AFTER", "2"),
        ],
    );
    assert!(!crashed.status.success());
    assert_eq!(journal(&archive)["committed_len"], 0);
    fs::create_dir(out.join("d")).unwrap();
    fs::write(out.join("d/mine.txt"), b"mine\n").unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("sub")).unwrap();

    for _ in 0..2 {
        let run = extract(&archive, &out, &[]);
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success());
        assert!(
            stderr.contains("every file is extracted, but 2 links could not be created"),
            "{stderr}"
        );
        assert!(
            stderr.contains("'d' (symbolic link to 'a.txt'): a folder already exists"),
            "{stderr}"
        );
        assert!(
            stderr.contains("'sub' is a link, and this one would be created through it"),
            "{stderr}"
        );
        assert!(stderr.contains("resume the extraction"), "{stderr}");
        // Nothing is lost or written anywhere it shouldn't be, however
        // often it is retried.
        assert_eq!(fs::read(out.join("d/mine.txt")).unwrap(), b"mine\n");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(journal(&archive)["links"].as_array().unwrap().len(), 2);
    }

    let restore = hatchzip(&["restore", archive.to_str().unwrap()], &[]);
    let stderr = String::from_utf8_lossy(&restore.stderr);
    assert!(!restore.status.success());
    assert!(stderr.contains("nothing to restore"), "{stderr}");
    assert!(stderr.contains("only the links remain"), "{stderr}");

    fs::rename(out.join("d"), out.join("d-folder")).unwrap();
    fs::remove_file(out.join("sub")).unwrap();
    assert_ok(&extract(&archive, &out, &[]));
    assert_eq!(fs::read_link(out.join("d")).unwrap(), Path::new("a.txt"));
    assert!(fs::symlink_metadata(out.join("sub")).unwrap().is_dir());
    assert_eq!(
        fs::read_link(out.join("sub/e")).unwrap(),
        Path::new("a.txt")
    );
    assert_consumed(&archive);
}

/// In a run over several archives, a colliding one stops the run before it
/// is touched, after the ones before it are done.
#[test]
fn a_colliding_archive_among_several_is_refused_untouched() {
    let files = sample_files();
    let tmp = TempDir::new().unwrap();
    let a = zip_of(tmp.path(), "a.zip", &files);
    let b = write(
        tmp.path(),
        "b.tar",
        tar_bytes(&[
            Tar::File("d/inner.txt", b"inner\n"),
            Tar::Symlink("d", "elsewhere"),
        ]),
    );
    let c = zip_of(tmp.path(), "c.zip", &files);
    let (b_before, c_before) = (snapshot(&b), snapshot(&c));
    let arg = |p: &Path| p.to_str().unwrap().to_string();

    let run = hatchzip(
        &[
            "extract",
            &arg(&a),
            &arg(&b),
            &arg(&c),
            "--keep-free",
            "0",
            "-y",
        ],
        &[],
    );
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!run.status.success(), "{out}");
    assert!(
        out.contains("'d' is a symbolic link, but 'd/inner.txt' requires a folder"),
        "{out}"
    );
    assert!(out.contains("Stopped at archive 2 of 3."), "{out}");
    assert_tree(&tmp.path().join("a"), &files);
    assert_consumed(&a);
    assert_unchanged(&b_before);
    assert_no_side_files(&b);
    assert!(!tmp.path().join("b").exists());
    assert_unchanged(&c_before);
    assert!(!tmp.path().join("c").exists());
}
