//! Large entries extracted while their own archive bytes are freed: killed
//! at every step of a resume point, resumed, and compared byte for byte.
//!
//! `HATCHZIP_STREAM_STEP` shrinks the step between resume points to 2 MiB,
//! so a 24 MB entry crosses a dozen of them, and makes every entry of at
//! least two steps take the streaming path even when it would fit whole.

mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use common::*;
use tempfile::TempDir;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

const STEP: &str = "2097152";
const HUGE: &str = "game/data/huge.bin";

fn files() -> Files {
    let mut files = sample_files();
    files.insert(HUGE.into(), noise(24_000_000, 9));
    files
}

/// Text-like data with bursts of noise: deflate gives it Huffman blocks
/// whose matches reach back across block boundaries, and stored blocks in
/// between, so resuming needs the window and the exact bit position.
fn text(len: usize) -> Vec<u8> {
    let words: Vec<&str> = "hatch every byte of the archive back into files while the disk \
        stays nearly full and a crash at any moment loses nothing 42 1337 zran"
        .split(' ')
        .collect();
    let bursts = noise(len / 50 + 1, 77);
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut out = Vec::with_capacity(len + 64);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(words[(x % words.len() as u64) as usize].as_bytes());
        out.push(if x & 0x700 == 0 { b'\n' } else { b' ' });
        if x & 0x3FFF == 0 {
            let at = (x as usize >> 20) % (bursts.len() - 600);
            out.extend_from_slice(&bursts[at..at + 600]);
        }
    }
    out.truncate(len);
    out
}

fn deflate_files() -> Files {
    let mut files = sample_files();
    files.insert(HUGE.into(), text(40_000_000));
    files
}

/// How many bytes the huge entry takes in a ZIP archive.
fn packed_size(archive: &Path) -> u64 {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive).unwrap()).unwrap();
    zip.by_name(HUGE).unwrap().compressed_size()
}

/// `method` is used for the huge entry; the rest are stored or deflated in
/// turn, so the ordinary path runs alongside.
fn build_zip(dir: &Path, files: &Files, method: CompressionMethod) -> PathBuf {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_directory("game/saves/", SimpleFileOptions::default())
        .unwrap();
    for (i, (name, data)) in files.iter().enumerate() {
        let m = if name == HUGE {
            method
        } else if i % 2 == 0 {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        zip.start_file(name, SimpleFileOptions::default().compression_method(m))
            .unwrap();
        zip.write_all(data).unwrap();
    }
    let path = dir.join("game.zip");
    fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    path
}

fn build_tar(dir: &Path, files: &Files) -> PathBuf {
    let mut b = tar::Builder::new(Vec::new());
    let mut dir_header = tar::Header::new_gnu();
    dir_header.set_entry_type(tar::EntryType::Directory);
    dir_header.set_mode(0o755);
    dir_header.set_size(0);
    b.append_data(&mut dir_header, EMPTY_DIR, std::io::empty())
        .unwrap();
    for (name, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(1_700_000_000);
        b.append_data(&mut h, name, data.as_slice()).unwrap();
    }
    let path = dir.join("game.tar");
    fs::write(&path, b.into_inner().unwrap()).unwrap();
    path
}

/// Bytes the archive occupies on disk, where the system can say.
#[cfg(unix)]
fn allocated(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(fs::metadata(path).ok()?.blocks() * 512)
}

#[cfg(not(unix))]
fn allocated(_path: &Path) -> Option<u64> {
    None
}

fn stream_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut env = vec![
        ("HATCHZIP_STREAM_STEP", STEP),
        ("HATCHZIP_BATCH_ENTRIES", "1"),
    ];
    env.extend_from_slice(extra);
    env
}

