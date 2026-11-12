//! `extract --nested`: after the archives given, the archives found inside
//! the folders they left are extracted too, by signature and one level deep.

mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use common::*;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn extract_nested(archive: &Path, out: &Path) -> std::process::Output {
    hatchzip(
        &[
            "extract",
            archive.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--nested",
            "--keep-free",
            "0",
            "--yes",
        ],
        &[],
    )
}

#[test]
fn the_archives_inside_are_extracted_one_level() {
    let tmp = TempDir::new().unwrap();
    let deeper = zip_bytes(&[("deeper.txt", b"three")]);
    let first = zip_bytes(&[("first.txt", b"one"), ("deeper.zip", &deeper)]);
    let mut files = Files::new();
    files.insert("second.txt".into(), b"two".to_vec());
    let second = targz_of(tmp.path(), "second.tar.gz", &files);
    let second = fs::read(second).unwrap();
    let outer = zip_bytes(&[
        ("first.zip", &first),
        ("second.tar.gz", &second),
        ("notes.zip", b"not an archive at all"),
    ]);
    let archive = tmp.path().join("outer.zip");
    fs::write(&archive, outer).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract_nested(&archive, &out));
    assert!(!archive.exists(), "the outer archive is removed");
    assert_eq!(fs::read(out.join("first/first.txt")).unwrap(), b"one");
    assert_eq!(fs::read(out.join("second/second.txt")).unwrap(), b"two");
    assert!(
        !out.join("first.zip").exists() && !out.join("second.tar.gz").exists(),
        "the nested archives are removed in place"
    );
    assert!(
        out.join("notes.zip").exists(),
        "a name is not a signature, so this stays"
    );
    assert!(
        out.join("first/deeper.zip").exists() && !out.join("first/deeper").exists(),
        "one level only"
    );
}

#[test]
fn without_the_flag_the_archives_stay_where_they_landed() {
    let tmp = TempDir::new().unwrap();
    let inner = zip_bytes(&[("first.txt", b"one")]);
    let outer = zip_bytes(&[("first.zip", &inner)]);
    let archive = tmp.path().join("outer.zip");
    fs::write(&archive, outer).unwrap();
    let out = tmp.path().join("out");

    let run = hatchzip(
        &[
            "extract",
            archive.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--keep-free",
            "0",
            "--yes",
        ],
        &[],
    );
    assert_ok(&run);
    assert!(out.join("first.zip").exists());
    assert!(!out.join("first").exists());
}

fn kept_parent_is_not_rescanned(relative: bool, case_alias: bool) {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    let inner = zip_bytes(&[("first.txt", b"one")]);
    let outer = zip_bytes(&[("inner.zip", &inner)]);
    let archive = out.join("Parent.zip");
    fs::write(&archive, &outer).unwrap();
    let name = if case_alias {
        "out/PARENT.ZIP"
    } else {
        "out/Parent.zip"
    };
    if !tmp.path().join(name).exists() {
        // This variant only applies where both spellings name the same file.
        return;
    }
    let given = if relative {
        std::path::PathBuf::from(name)
    } else {
        tmp.path().join(name)
    };
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_hatchzip"))
        .current_dir(tmp.path())
        .env("HATCHZIP_STATE_DIR", state_dir())
        .args(["extract", "--nested", "--keep-archive", "--keep-free", "0"])
        .arg(given)
        .args(["-o", "out"])
        .output()
        .unwrap();
    assert_ok(&run);
    assert_eq!(fs::read(&archive).unwrap(), outer);
    assert_eq!(fs::read(out.join("inner.zip")).unwrap(), inner);
    assert_eq!(fs::read(out.join("inner/first.txt")).unwrap(), b"one");
    assert!(
        !out.join("Parent").exists(),
        "the given parent must not be extracted again as a nested archive"
    );
}

#[test]
fn a_kept_parent_given_by_relative_path_is_not_nested() {
    kept_parent_is_not_rescanned(true, false);
}

#[test]
fn a_kept_parent_given_with_another_case_is_not_nested() {
    kept_parent_is_not_rescanned(false, true);
}

/// A link inside the output can point anywhere; the scan must not follow it.
#[cfg(unix)]
#[test]
fn a_link_is_not_followed_or_extracted() {
    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside.zip");
    fs::write(&outside, zip_bytes(&[("x.txt", b"x")])).unwrap();

    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    tar.append_link(&mut header, "link.zip", "../outside.zip")
        .unwrap();
    let archive = tmp.path().join("outer.tar");
    fs::write(&archive, tar.into_inner().unwrap()).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract_nested(&archive, &out));
    let link = out.join("link.zip");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link survives"
    );
    assert!(outside.exists(), "what it points at is untouched");
    assert!(!out.join("link").exists(), "nothing was extracted");
}

/// The scan must not follow a Windows link either. An archive cannot carry
/// one, so the link is left in the output between two runs, the way a user
/// would. Windows makes links only with Developer Mode on or as admin; the
/// test steps aside where the machine cannot.
#[cfg(windows)]
#[test]
fn a_windows_link_is_not_followed_or_extracted() {
    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside.zip");
    fs::write(&outside, zip_bytes(&[("x.txt", b"x")])).unwrap();

    let first = zip_bytes(&[("holder.zip", &zip_bytes(&[("y.txt", b"y")]))]);
    let archive = tmp.path().join("outer1.zip");
    fs::write(&archive, first).unwrap();
    let out = tmp.path().join("out");
    assert_ok(&extract_nested(&archive, &out));

    if std::os::windows::fs::symlink_file(&outside, out.join("link.zip")).is_err() {
        return;
    }

    let second = zip_bytes(&[("holder2.zip", &zip_bytes(&[("z.txt", b"z")]))]);
    let archive = tmp.path().join("outer2.zip");
    fs::write(&archive, second).unwrap();
    assert_ok(&extract_nested(&archive, &out));

    let link = out.join("link.zip");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link survives"
    );
    assert!(outside.exists(), "what it points at is untouched");
    assert!(!out.join("link").exists(), "nothing was extracted");
}
