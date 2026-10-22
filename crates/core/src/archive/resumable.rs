//! Decoders that can stop, have their position recorded in the journal, and
//! start again from that position in a later process. That is what lets the
//! engine free the archive bytes behind a large file while it is still being
//! extracted: once a position is durable, nothing before it is ever read
//! again.
//!
//! Stored data can restart at any byte. Deflate can restart at any block
//! boundary, the technique of zlib's `examples/zran.c`: a block boundary may
//! fall inside a byte, so the position is kept in bits, and the decoder
//! needs the 32 KiB of output before it, which back-references reach into.

use std::ffi::CStr;
use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail};
use libz_rs_sys as z;

use super::storage::Reader;
use super::{Codec, Stream};

/// How much output from just before a resume point a decoder needs to start
/// there: the deflate window.
pub const WINDOW: usize = 32 * 1024;

const INPUT_BUF: usize = 1024 * 1024;

pub struct Decoder {
    inner: Inner,
}

enum Inner {
    Stored(Stored),
    Deflate(Inflate),
}

impl Decoder {
    /// Starts decoding `stream` at `bit`, counted in bits from the start of
    /// the archive: the start of the data, or a point an earlier run
    /// recorded. `window` is the output just before that point, up to
    /// [`WINDOW`] bytes.
    pub fn new(reader: Reader, stream: &Stream, bit: u64, window: &[u8]) -> Result<Self> {
        let end = stream.data_start + stream.packed;
        if bit / 8 < stream.data_start || bit / 8 > end {
            bail!("the journal's resume point lies outside the file's data");
        }
        let inner = match stream.codec {
            Codec::Stored => {
                if !bit.is_multiple_of(8) {
                    bail!("the journal's resume point lies inside a byte of stored data");
                }
                Inner::Stored(Stored {
                    reader,
                    pos: bit / 8,
                    end,
                })
            }
            Codec::Deflate => Inner::Deflate(Inflate::new(reader, bit, end, window)?),
        };
        Ok(Self { inner })
    }

    /// Decodes into `buf`. Returns 0 only at the end of the data.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match &mut self.inner {
            Inner::Stored(s) => s.read(buf),
            Inner::Deflate(d) => d.read(buf),
        }
    }

    /// Where decoding could restart, in bits from the start of the archive,
    /// if it can be stopped right after the output handed out so far.
    pub fn restart_point(&self) -> Option<u64> {
        match &self.inner {
            Inner::Stored(s) => Some(s.pos * 8),
            Inner::Deflate(d) => d.restart_point(),
        }
    }
}

struct Stored {
    reader: Reader,
    pos: u64,
    end: u64,
}

impl Stored {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = (self.end - self.pos).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.reader.seek(SeekFrom::Start(self.pos))?;
        self.reader
            .read_exact(&mut buf[..n])
            .context("reading the archive")?;
        self.pos += n as u64;
        Ok(n)
    }
}

/// A raw inflate over the archive that reports block boundaries.
struct Inflate {
    /// Boxed because zlib's state keeps a pointer back to it.
    strm: Box<z::z_stream>,
    reader: Reader,
    input: Vec<u8>,
    /// The next archive offset to read into `input`.
    next_read: u64,
    end: u64,
    /// Archive offset of the first byte handed to zlib, and how many bytes
    /// it has taken since. Counted here because zlib's own `total_in` is 32
    /// bits on Windows.
    fed_from: u64,
    taken: u64,
    /// `data_type` after the last call, which says whether zlib stopped
    /// right after a block and how many bits it holds unread.
    data_type: i32,
    done: bool,
}