/// Kills a run at each step of a resume point, after `after` earlier points,
/// then resumes it and checks the result. `packed` gives the huge entry's
/// size in the archive.
fn crash_inside_the_entry(
    build: impl Fn(&Path) -> PathBuf,
    files: &Files,
    packed: impl Fn(&Path) -> u64,
) {
    for point in [
        "stream-after-sync",
        "stream-after-journal",
        "stream-after-punch",
    ] {
        for after in [0u32, 3, 9] {
            let tmp = TempDir::new().unwrap();
            let archive = build(tmp.path());
            let packed = packed(&archive);
            let out = tmp.path().join("out");
            let after_s = after.to_string();

            let crashed = extract(
                &archive,
                &out,
                &stream_env(&[
                    ("HATCHZIP_CRASH_AT", point),
                    ("HATCHZIP_CRASH_AFTER", &after_s),
                ]),
            );
            assert!(
                !crashed.status.success(),
                "{point}#{after} should have crashed"
            );
            // Entries above the huge one were committed and truncated away;
            // the huge one keeps its full extent while it streams.
            let len = fs::metadata(&archive).unwrap().len();
            assert!(
                len > packed,
                "{point}#{after}: the archive was cut to {len} mid-entry"
            );
            // A point that was journaled and then punched has given back
            // the archive bytes behind it: at least `after + 1` steps of
            // output, less what compression saved, give or take 10%.
            if point == "stream-after-punch"
                && let Some(used) = allocated(&archive)
            {
                let size = files[HUGE].len() as u64;
                let per_step = 2_097_152 * packed / size * 9 / 10;
                let freed = (after as u64 + 1) * per_step - 2 * 65_536;
                assert!(
                    used + freed <= len.next_multiple_of(4096),
                    "{point}#{after}: {used} of {len} bytes still allocated"
                );
            }
            for p in side_files(&archive) {
                assert!(p.exists(), "{point}#{after}: {} is missing", p.display());
            }

            assert_ok(&extract(&archive, &out, &stream_env(&[])));
            assert_tree(&out, files);
            assert_consumed(&archive);
        }
    }
}

#[test]
fn a_stored_zip_entry_frees_space_as_it_goes_and_survives_crashes() {
    let files = files();
    crash_inside_the_entry(
        |d| build_zip(d, &files, CompressionMethod::Stored),
        &files,
        packed_size,
    );
}

#[test]
fn a_tar_entry_frees_space_as_it_goes_and_survives_crashes() {
    let files = files();
    let size = files[HUGE].len() as u64;
    crash_inside_the_entry(|d| build_tar(d, &files), &files, |_| size);
}

#[test]
fn a_deflated_zip_entry_frees_space_as_it_goes_and_survives_crashes() {
    let files = deflate_files();
    crash_inside_the_entry(
        |d| build_zip(d, &files, CompressionMethod::Deflated),
        &files,
        packed_size,
    );
}

/// Damage inside a streamed entry stops the run, and since the bytes before
/// the last point are gone, the half-written file and its point stay for a
/// later run rather than being thrown away.
#[test]
fn damage_inside_a_streamed_entry_keeps_its_progress() {
    let files = deflate_files();
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &files, CompressionMethod::Deflated);
    let mut bytes = fs::read(&archive).unwrap();
    // Late in the entry's data, past several resume points.
    let at = bytes.len() / 2 + bytes.len() / 4;
    for b in &mut bytes[at..at + 64] {
        *b ^= 0x5A;
    }
    fs::write(&archive, bytes).unwrap();
    let out = tmp.path().join("out");

    let run = extract(&archive, &out, &stream_env(&[]));
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("huge.bin"), "{stderr}");
    assert!(out.join(format!("{HUGE}.hatchzip-part")).exists());
    for p in side_files(&archive) {
        assert!(p.exists());
    }
    // Trying again fails the same way, and still loses nothing.
    assert!(!extract(&archive, &out, &stream_env(&[])).status.success());
    assert!(out.join(format!("{HUGE}.hatchzip-part")).exists());
}

/// The commit at the end of a streamed entry goes through the same two crash
/// points as every batch.
#[test]
fn crashes_around_the_final_commit_of_a_streamed_entry() {
    let files = files();
    for point in ["after-extract", "after-journal"] {
        for after in 0..6 {
            let tmp = TempDir::new().unwrap();
            let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
            let out = tmp.path().join("out");
            let after = after.to_string();
            let crashed = extract(
                &archive,
                &out,
                &stream_env(&[
                    ("HATCHZIP_CRASH_AT", point),
                    ("HATCHZIP_CRASH_AFTER", &after),
                ]),
            );
            assert!(!crashed.status.success(), "{point}#{after}");
            assert_ok(&extract(&archive, &out, &stream_env(&[])));
            assert_tree(&out, &files);
            assert_consumed(&archive);
        }
    }
}

