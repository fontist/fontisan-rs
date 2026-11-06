//! WOFF and WOFF2 container decode to raw sfnt bytes (card 1e).
//!
//! The WOFF2 output is laid out exactly as the pinned Google `woff2_decompress`
//! lays it out (sorted directory, 4-byte padded tables, re-built `glyf`/`loca`/
//! `hmtx`, recomputed checksums), so the two decoders are byte-comparable.

mod woff1;
mod woff2;

pub(crate) use woff2::{KNOWN_TAGS, brotli_inflate, triplet_decode};

use std::fmt;

pub use woff1::decode_woff;
pub use woff2::{
    decode_woff2, decode_woff2_collection, decode_woff2_ruby_padding, is_woff2_collection,
};

/// Why a WOFF/WOFF2 input was rejected. Never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub(crate) String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "woff: {}", self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn err<T>(msg: &str) -> Result<T> {
    Err(Error(msg.to_string()))
}

/// Upper bound on any decoded size we will materialise (1 GiB).
pub(crate) const MAX_OUTPUT: u64 = 1 << 30;

/// Bounds-checked big-endian cursor.
#[derive(Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }
    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return err("unexpected end of data");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

pub(crate) fn round4(n: u64) -> u64 {
    (n + 3) & !3
}

/// Sum of big-endian u32 words, the tail zero-padded (sfnt table checksum).
pub(crate) fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(4);
    for c in &mut chunks {
        sum = sum.wrapping_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut w = [0u8; 4];
        w[..rest.len()].copy_from_slice(rest);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

/// `searchRange`, `entrySelector`, `rangeShift` for an sfnt offset table.
pub(crate) fn offset_table(flavor: u32, num_tables: u16) -> [u8; 12] {
    let mut max_pow2 = 0u32;
    while (1u32 << (max_pow2 + 1)) <= u32::from(num_tables) {
        max_pow2 += 1;
    }
    let search_range = ((1u32 << max_pow2) << 4) as u16;
    let mut out = [0u8; 12];
    out[0..4].copy_from_slice(&flavor.to_be_bytes());
    out[4..6].copy_from_slice(&num_tables.to_be_bytes());
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&(max_pow2 as u16).to_be_bytes());
    out[10..12]
        .copy_from_slice(&(num_tables.wrapping_shl(4).wrapping_sub(search_range)).to_be_bytes());
    out
}