impl Inflate {
    fn new(mut reader: Reader, bit: u64, end: u64, window: &[u8]) -> Result<Self> {
        let byte = bit / 8;
        let skip = (bit % 8) as u32;
        // A point inside a byte: the bits below `skip` belong to the block
        // before, the rest are fed in by hand.
        let partial = if skip == 0 {
            None
        } else {
            let mut b = [0u8];
            reader.seek(SeekFrom::Start(byte))?;
            reader.read_exact(&mut b).context("reading the archive")?;
            Some(b[0] >> skip)
        };
        let start = if partial.is_some() { byte + 1 } else { byte };
        let mut strm = Box::new(z::z_stream::default());
        // Negative window bits: raw deflate, no zlib or gzip wrapper.
        let ret = unsafe {
            z::inflateInit2_(
                &mut *strm,
                -15,
                z::zlibVersion(),
                size_of::<z::z_stream>() as i32,
            )
        };
        if ret != z::Z_OK {
            bail!("starting the deflate decoder failed ({ret})");
        }
        // Constructed right away so that every error below still ends the
        // decoder.
        let mut me = Self {
            strm,
            reader,
            input: vec![0; INPUT_BUF],
            next_read: start,
            end,
            fed_from: start,
            taken: 0,
            data_type: 0,
            done: false,
        };
        if let Some(bits) = partial {
            let ret = unsafe { z::inflatePrime(&mut *me.strm, (8 - skip) as i32, i32::from(bits)) };
            if ret != z::Z_OK {
                bail!("resuming the deflate decoder failed ({ret})");
            }
        }
        if !window.is_empty() {
            let ret = unsafe {
                z::inflateSetDictionary(&mut *me.strm, window.as_ptr(), window.len() as u32)
            };
            if ret != z::Z_OK {
                bail!("resuming the deflate decoder failed ({ret})");
            }
        }
        Ok(me)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let buf = &mut buf[..len];
        loop {
            if self.strm.avail_in == 0 && self.next_read < self.end {
                let n = (self.end - self.next_read).min(self.input.len() as u64) as usize;
                self.reader.seek(SeekFrom::Start(self.next_read))?;
                self.reader
                    .read_exact(&mut self.input[..n])
                    .context("reading the archive")?;
                self.next_read += n as u64;
                self.strm.next_in = self.input.as_ptr();
                self.strm.avail_in = n as u32;
            }
            let avail_in = self.strm.avail_in;
            self.strm.next_out = buf.as_mut_ptr();
            self.strm.avail_out = buf.len() as u32;
            // Z_BLOCK makes inflate return at every block boundary, the only
            // places a deflate stream can be resumed from.
            let ret = unsafe { z::inflate(&mut *self.strm, z::Z_BLOCK) };
            self.taken += u64::from(avail_in - self.strm.avail_in);
            let produced = buf.len() - self.strm.avail_out as usize;
            self.data_type = self.strm.data_type;
            match ret {
                z::Z_STREAM_END => {
                    self.done = true;
                    return Ok(produced);
                }
                z::Z_OK => {}
                z::Z_BUF_ERROR if self.strm.avail_in == 0 && self.next_read >= self.end => {
                    bail!("its compressed data ends early");
                }
                z::Z_BUF_ERROR => {}
                _ => bail!("its compressed data is damaged ({})", self.message()),
            }
            if produced > 0 {
                return Ok(produced);
            }
        }
    }

    fn restart_point(&self) -> Option<u64> {
        // 128: stopped right after a block ended. 64: that was the last one,
        // so there is nothing to restart into.
        if self.done || self.data_type & 128 == 0 || self.data_type & 64 != 0 {
            return None;
        }
        // The low bits count bits zlib took from the input but hasn't used:
        // the tail end of the bytes it took last.
        let held = (self.data_type & 63) as u64;
        Some((self.fed_from + self.taken) * 8 - held)
    }

