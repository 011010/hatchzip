//! Decoders that can stop, have their position recorded in the journal, and
//! start again from that position in a later process. That is what lets the
//! engine free the archive bytes behind a large file while it is still being
//! extracted: once a position is durable, nothing before it is ever read
//! again.

use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail};

use super::storage::Reader;
use super::{Codec, Stream};

/// How much output from just before a resume point a decoder needs to start
/// there.
pub const WINDOW: usize = 0;

pub struct Decoder {
    inner: Inner,
}

enum Inner {
    Stored(Stored),
}

impl Decoder {
    /// Starts decoding `stream` at `bit`, counted in bits from the start of
    /// the archive: the start of the data, or a point an earlier run
    /// recorded. `window` is the output just before that point, up to
    /// [`WINDOW`] bytes.
    pub fn new(reader: Reader, stream: &Stream, bit: u64, _window: &[u8]) -> Result<Self> {
        let end = stream.data_start + stream.packed;
        let inner = match stream.codec {
            Codec::Stored => {
                if !bit.is_multiple_of(8) || bit / 8 < stream.data_start || bit / 8 > end {
                    bail!("the journal's resume point lies outside the file's data");
                }
                Inner::Stored(Stored {
                    reader,
                    pos: bit / 8,
                    end,
                })
            }
            Codec::Deflate => bail!("deflate streams can't be resumed yet"),
        };
        Ok(Self { inner })
    }

    /// Decodes into `buf`. Returns 0 only at the end of the data.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match &mut self.inner {
            Inner::Stored(s) => s.read(buf),
        }
    }

    /// Where decoding could restart, in bits from the start of the archive,
    /// if it can be stopped right after the output handed out so far.
    pub fn restart_point(&self) -> Option<u64> {
        match &self.inner {
            Inner::Stored(s) => Some(s.pos * 8),
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
