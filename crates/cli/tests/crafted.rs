//! Archives put together by hand, with layouts no ordinary tool writes,
//! aimed at what in-place extraction relies on. Each one is either extracted
//! whole or refused before anything is touched.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use tempfile::TempDir;

/// 7z's variable-length numbers, for the values these tests need.
fn number(out: &mut Vec<u8>, n: u64) {
    if n < 0x80 {
        out.push(n as u8);
    } else if n < 0x4000 {
        out.extend([0x80 | (n >> 8) as u8, n as u8]);
    } else {
        out.push(0xFF);
        out.extend(n.to_le_bytes());
    }
}

/// A 7z header for stored (Copy) blocks of one packed stream each. `packs`
/// are the packed stream sizes, which may list more streams than there are
/// blocks; `blocks` the files of each block with their sizes; `dirs` folders,
/// which have no data.
fn header(packs: &[u64], blocks: &[&[(&str, u64)]], dirs: &[&str]) -> Vec<u8> {
    // Header, main streams info, pack info at position 0.
    let mut h = vec![0x01, 0x04, 0x06, 0x00];
    number(&mut h, packs.len() as u64);
    h.push(0x09);
    for &p in packs {
        number(&mut h, p);
    }
    h.push(0x00);

    // Unpack info: one Copy coder per block, then each block's size.
    h.extend([0x07, 0x0B]);
    number(&mut h, blocks.len() as u64);
    h.push(0x00);
    for _ in blocks {
        h.extend([0x01, 0x01, 0x00]);
    }
    h.push(0x0C);
    for files in blocks {
        number(&mut h, files.iter().map(|f| f.1).sum());
    }
    h.push(0x00);

    // Sub-streams: how many files each block holds, and the size of all
    // but its last one.
    h.extend([0x08, 0x0D]);
    for files in blocks {
        number(&mut h, files.len() as u64);
    }
    h.push(0x09);
    for files in blocks {
        for f in files.iter().take(files.len().saturating_sub(1)) {
            number(&mut h, f.1);
        }
    }
    h.extend([0x00, 0x00]);

    // Files: those of the blocks in order, then the folders.
    let names: Vec<&str> = blocks
        .iter()
        .flat_map(|b| b.iter().map(|f| f.0))
        .chain(dirs.iter().copied())
        .collect();
    h.push(0x05);
    number(&mut h, names.len() as u64);
    if !dirs.is_empty() {
        let streams = names.len() - dirs.len();
        let mut bits = vec![0u8; names.len().div_ceil(8)];
        for i in streams..names.len() {
            bits[i / 8] |= 0x80 >> (i % 8);
        }
        h.push(0x0E);
        number(&mut h, bits.len() as u64);
        h.extend(bits);
    }
    let mut utf16 = vec![0x00];
    for n in &names {
        for c in n.encode_utf16().chain([0]) {
            utf16.extend(c.to_le_bytes());
        }
    }
    h.push(0x11);
    number(&mut h, utf16.len() as u64);
    h.extend(utf16);
    h.extend([0x00, 0x00]);
    h
}

/// A whole 7z: the start header, then `body`, whose header is the `len`
/// bytes found `at` bytes in.
fn sevenz(body: &[u8], at: usize, len: usize) -> Vec<u8> {
    let mut start = Vec::with_capacity(20);
    start.extend((at as u64).to_le_bytes());
    start.extend((len as u64).to_le_bytes());
    start.extend(crc32fast::hash(&body[at..at + len]).to_le_bytes());
    let mut out = b"7z\xBC\xAF\x27\x1C\x00\x04".to_vec();
    out.extend(crc32fast::hash(&start).to_le_bytes());
    out.extend(start);
    out.extend(body);
    out
}

/// The packed streams in order, with the header right after them, as 7-Zip
/// lays an archive out.
fn sevenz_with_header_last(packs: &[&[u8]], header: &[u8]) -> Vec<u8> {
    let mut body = packs.concat();
    let at = body.len();
    body.extend(header);
    sevenz(&body, at, header.len())
}

fn refused(archive: &Path, out: &Path, says: &str) {
    let before = fs::read(archive).unwrap();
    let run = extract(archive, out, &[]);
    assert!(!run.status.success(), "the archive should be refused");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains(says), "{stderr}");
    assert_eq!(fs::read(archive).unwrap(), before);
    assert_no_side_files(archive);
}

/// A block with no packed bytes starts where the next block does. Committed
/// on its own, it would truncate that block's data before it was extracted;
/// its empty file is made with the folders instead.
#[test]
fn a_7z_block_without_packed_bytes_does_not_cut_off_the_next() {
    let tmp = TempDir::new().unwrap();
    let data = noise(3000, 1);
    let last = noise(500, 2);
    let h = header(
        &[0, 3000, 500],
        &[
            &[("empty.txt", 0)],
            &[("data.bin", 3000)],
            &[("last.bin", 500)],
        ],
        &[EMPTY_DIR],
    );
    let archive = tmp.path().join("shared.7z");
    fs::write(&archive, sevenz_with_header_last(&[&data, &last], &h)).unwrap();
    let out = tmp.path().join("out");

    assert_ok(&extract(&archive, &out, &[("HATCHZIP_BATCH_ENTRIES", "1")]));
    let mut files = Files::new();
    files.insert("empty.txt".into(), Vec::new());
    files.insert("data.bin".into(), data);
    files.insert("last.bin".into(), last);
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

/// The same block claiming data it has no bytes for is damage.
#[test]
fn a_7z_block_without_packed_bytes_but_with_data_is_refused() {
    let tmp = TempDir::new().unwrap();
    let data = noise(3000, 1);
    let h = header(
        &[0, 3000],
        &[&[("ghost.bin", 10)], &[("data.bin", 3000)]],
        &[],
    );
    let archive = tmp.path().join("ghost.7z");
    fs::write(&archive, sevenz_with_header_last(&[&data], &h)).unwrap();
    refused(&archive, &tmp.path().join("out"), "header is inconsistent");
}