    fn message(&self) -> String {
        if self.strm.msg.is_null() {
            return "unknown error".into();
        }
        unsafe { CStr::from_ptr(self.strm.msg) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Inflate {
    fn drop(&mut self) {
        unsafe {
            z::inflateEnd(&mut *self.strm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::storage::Storage;

    /// Text-like data: matches reach back across block boundaries, so a
    /// wrong window or bit position can't go unnoticed.
    fn text(len: usize) -> Vec<u8> {
        let words: Vec<&[u8]> = b"the archive frees its bytes behind the decoder while every \
            resume point stays durable and nothing is lost 0123456789 zran inflate prime"
            .split(|b| *b == b' ')
            .collect();
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut out = Vec::with_capacity(len + 16);
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.extend_from_slice(words[(x % words.len() as u64) as usize]);
            out.push(if x & 0x700 == 0 { b'\n' } else { b' ' });
            if x & 0xF000 == 0 {
                // Bursts of noise force stored blocks in between.
                out.extend((0..300).map(|i| (x >> (i % 56)) as u8));
            }
        }
        out.truncate(len);
        out
    }

    fn deflate_raw(data: &[u8], level: i32) -> Vec<u8> {
        let mut strm = z::z_stream::default();
        unsafe {
            let ret = z::deflateInit2_(
                &mut strm,
                level,
                z::Z_DEFLATED,
                -15,
                8,
                z::Z_DEFAULT_STRATEGY,
                z::zlibVersion(),
                size_of::<z::z_stream>() as i32,
            );
            assert_eq!(ret, z::Z_OK);
            let mut out = vec![0u8; data.len() + data.len() / 100 + 1024];
            strm.next_in = data.as_ptr();
            strm.avail_in = data.len() as u32;
            strm.next_out = out.as_mut_ptr();
            strm.avail_out = out.len() as u32;
            assert_eq!(z::deflate(&mut strm, z::Z_FINISH), z::Z_STREAM_END);
            out.truncate(out.len() - strm.avail_out as usize);
            z::deflateEnd(&mut strm);
            out
        }
    }

    fn decode_all(d: &mut Decoder) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = vec![0; 64 * 1024];
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                return out;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    /// Records every restart point of a deflate stream, then starts a fresh
    /// decoder at each one, as a resumed run does, and checks it produces
    /// exactly the rest of the data.
    #[test]
    fn deflate_resumes_from_every_block_boundary() {
        let dir = std::env::temp_dir().join(format!("hatchzip-inflate-{}", std::process::id()));
        fs_err(std::fs::create_dir_all(&dir));
        let data = text(6_000_000);
        for level in [6, 9, 1] {
            let packed = deflate_raw(&data, level);
            // Something in front, so offsets are absolute as in a real archive.
            let prefix = 777u64;
            let path = dir.join(format!("stream-{level}"));
            let mut file = vec![0xEE; prefix as usize];
            file.extend_from_slice(&packed);
            file.extend_from_slice(b"trailing bytes that are not deflate");
            std::fs::write(&path, &file).unwrap();
            let storage = Storage::open(&path, false).unwrap();
            let stream = Stream {
                data_start: prefix,
                packed: packed.len() as u64,
                codec: Codec::Deflate,
                crc: None,
            };

            let mut d = Decoder::new(storage.reader(None), &stream, prefix * 8, &[]).unwrap();
            let mut points = Vec::new();
            let mut written = 0;
            let mut buf = vec![0; 100_000];
            loop {
                let n = d.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                assert_eq!(buf[..n], data[written..written + n]);
                written += n;
                if let Some(bit) = d.restart_point() {
                    points.push((bit, written));
                }
            }
            assert_eq!(written, data.len());
            if level == 1 {
                // zlib-rs, like zlib-ng, writes level 1 as one single block,
                // which has no point to resume from but its start.
                assert!(points.is_empty(), "level 1 grew {} points", points.len());
                continue;
            }
            assert!(
                points.len() >= 10,
                "level {level}: only {} points",
                points.len()
            );
            assert!(
                points.iter().any(|(bit, _)| bit % 8 != 0),
                "level {level}: no point inside a byte"
            );

            for (bit, at) in points.iter().copied() {
                let window = &data[at.saturating_sub(WINDOW)..at];
                let mut d = Decoder::new(storage.reader(None), &stream, bit, window).unwrap();
                assert!(
                    decode_all(&mut d) == data[at..],
                    "level {level}: resuming at bit {bit} (output {at}) went wrong"
                );
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn fs_err(r: std::io::Result<()>) {
        r.unwrap()
    }

    #[test]
    fn truncated_and_damaged_deflate_is_an_error() {
        let dir = std::env::temp_dir().join(format!("hatchzip-damaged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = text(500_000);
        let packed = deflate_raw(&data, 6);
        let path = dir.join("stream");
        std::fs::write(&path, &packed).unwrap();
        let storage = Storage::open(&path, false).unwrap();
        let short = Stream {
            data_start: 0,
            packed: packed.len() as u64 / 2,
            codec: Codec::Deflate,
            crc: None,
        };
        let mut d = Decoder::new(storage.reader(None), &short, 0, &[]).unwrap();
        let mut buf = vec![0; 1 << 16];
        let err = loop {
            match d.read(&mut buf) {
                Ok(0) => panic!("a cut stream decoded to the end"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(format!("{err:#}").contains("ends early"), "{err:#}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
