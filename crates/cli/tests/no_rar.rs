//! A build without the `rar` feature has no UnRAR in it, and refuses RAR
//! archives before anything is touched, saying where a build that reads
//! them comes from.

#![cfg(not(feature = "rar"))]

mod common;

use std::fs;
use std::path::Path;

use common::*;
use tempfile::TempDir;

#[test]
fn rar_archives_are_refused_untouched() {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rar/rar5");
    let tmp = TempDir::new().unwrap();
    for e in fs::read_dir(&from).unwrap() {
        let e = e.unwrap();
        fs::copy(e.path(), tmp.path().join(e.file_name())).unwrap();
    }
    let before: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            let bytes = fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    // The first volume, a later one, and the commands that only read.
    for (cmd, name) in [
        ("extract", "set.part1.rar"),
        ("extract", "set.part3.rar"),
        ("list", "set.part1.rar"),
        ("check", "set.part1.rar"),
    ] {
        let archive = tmp.path().join(name);
        let out = if cmd == "extract" {
            extract(&archive, &tmp.path().join("out"), &[])
        } else {
            hatchzip(&[cmd, archive.to_str().unwrap()], &[])
        };
        assert_eq!(out.status.code(), Some(1), "{cmd} {name}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("does not include RAR support")
                && err.contains("github.com/xevrion/hatchzip/releases"),
            "{cmd} {name}: {err}"
        );
        assert_unchanged(&before);
        assert!(!tmp.path().join("out").exists());
        assert_no_side_files(&tmp.path().join("set.part1.rar"));
    }
}
