//! Every compression method the decoders claim, on archives made by the real
//! 7-Zip 25.01 (`tests/fixtures/codecs/`), each extracted in place and
//! compared byte for byte with the content the fixtures were made from.
//!
//! The fixtures were made from three files:
//!   game/text.txt   400 lines of text
//!   game/noise.bin  3000 bytes from a fixed linear congruential generator
//!   game/tool.exe   "MZ" and a byte ramp, so the BCJ filter has x86-ish input
//! with, for example, `7z a -tzip -mm=PPMd zip-ppmd.zip game` and
//! `7z a -t7z -m0=BCJ -m1=LZMA2 7z-bcj.7z game`.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use tempfile::TempDir;

fn expected() -> Vec<(&'static str, Vec<u8>)> {
    let text: String = (0..400)
        .map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n"))
        .collect();
    let mut x: u64 = 12345;
    let noise = (0..3000)
        .map(|_| {
            x = (x * 1_103_515_245 + 12_345) & 0x7fff_ffff;
            (x >> 16) as u8
        })
        .collect();
    let mut tool = b"MZ".to_vec();
    tool.extend((0..20).flat_map(|_| 0..=255u8));
    vec![
        ("game/text.txt", text.into_bytes()),
        ("game/noise.bin", noise),
        ("game/tool.exe", tool),
    ]
}

fn extract_fixture(name: &str) {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codecs")
        .join(name);
    let archive = tmp.path().join(name);
    fs::copy(&src, &archive).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[]));
    for (path, data) in expected() {
        let got = fs::read(out.join(path)).unwrap_or_else(|e| panic!("{name}: {path}: {e}"));
        assert!(got == data, "{name}: {path} differs");
    }
    assert_consumed(&archive);
}

#[test]
fn zip_deflate64() {
    extract_fixture("zip-deflate64.zip");
}

#[test]
fn zip_bzip2() {
    extract_fixture("zip-bzip2.zip");
}

#[test]
fn zip_lzma() {
    extract_fixture("zip-lzma.zip");
}

#[test]
fn zip_ppmd() {
    extract_fixture("zip-ppmd.zip");
}

#[test]
fn sevenz_lzma() {
    extract_fixture("7z-lzma.7z");
}

#[test]
fn sevenz_bcj_and_lzma2() {
    extract_fixture("7z-bcj.7z");
}

#[test]
fn sevenz_bzip2() {
    extract_fixture("7z-bzip2.7z");
}

#[test]
fn sevenz_deflate() {
    extract_fixture("7z-deflate.7z");
}

#[test]
fn sevenz_ppmd() {
    extract_fixture("7z-ppmd.7z");
}