#[test]
fn crashing_again_and_again_inside_one_entry_is_harmless() {
    let files = files();
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
    let out = tmp.path().join("out");
    for point in [
        "stream-after-punch",
        "stream-after-sync",
        "stream-after-journal",
        "stream-after-punch",
    ] {
        let crashed = extract(
            &archive,
            &out,
            &stream_env(&[("HATCHZIP_CRASH_AT", point), ("HATCHZIP_CRASH_AFTER", "1")]),
        );
        assert!(!crashed.status.success(), "{point} should have crashed");
    }
    // The last run goes on without the knobs: a half-streamed entry keeps
    // streaming, at the default step.
    assert_ok(&extract(&archive, &out, &[]));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

/// What `check` says HatchZip needs, in bytes, give or take its rounding.
fn needed(archive: &Path, env: &[(&str, &str)]) -> f64 {
    let a = archive.to_str().unwrap();
    let run = hatchzip(&["check", a, "--keep-free", "0"], env);
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let line = stdout
        .lines()
        .find(|l| l.contains("HatchZip needs"))
        .unwrap_or_else(|| panic!("no plan in:\n{stdout}"));
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "needs").unwrap();
    let value: f64 = words[at + 1]
        .trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
        .parse()
        .unwrap();
    let unit = words[at + 2];
    let scale = match unit.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        other => panic!("unit {other} in {line}"),
    };
    value * scale
}

#[test]
fn the_space_check_counts_freeing_inside_the_entry_only_where_it_works() {
    let files = files();
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
    let huge = files[HUGE].len() as f64;

    let streaming = needed(&archive, &[("HATCHZIP_STREAM_STEP", STEP)]);
    let whole = needed(
        &archive,
        &[("HATCHZIP_STREAM_STEP", STEP), ("HATCHZIP_NO_PUNCH", "1")],
    );
    assert!(whole >= huge, "without punching it needs {whole}");
    assert!(
        streaming < huge / 2.0,
        "with punching it should need a few steps, not {streaming}"
    );
}

#[test]
fn without_hole_punching_large_entries_are_extracted_whole() {
    let files = files();
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
    let out = tmp.path().join("out");
    // The crash point is never reached, because nothing streams.
    let run = extract(
        &archive,
        &out,
        &stream_env(&[
            ("HATCHZIP_NO_PUNCH", "1"),
            ("HATCHZIP_CRASH_AT", "stream-after-sync"),
        ]),
    );
    assert_ok(&run);
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

#[test]
fn a_half_streamed_entry_is_finished_even_if_punching_stops_working() {
    let files = files();
    let tmp = TempDir::new().unwrap();
    let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
    let out = tmp.path().join("out");
    let crash = stream_env(&[
        ("HATCHZIP_CRASH_AT", "stream-after-punch"),
        ("HATCHZIP_CRASH_AFTER", "2"),
    ]);
    assert!(!extract(&archive, &out, &crash).status.success());
    assert_ok(&extract(
        &archive,
        &out,
        &stream_env(&[("HATCHZIP_NO_PUNCH", "1")]),
    ));
    assert_tree(&out, &files);
    assert_consumed(&archive);
}

/// An entry with holes punched in it can't go back into an archive, so
/// restore refuses before changing anything, and extraction still finishes.
/// Before the first punch there is nothing to refuse.
#[test]
fn restore_refuses_an_archive_with_a_partly_freed_entry() {
    let files = files();
    for (point, after, refused) in [
        ("stream-after-punch", "1", true),
        ("stream-after-journal", "0", true),
        ("stream-after-sync", "0", false),
    ] {
        let tmp = TempDir::new().unwrap();
        let archive = build_zip(tmp.path(), &files, CompressionMethod::Stored);
        let out = tmp.path().join("out");
        let crash = stream_env(&[
            ("HATCHZIP_CRASH_AT", point),
            ("HATCHZIP_CRASH_AFTER", after),
        ]);
        assert!(!extract(&archive, &out, &crash).status.success());

        let before = fs::read(&archive).unwrap();
        let journal = fs::read(&side_files(&archive)[0]).unwrap();
        let run = hatchzip(&["restore", archive.to_str().unwrap()], &[]);
        if !refused {
            assert_ok(&run);
            continue;
        }
        assert!(!run.status.success(), "{point}: restore should refuse");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(stderr.contains("can't be restored"), "{point}: {stderr}");
        assert_eq!(
            fs::read(&archive).unwrap(),
            before,
            "{point}: archive changed"
        );
        assert_eq!(fs::read(&side_files(&archive)[0]).unwrap(), journal);

        assert_ok(&extract(&archive, &out, &stream_env(&[])));
        assert_tree(&out, &files);
        assert_consumed(&archive);
    }
}
