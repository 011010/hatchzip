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
//!
//! [`Gzip`] wraps the same raw inflate in gzip members: it parses their
//! headers, checks each trailer's CRC-32 and length, and can also restart at
//! the start of any member, where nothing from before is needed.

use std::ffi::CStr;
use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, anyhow, bail};
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
            Codec::Deflate => Inner::Deflate(Inflate::new(reader, bit, end, window, INPUT_BUF)?),
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
    /// `input` is the size of the input buffer: [`INPUT_BUF`], except in
    /// tests that make every boundary fall across a refill.
    fn new(mut reader: Reader, bit: u64, end: u64, window: &[u8], input: usize) -> Result<Self> {
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
            input: vec![0; input],
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

    /// Fills the input buffer if zlib has taken all of it.
    fn refill(&mut self) -> Result<()> {
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
        Ok(())
    }

    /// The archive offset of the next byte zlib has not taken.
    fn pos(&self) -> u64 {
        self.fed_from + self.taken
    }

    /// Takes one byte of input past zlib, for the gzip framing between
    /// deflate streams. `None` at the end of the archive.
    fn raw_byte(&mut self) -> Result<Option<u8>> {
        self.refill()?;
        if self.strm.avail_in == 0 {
            return Ok(None);
        }
        // SAFETY: `avail_in` bytes from `next_in` lie inside `self.input`.
        let b = unsafe { *self.strm.next_in };
        self.strm.next_in = unsafe { self.strm.next_in.add(1) };
        self.strm.avail_in -= 1;
        self.taken += 1;
        Ok(Some(b))
    }

    /// After the end of a deflate stream: hands back the whole bytes zlib
    /// pulled into its bit buffer without using them, so that [`pos`] is the
    /// first byte after the deflate data.
    ///
    /// [`pos`]: Self::pos
    fn give_back_unused(&mut self) {
        // zlib skips to a byte boundary after the last block, so what it
        // holds is whole bytes. zlib-rs was seen to hold none, at every
        // member end of the unit tests with buffers down to one byte; zlib
        // may, so this stays.
        let unused = (self.data_type & 63) as u64 / 8;
        // SAFETY: `next_in` points into `self.input`.
        let in_buffer = unsafe { self.strm.next_in.offset_from(self.input.as_ptr()) } as u64;
        if unused <= in_buffer {
            self.strm.next_in = unsafe { self.strm.next_in.sub(unused as usize) };
            self.strm.avail_in += unused as u32;
            self.taken -= unused;
        } else {
            // They came from an earlier fill of the buffer: read them again.
            let at = self.pos() - unused;
            self.fed_from = at;
            self.taken = 0;
            self.next_read = at;
            self.strm.avail_in = 0;
        }
    }

    /// Starts a new raw deflate stream at [`pos`](Self::pos), keeping the
    /// input already read.
    fn restart(&mut self) -> Result<()> {
        let ret = unsafe { z::inflateReset(&mut *self.strm) };
        if ret != z::Z_OK {
            bail!("restarting the deflate decoder failed ({ret})");
        }
        self.done = false;
        self.data_type = 0;
        Ok(())
    }

    /// The output just before the current position that the rest of the
    /// stream may refer back to, up to [`WINDOW`] bytes.
    fn window(&self) -> Result<Vec<u8>> {
        let mut window = vec![0; WINDOW];
        let mut len = 0u32;
        // SAFETY: the buffer holds WINDOW bytes, the most zlib ever copies.
        let ret = unsafe { z::inflateGetDictionary(&*self.strm, window.as_mut_ptr(), &mut len) };
        if ret != z::Z_OK || len as usize > WINDOW {
            bail!("reading the deflate window failed ({ret})");
        }
        window.truncate(len as usize);
        Ok(window)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let buf = &mut buf[..len];
        loop {
            self.refill()?;
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
                if self.data_type & 192 == 192 {
                    // The last block just ended. zlib reports the end of the
                    // stream on the next call, which outputs nothing; taking
                    // it now lets a gzip member end on this read, so the
                    // start of the next one is a point to resume from.
                    self.finish()?;
                }
                return Ok(produced);
            }
        }
    }

    fn finish(&mut self) -> Result<()> {
        let mut scratch = [0u8; 1];
        let avail_in = self.strm.avail_in;
        self.strm.next_out = scratch.as_mut_ptr();
        self.strm.avail_out = 1;
        let ret = unsafe { z::inflate(&mut *self.strm, z::Z_BLOCK) };
        self.taken += u64::from(avail_in - self.strm.avail_in);
        self.data_type = self.strm.data_type;
        match ret {
            z::Z_STREAM_END if self.strm.avail_out == 1 => {
                self.done = true;
                Ok(())
            }
            _ => bail!("its compressed data is damaged ({})", self.message()),
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

/// Where a gzip stream can be decoded from again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GzPoint {
    /// In bits from the start of the archive.
    pub bit: u64,
    /// `bit` is where a member's header begins, so nothing from before is
    /// needed. Otherwise it is a deflate block boundary inside a member.
    pub member_start: bool,
    /// CRC-32 and length of the member's output before the point, which its
    /// trailer is checked against.
    pub crc: u32,
    pub len: u64,
    /// The member's output just before the point, up to [`WINDOW`] bytes.
    pub window: Vec<u8>,
}

impl GzPoint {
    pub fn start() -> Self {
        Self {
            bit: 0,
            member_start: true,
            crc: 0,
            len: 0,
            window: Vec::new(),
        }
    }
}

/// A gzip stream of one or more members (RFC 1952), decoded from the front
/// or from a [`GzPoint`]. Each member's CRC-32 and length are checked against
/// its trailer as it ends; `read` returns 0 only once every member is
/// verified.
pub struct Gzip {
    z: Inflate,
    /// Inside a member's deflate data, rather than before a header.
    in_member: bool,
    crc: crc32fast::Hasher,
    len: u64,
    done: bool,
}

impl Gzip {
    /// Decodes the gzip stream that fills the archive up to `end`, from `at`.
    pub fn new(reader: Reader, end: u64, at: &GzPoint) -> Result<Self> {
        Self::with_input(reader, end, at, INPUT_BUF)
    }

    fn with_input(reader: Reader, end: u64, at: &GzPoint, input: usize) -> Result<Self> {
        if at.bit / 8 > end || (at.member_start && !at.bit.is_multiple_of(8)) {
            bail!("the journal's resume point lies outside the compressed data");
        }
        let window: &[u8] = if at.member_start { &[] } else { &at.window };
        Ok(Self {
            z: Inflate::new(reader, at.bit, end, window, input)?,
            in_member: !at.member_start,
            crc: crc32fast::Hasher::new_with_initial(at.crc),
            len: at.len,
            done: false,
        })
    }

    /// Decodes into `buf`. Returns 0 only at the end of the stream.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.done {
                return Ok(0);
            }
            if !self.in_member {
                if !self.header()? {
                    self.done = true;
                    return Ok(0);
                }
                self.in_member = true;
                self.crc = crc32fast::Hasher::new();
                self.len = 0;
            }
            let n = self.z.read(buf)?;
            self.crc.update(&buf[..n]);
            self.len += n as u64;
            if self.z.done {
                self.z.give_back_unused();
                self.trailer()?;
                self.z.restart()?;
                self.in_member = false;
            }
            if n > 0 {
                return Ok(n);
            }
        }
    }

    /// Where decoding could restart, in bits, if it is stopped right after
    /// the output handed out so far, and whether that is a member's start.
    /// Cheap; [`restart_point`](Self::restart_point) adds the window.
    pub fn restart_bit(&self) -> Option<(u64, bool)> {
        if self.done {
            None
        } else if !self.in_member {
            Some((self.z.pos() * 8, true))
        } else {
            self.z.restart_point().map(|bit| (bit, false))
        }
    }

    pub fn restart_point(&self) -> Result<Option<GzPoint>> {
        let Some((bit, member_start)) = self.restart_bit() else {
            return Ok(None);
        };
        if member_start {
            return Ok(Some(GzPoint {
                bit,
                ..GzPoint::start()
            }));
        }
        Ok(Some(GzPoint {
            bit,
            member_start,
            crc: self.crc.clone().finalize(),
            len: self.len,
            window: self.z.window()?,
        }))
    }

    fn byte(&mut self) -> Result<u8> {
        self.z
            .raw_byte()?
            .ok_or_else(|| anyhow!("the compressed data ends early"))
    }

    /// Parses a member header. Returns false at the end of the stream: the
    /// end of the archive, or zeros up to it, which some tools pad with.
    fn header(&mut self) -> Result<bool> {
        let at = self.z.pos();
        let Some(first) = self.z.raw_byte()? else {
            return Ok(false);
        };
        if first == 0 {
            while let Some(b) = self.z.raw_byte()? {
                if b != 0 {
                    bail!("unexpected data follows the compressed data at byte {at}");
                }
            }
            return Ok(false);
        }
        let unexpected = || anyhow!("unexpected data follows the compressed data at byte {at}");
        if first != 0x1F {
            return Err(unexpected());
        }
        let mut head = vec![first];
        // The second magic byte, then the method, which is always deflate.
        for want in [0x8B, 8] {
            match self.z.raw_byte()? {
                Some(b) if b == want => head.push(b),
                _ => return Err(unexpected()),
            }
        }
        for _ in 3..10 {
            head.push(self.byte()?);
        }
        let flags = head[3];
        if flags & 0xE0 != 0 {
            bail!("the gzip member at byte {at} uses header flags that are not defined");
        }
        if flags & 4 != 0 {
            let (a, b) = (self.byte()?, self.byte()?);
            head.extend([a, b]);
            for _ in 0..u16::from_le_bytes([a, b]) {
                let b = self.byte()?;
                head.push(b);
            }
        }
        // The file name, then the comment, each ending in a zero byte.
        for flag in [8, 16] {
            if flags & flag != 0 {
                loop {
                    let b = self.byte()?;
                    head.push(b);
                    if b == 0 {
                        break;
                    }
                }
            }
        }
        if flags & 2 != 0 {
            let check = u16::from_le_bytes([self.byte()?, self.byte()?]);
            if check != crc32fast::hash(&head) as u16 {
                bail!("the gzip member at byte {at} has a damaged header (CRC mismatch)");
            }
        }
        Ok(true)
    }

    fn trailer(&mut self) -> Result<()> {
        let mut t = [0u8; 8];
        for b in &mut t {
            *b = self.byte()?;
        }
        if u32::from_le_bytes([t[0], t[1], t[2], t[3]]) != self.crc.clone().finalize() {
            bail!("the decompressed data is damaged (CRC mismatch)");
        }
        // ISIZE is the length modulo 2^32.
        if u32::from_le_bytes([t[4], t[5], t[6], t[7]]) != self.len as u32 {
            bail!("the decompressed data is damaged (length mismatch)");
        }
        Ok(())
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

    /// One gzip member, with the optional header fields `flags` asks for.
    fn gzip_member(data: &[u8], level: i32, flags: u8) -> Vec<u8> {
        let mut head = vec![0x1F, 0x8B, 8, flags, 1, 2, 3, 4, 0, 3];
        if flags & 4 != 0 {
            head.extend(5u16.to_le_bytes());
            head.extend(b"extra");
        }
        if flags & 8 != 0 {
            head.extend(b"name.tar\0");
        }
        if flags & 16 != 0 {
            head.extend(b"a comment\0");
        }
        if flags & 2 != 0 {
            let check = crc32fast::hash(&head) as u16;
            head.extend(check.to_le_bytes());
        }
        head.extend(deflate_raw(data, level));
        head.extend(crc32fast::hash(data).to_le_bytes());
        head.extend((data.len() as u32).to_le_bytes());
        head
    }

    fn gzip_decode_all(g: &mut Gzip) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = vec![0; 64 * 1024];
        loop {
            let n = g.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn open_gzip(path: &std::path::Path, at: &GzPoint) -> Gzip {
        let storage = Storage::open(path, false).unwrap();
        let len = storage.original_len();
        Gzip::new(storage.reader(None), len, at).unwrap()
    }

    /// A multi-member stream, with every optional header field, an empty
    /// member and zero padding at the end: decoded whole, then resumed from
    /// every point it reports, member starts included, with the CRC-32 and
    /// length checks carried across the resume.
    #[test]
    fn gzip_resumes_from_every_point_across_members() {
        let dir = std::env::temp_dir().join(format!("hatchzip-gzip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = text(3_000_000);
        let mut file = gzip_member(&data[..1_000_000], 6, 8);
        file.extend(gzip_member(&[], 6, 0));
        file.extend(gzip_member(&data[1_000_000..1_700_000], 9, 2 | 4 | 16));
        file.extend(gzip_member(&data[1_700_000..1_700_100], 6, 0));
        file.extend(gzip_member(&data[1_700_100..], 6, 0));
        file.extend([0u8; 1000]);
        let path = dir.join("multi.gz");
        std::fs::write(&path, &file).unwrap();

        let mut g = open_gzip(&path, &GzPoint::start());
        let mut points = Vec::new();
        let mut written = 0;
        let mut buf = vec![0; 100_000];
        loop {
            let n = g.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            assert_eq!(buf[..n], data[written..written + n]);
            written += n;
            if let Some(p) = g.restart_point().unwrap() {
                points.push((p, written));
            }
        }
        assert_eq!(written, data.len());
        let starts = points.iter().filter(|(p, _)| p.member_start).count();
        assert!(starts >= 3, "only {starts} member starts");
        assert!(points.len() - starts >= 5, "only {} points", points.len());
        assert!(points.iter().any(|(p, _)| p.bit % 8 != 0));

        for (p, at) in &points {
            let mut g = open_gzip(&path, p);
            let rest = gzip_decode_all(&mut g).unwrap();
            assert!(
                rest == data[*at..],
                "resuming at {p:?} (output {at}) went wrong"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gzip_checks_every_trailer_and_what_follows() {
        let dir = std::env::temp_dir().join(format!("hatchzip-gzbad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = text(200_000);
        let one = gzip_member(&data, 6, 0);
        let path = dir.join("bad.gz");
        let fails = |bytes: &[u8]| -> String {
            std::fs::write(&path, bytes).unwrap();
            let mut g = open_gzip(&path, &GzPoint::start());
            match gzip_decode_all(&mut g) {
                Ok(_) => String::new(),
                Err(e) => format!("{e:#}"),
            }
        };

        let mut two = one.clone();
        two.extend(&one);
        assert_eq!(fails(&two), "");

        let n = one.len();
        let mut bad_crc = one.clone();
        bad_crc[n - 8] ^= 1;
        assert!(fails(&bad_crc).contains("CRC mismatch"));
        let mut bad_len = one.clone();
        bad_len[n - 1] ^= 1;
        assert!(fails(&bad_len).contains("length mismatch"));
        // Damage in a later member is found too.
        let mut late = one.clone();
        late.extend(&bad_crc);
        assert!(fails(&late).contains("CRC mismatch"));
        assert!(fails(&one[..n - 3]).contains("ends early"));
        let mut garbage = one.clone();
        garbage.extend(b"\0\0junk");
        assert!(fails(&garbage).contains("unexpected data"));
        let mut hcrc = gzip_member(&data, 6, 2);
        hcrc[10] ^= 1;
        assert!(fails(&hcrc).contains("damaged header"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With a tiny input buffer every header, trailer and stream end falls
    /// across a refill, including the bytes zlib pulled in past the end of
    /// a deflate stream, which then come from an earlier fill.
    #[test]
    fn gzip_framing_survives_any_input_buffer_size() {
        let dir = std::env::temp_dir().join(format!("hatchzip-gzsmall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = text(120_000);
        let mut file = Vec::new();
        let mut from = 0;
        for (i, to) in [1, 5_000, 5_000, 40_000, 40_007, 120_000]
            .into_iter()
            .enumerate()
        {
            file.extend(gzip_member(
                &data[from..to],
                [1, 6, 9][i % 3],
                [0, 8, 30][i % 3],
            ));
            from = to;
        }
        let path = dir.join("small.gz");
        std::fs::write(&path, &file).unwrap();
        let storage = Storage::open(&path, false).unwrap();
        for input in [1, 2, 3, 7, 64, 1000, 70_000] {
            for output in [1, 5, 4096] {
                let mut g = Gzip::with_input(
                    storage.reader(None),
                    file.len() as u64,
                    &GzPoint::start(),
                    input,
                )
                .unwrap();
                let mut got = Vec::new();
                let mut buf = vec![0; output];
                loop {
                    let n = g.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                assert!(got == data, "buffers {input}/{output}: wrong output");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
