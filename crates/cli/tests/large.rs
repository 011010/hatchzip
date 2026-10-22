//! Archives past 4 GiB, without using 4 GiB of disk.
//!
//! A ZIP may hold unreferenced bytes between entries, and a file that is
//! seeked past its end gets a hole the filesystem doesn't store. So entries
//! can sit beyond the 4 GiB mark in an archive that takes a few kilobytes,
//! which exercises the 64-bit offset paths a real 180 GB archive uses: ZIP64
//! records, truncation past 4 GiB, and resuming from there.
//!
//! Unix only: NTFS writes real zeros into such a hole unless the file is
//! marked sparse first, which would put 4 GiB on the CI runner's disk.
#![cfg(unix)]

mod common;

use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use common::*;
use tempfile::TempDir;

const GAP: u64 = 4_400_000_000;

/// Writes a stored (uncompressed) ZIP64 archive by hand: the first file at
/// offset 0, the rest after a hole that ends at `GAP`.
fn build_sparse_zip(path: &Path, files: &[(&str, Vec<u8>)]) {
    let mut f = File::create(path).unwrap();
    let mut central = Vec::new();
    for (i, (name, data)) in files.iter().enumerate() {
        if i == 1 {
            f.seek(SeekFrom::Start(GAP)).unwrap();
        }
        let offset = f.stream_position().unwrap();
        let crc = crc32(data);
        let mut local = Vec::new();
        local.extend(0x0403_4b50u32.to_le_bytes());
        local.extend(45u16.to_le_bytes()); // version needed: ZIP64
        local.extend([0u8; 4]); // flags, method stored
        local.extend([0u8; 4]); // time, date
        local.extend(crc.to_le_bytes());
        local.extend((data.len() as u32).to_le_bytes());
        local.extend((data.len() as u32).to_le_bytes());
        local.extend((name.len() as u16).to_le_bytes());
        local.extend(0u16.to_le_bytes());
        local.extend(name.as_bytes());
        f.write_all(&local).unwrap();
        f.write_all(data).unwrap();

        // Offsets past 4 GiB live in a ZIP64 extra field.
        let big = offset > u32::MAX as u64;
        let extra: Vec<u8> = if big {
            [1u16.to_le_bytes(), 8u16.to_le_bytes()]
                .concat()
                .into_iter()
                .chain(offset.to_le_bytes())
                .collect()
        } else {
            Vec::new()
        };
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend((45u16 | 3 << 8).to_le_bytes()); // made by Unix
        central.extend(45u16.to_le_bytes());
        central.extend([0u8; 8]); // flags, method, time, date
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend((extra.len() as u16).to_le_bytes());
        central.extend([0u8; 6]); // comment length, disk, internal attributes
        central.extend((0o100644u32 << 16).to_le_bytes());
        central.extend((if big { u32::MAX } else { offset as u32 }).to_le_bytes());
        central.extend(name.as_bytes());
        central.extend(extra);
    }
    let cd_offset = f.stream_position().unwrap();
    f.write_all(&central).unwrap();
    let eocd64 = f.stream_position().unwrap();
    let n = files.len() as u64;
    let mut tail = Vec::new();
    tail.extend(0x0606_4b50u32.to_le_bytes());
    tail.extend(44u64.to_le_bytes());
    tail.extend(45u16.to_le_bytes());
    tail.extend(45u16.to_le_bytes());
    tail.extend([0u8; 8]); // this disk, central directory disk
    tail.extend(n.to_le_bytes());
    tail.extend(n.to_le_bytes());
    tail.extend((central.len() as u64).to_le_bytes());
    tail.extend(cd_offset.to_le_bytes());
    tail.extend(0x0706_4b50u32.to_le_bytes());
    tail.extend(0u32.to_le_bytes());
    tail.extend(eocd64.to_le_bytes());
    tail.extend(1u32.to_le_bytes());
    tail.extend(0x0605_4b50u32.to_le_bytes());
    tail.extend([0u8; 4]);
    tail.extend((n as u16).to_le_bytes());
    tail.extend((n as u16).to_le_bytes());
    tail.extend((central.len() as u32).to_le_bytes());
    tail.extend(u32::MAX.to_le_bytes());
    tail.extend(0u16.to_le_bytes());
    f.write_all(&tail).unwrap();
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn entries_past_4_gib_extract_and_resume() {
    let tmp = TempDir::new().unwrap();
    let files = [
        ("game/before-the-gap.txt", b"below 4 GiB\n".repeat(100)),
        ("game/after-gap-1.bin", noise(200_000, 7)),
        ("game/after-gap-2.txt", b"well past 4 GiB\n".repeat(500)),
        ("game/last.txt", b"the end\n".to_vec()),
    ];
    let archive = tmp.path().join("big.zip");
    build_sparse_zip(&archive, &files);
    assert!(fs::metadata(&archive).unwrap().len() > GAP);

    let a = archive.to_str().unwrap();
    let check = hatchzip(&["check", a, "--keep-free", "0"], &[]);
    assert_ok(&check);
    assert!(String::from_utf8_lossy(&check.stdout).contains("4.4 GB zip"));

    // Crash after the first commit: the cut lands past 4 GiB.
    let out = tmp.path().join("out");
    let crash = [
        ("HATCHZIP_BATCH_ENTRIES", "1"),
        ("HATCHZIP_CRASH_AT", "after-journal"),
        ("HATCHZIP_CRASH_AFTER", "1"),
    ];
    assert!(!extract(&archive, &out, &crash).status.success());
    let left = fs::metadata(&archive).unwrap().len();
    assert!(
        left > GAP && left < GAP + 300_000,
        "archive is {left} bytes"
    );

    assert_ok(&extract(&archive, &out, &[]));
    for (name, data) in &files {
        assert_eq!(&fs::read(out.join(name)).unwrap(), data, "{name}");
    }
    assert_consumed(&archive);
}
