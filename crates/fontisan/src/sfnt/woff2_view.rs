//! WOFF2 as the Ruby reference reads it (card 1e-wire).
//!
//! `Fontisan::Woff2Font.from_file` and `Woff2Collection#font` do not lay the
//! font out the way Google's `woff2_decompress` does (which `crate::woff`
//! reproduces byte for byte). Measured against the read goldens, Ruby:
//!
//! - keeps the WOFF2 directory order (the decoder sorts by tag);
//! - re-encodes every glyph itself (`Woff2::GlyfLocaReconstruct`): 2-byte glyph
//!   alignment for a short `loca` (the decoder pads to 4), its own flag run
//!   encoding (the repeat byte holds the run length, as Ruby writes it), and
//!   no `loca`/`glyf` size rounding;
//! - recomputes `head.checksumAdjustment` over its own layout (single fonts);
//! - for a collection, never reconstructs `hmtx`, and builds each member with
//!   `SfntBuilder`, which assigns offsets in member order but writes the
//!   directory and data sorted by tag.
//!
//! This module reproduces those bytes so the 1c model reads them like Ruby's.
//! Where Ruby would raise a non-`InvalidFontError` (a `NoMethodError` on nil
//! from a malformed stream) this raises `InvalidFont`; the goldens carry no such
//! case, so the class of those failures is unverified.
use super::error::{Error, ErrorClass};
use crate::woff::{KNOWN_TAGS, brotli_inflate, triplet_decode};

type R<T> = Result<T, Error>;

const HEADER_SIZE: usize = 36;
const FLAG_ON_CURVE: u8 = 0x01;
const FLAG_X_SHORT: u8 = 0x02;
const FLAG_Y_SHORT: u8 = 0x04;
const FLAG_REPEAT: u8 = 0x08;
const FLAG_X_SAME_OR_POS: u8 = 0x10;
const FLAG_Y_SAME_OR_POS: u8 = 0x20;
const FLAG_OVERLAP_SIMPLE: u8 = 0x40;
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
const MAX_OUTPUT: u64 = 1 << 30;

fn bad(msg: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorClass::InvalidFont,
        format!("Invalid WOFF2 file: {msg}"),
    )
}

fn be16(b: &[u8], at: usize) -> R<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("unexpected end of data"))
}

fn be32(b: &[u8], at: usize) -> R<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| bad("unexpected end of data"))
}

/// Ruby `String#[pos, len]` on bytes: clamped, `None` past the end.
fn slice(b: &[u8], pos: usize, len: usize) -> Option<&[u8]> {
    (pos <= b.len()).then(|| &b[pos..pos.saturating_add(len).min(b.len())])
}

struct Entry {
    tag: [u8; 4],
    orig_length: u32,
    transform_length: Option<u32>,
}

impl Entry {
    fn size(&self) -> u32 {
        self.transform_length.unwrap_or(self.orig_length)
    }
}

/// `UIntBase128` as Ruby reads it (lax: no leading-zero or overflow checks).
fn base128(b: &[u8], pos: &mut usize) -> R<u32> {
    let mut v: u64 = 0;
    for _ in 0..5 {
        let byte = *b
            .get(*pos)
            .ok_or_else(|| bad("Unexpected EOF while reading UIntBase128"))?;
        *pos += 1;
        v = (v << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return u32::try_from(v).map_err(|_| bad("UIntBase128 overflow"));
        }
    }
    Err(bad("Invalid UIntBase128 encoding"))
}

/// `UInt255.decode_at`.
fn u255(b: &[u8], pos: &mut usize) -> R<u32> {
    let code = *b.get(*pos).ok_or_else(|| bad("unexpected end of data"))?;
    *pos += 1;
    Ok(match code {
        253 => {
            let v = be16(b, *pos)?;
            *pos += 2;
            u32::from(v)
        }
        254 | 255 => {
            let v = *b.get(*pos).ok_or_else(|| bad("unexpected end of data"))?;
            *pos += 1;
            u32::from(v) + if code == 254 { 506 } else { 253 }
        }
        c => u32::from(c),
    })
}

fn read_directory(b: &[u8], mut pos: usize, n: usize) -> R<(Vec<Entry>, usize)> {
    let mut out = Vec::with_capacity(n.min(b.len()));
    for _ in 0..n {
        let flags = *b
            .get(pos)
            .ok_or_else(|| bad("Unexpected EOF while reading table directory flags"))?;
        pos += 1;
        let idx = usize::from(flags & 0x3f);
        let tag: [u8; 4] = if idx == 0x3f {
            let t = b
                .get(pos..pos + 4)
                .ok_or_else(|| bad("Unexpected EOF while reading custom tag"))?;
            pos += 4;
            t.try_into().unwrap()
        } else {
            **KNOWN_TAGS
                .get(idx)
                .ok_or_else(|| bad(format!("Invalid table tag index: {idx}")))?
        };
        let orig_length = base128(b, &mut pos)?;
        let version = (flags >> 6) & 3;
        let has = match &tag {
            b"glyf" | b"loca" => version == 0,
            b"hmtx" => version == 1,
            _ => false,
        };
        let transform_length = if has {
            Some(base128(b, &mut pos)?)
        } else {
            None
        };
        out.push(Entry {
            tag,
            orig_length,
            transform_length,
        });
    }
    Ok((out, pos))
}

/// Brotli block split in directory order (`entry.transform_length || orig_length`).
fn inflate_split(b: &[u8], pos: usize, entries: &[Entry]) -> R<Vec<Vec<u8>>> {
    let clen = be32(b, 20)? as usize;
    let comp = slice(b, pos, clen).ok_or_else(|| bad("compressed data missing"))?;
    let total: u64 = entries.iter().map(|e| u64::from(e.size())).sum();
    if total > MAX_OUTPUT {
        return Err(bad("table sizes exceed the decode limit"));
    }
    let all = brotli_inflate(comp, total as usize).map_err(bad)?;
    let mut at = 0usize;
    Ok(entries
        .iter()
        .map(|e| {
            let n = e.size() as usize;
            let v = all[at..at + n].to_vec();
            at += n;
            v
        })
        .collect())
}

/// `ChecksumCalculator.calculate_table_checksum`.
fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    })
}

fn offset_fields(n: usize) -> R<(u16, u16, u16)> {
    if n == 0 {
        return Err(Error::new(
            ErrorClass::Runtime,
            "Infinity (FloatDomainError)",
        ));
    }
    let sel = n.ilog2();
    let search = (1u32 << sel) * 16;
    Ok((search as u16, sel as u16, (n as u32 * 16 - search) as u16))
}

fn pad(len: usize) -> usize {
    (4 - len % 4) % 4
}

/// `Woff2Font.build_sfnt_in_memory`: directory order, one offset sequence, head adjusted.
fn build_single(flavor: u32, tables: &[(&[u8; 4], &[u8])]) -> R<Vec<u8>> {
    let (sr, es, rs) = offset_fields(tables.len())?;
    let mut out = Vec::new();
    out.extend(flavor.to_be_bytes());
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([sr, es, rs].iter().flat_map(|v| v.to_be_bytes()));
    let mut off = 12 + tables.len() * 16;
    let mut head_off = None;
    for (tag, d) in tables {
        out.extend(*tag);
        out.extend(checksum(d).to_be_bytes());
        out.extend((off as u32).to_be_bytes());
        out.extend((d.len() as u32).to_be_bytes());
        if *tag == b"head" && head_off.is_none() {
            head_off = Some(off);
        }
        off += d.len() + pad(d.len());
    }
    for (_, d) in tables {
        out.extend(*d);
        out.extend(std::iter::repeat_n(0u8, pad(d.len())));
    }
    if let Some(h) = head_off {
        let at = h + 8;
        let field = out
            .get_mut(at..at + 4)
            .ok_or_else(|| bad("head table too short"))?;
        field.fill(0);
        let sum = out.chunks(4).fold(0u32, |s, c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            s.wrapping_add(u32::from_be_bytes(w))
        });
        out[at..at + 4].copy_from_slice(&0xB1B0_AFBAu32.wrapping_sub(sum).to_be_bytes());
    }
    Ok(out)
}

/// `SfntBuilder.build`: offsets in the given order, directory and data sorted by tag.
fn build_sorted(flavor: u32, tables: &[([u8; 4], Vec<u8>)]) -> R<Vec<u8>> {
    let (sr, es, rs) = offset_fields(tables.len())?;
    let mut out = Vec::new();
    out.extend(flavor.to_be_bytes());
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([sr, es, rs].iter().flat_map(|v| v.to_be_bytes()));
    let mut cursor = 12 + tables.len() * 16;
    let mut recs: Vec<(&[u8; 4], u32, usize, &[u8])> = Vec::new();
    for (tag, d) in tables {
        recs.push((tag, checksum(d), cursor, d));
        cursor += d.len() + pad(d.len());
    }
    recs.sort_by_key(|r| *r.0);
    for (tag, sum, off, d) in &recs {
        out.extend(*tag);
        out.extend(sum.to_be_bytes());
        out.extend((*off as u32).to_be_bytes());
        out.extend((d.len() as u32).to_be_bytes());
    }
    for (_, _, _, d) in &recs {
        out.extend(*d);
        out.extend(std::iter::repeat_n(0u8, pad(d.len())));
    }
    Ok(out)
}

/// The `Woff2Header` fields the `woff2_validation` check reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Woff2Header {
    pub signature: u32,
    pub flavor: u32,
    pub reserved: u16,
    pub total_sfnt_size: u32,
    pub total_compressed_size: u32,
    pub major_version: u16,
    pub meta_offset: u32,
    pub meta_length: u32,
    pub meta_orig_length: u32,
}

impl Woff2Header {
    pub(crate) fn parse(data: &[u8]) -> R<Self> {
        Ok(Self {
            signature: be32(data, 0)?,
            flavor: be32(data, 4)?,
            reserved: be16(data, 14)?,
            total_sfnt_size: be32(data, 16)?,
            total_compressed_size: be32(data, 20)?,
            major_version: be16(data, 24)?,
            meta_offset: be32(data, 28)?,
            meta_length: be32(data, 32)?,
            meta_orig_length: be32(data, 36)?,
        })
    }
}

/// A single-font WOFF2 as `Woff2Font.from_file` lays it out in memory.
pub(crate) fn single(data: &[u8]) -> R<Vec<u8>> {
    let flavor = be32(data, 4)?;
    if flavor != 0x0001_0000 && flavor != 0x4F54_544F {
        return Err(Error::new(
            ErrorClass::InvalidFont,
            format!("Unknown WOFF2 flavor: 0x{flavor:x}"),
        ));
    }
    let n = be16(data, 12)? as usize;
    let (entries, pos) = read_directory(data, 48, n)?;
    let raw = inflate_split(data, pos, &entries)?;
    // Ruby keys the decompressed tables by tag: a later duplicate wins.
    let mut by_tag: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for (e, d) in entries.iter().zip(raw) {
        match by_tag.iter_mut().find(|(t, _)| *t == e.tag) {
            Some(slot) => slot.1 = d,
            None => by_tag.push((e.tag, d)),
        }
    }
    let get = |by: &[([u8; 4], Vec<u8>)], t: &[u8; 4]| {
        by.iter().find(|(x, _)| x == t).map(|(_, d)| d.clone())
    };
    let set = |by: &mut Vec<([u8; 4], Vec<u8>)>, t: [u8; 4], d: Vec<u8>| match by
        .iter_mut()
        .find(|(x, _)| *x == t)
    {
        Some(slot) => slot.1 = d,
        None => by.push((t, d)),
    };
    if let (Some(maxp), Some(hhea), Some(head)) = (
        get(&by_tag, b"maxp"),
        get(&by_tag, b"hhea"),
        get(&by_tag, b"head"),
    ) {
        let num_glyphs = be16(&maxp, 4)?;
        let n_hm = be16(&hhea, 34)?;
        let index_format = be16(&head, 50)?;
        let first = |tag: &[u8; 4]| entries.iter().find(|e| &e.tag == tag);
        let transformed = |tag: &[u8; 4]| first(tag).is_some_and(|e| e.transform_length.is_some());
        if let Some(g) = get(&by_tag, b"glyf").filter(|_| transformed(b"glyf")) {
            let (glyf, loca) = reconstruct_glyf(&g, num_glyphs, index_format)?;
            set(&mut by_tag, *b"glyf", glyf);
            set(&mut by_tag, *b"loca", loca);
        }
        if let Some(h) = get(&by_tag, b"hmtx").filter(|_| transformed(b"hmtx")) {
            set(
                &mut by_tag,
                *b"hmtx",
                reconstruct_hmtx(&h, num_glyphs, n_hm)?,
            );
        }
    }
    let tables: Vec<(&[u8; 4], &[u8])> = entries
        .iter()
        .filter_map(|e| {
            by_tag
                .iter()
                .find(|(t, _)| *t == e.tag)
                .map(|(t, d)| (t, d.as_slice()))
        })
        .collect();
    build_single(flavor, &tables)
}

/// Number of members in a WOFF2 collection, and member `index` laid out as
/// `Woff2Collection#font` builds it. The error for an out-of-range index is the
/// caller's (Ruby decodes every member first).
pub(crate) fn collection(data: &[u8], index: usize) -> R<Result<Vec<u8>, usize>> {
    if be32(data, 0)? != 0x774F_4632 {
        return Err(bad("bad signature"));
    }
    let n = be16(data, 12)? as usize;
    let (entries, mut pos) = read_directory(data, 48, n)?;
    if be32(data, pos)? != 0x0001_0000 {
        return Err(bad("Unsupported CollectionHeader version"));
    }
    pos += 4;
    let num_fonts = u255(data, &mut pos)? as usize;
    let mut fonts: Vec<(u32, Vec<usize>)> = Vec::new();
    for _ in 0..num_fonts {
        let nt = u255(data, &mut pos)? as usize;
        let flavor = be32(data, pos)?;
        pos += 4;
        let mut idxs = Vec::new();
        for _ in 0..nt {
            let i = u255(data, &mut pos)? as usize;
            if i >= entries.len() {
                return Err(bad("table index out of range"));
            }
            idxs.push(i);
        }
        fonts.push((flavor, idxs));
    }
    let raw = inflate_split(data, pos, &entries)?;
    let mut built = Vec::new();
    for (flavor, idxs) in &fonts {
        let mut tabs: Vec<([u8; 4], Vec<u8>)> = Vec::new();
        for &i in idxs {
            match tabs.iter_mut().find(|(t, _)| *t == entries[i].tag) {
                Some(slot) => slot.1 = raw[i].clone(),
                None => tabs.push((entries[i].tag, raw[i].clone())),
            }
        }
        if let Some(g) = tabs
            .iter()
            .find(|(t, _)| t == b"glyf")
            .map(|(_, d)| d.clone())
            .filter(|g| g.len() >= HEADER_SIZE)
        {
            let ng = tabs
                .iter()
                .find(|(t, _)| t == b"maxp")
                .filter(|(_, d)| d.len() >= 6)
                .map(|(_, d)| be16(d, 4));
            let fmt = tabs
                .iter()
                .find(|(t, _)| t == b"head")
                .filter(|(_, d)| d.len() >= 52)
                .map(|(_, d)| be16(d, 50));
            if let (Some(ng), Some(fmt)) = (ng, fmt) {
                let (glyf, loca) = reconstruct_glyf(&g, ng?, fmt?)?;
                for (t, d) in [(*b"glyf", glyf), (*b"loca", loca)] {
                    match tabs.iter_mut().find(|(x, _)| *x == t) {
                        Some(slot) => slot.1 = d,
                        None => tabs.push((t, d)),
                    }
                }
            }
        }
        built.push((*flavor, tabs));
    }
    match built.get(index) {
        Some((flavor, tabs)) => Ok(Ok(build_sorted(*flavor, tabs)?)),
        None => Ok(Err(built.len())),
    }
}

/// `Woff2::HmtxTransformer.reconstruct`.
fn reconstruct_hmtx(d: &[u8], num_glyphs: u16, n_hm: u16) -> R<Vec<u8>> {
    let mut pos = 0usize;
    let flags = *d.first().ok_or_else(|| bad("Unexpected end of stream"))?;
    pos += 1;
    let mut adv: Vec<u32> = Vec::new();
    if flags & 0x01 == 0 {
        adv.push(u255(d, &mut pos)?);
        for _ in 1..n_hm {
            let delta = i32::from(be16(d, pos)? as i16);
            pos += 2;
            adv.push((i64::from(*adv.last().unwrap()) + i64::from(delta)) as u32);
        }
    } else {
        for _ in 0..n_hm {
            adv.push(u255(d, &mut pos)?);
        }
    }
    let n_lsb = if flags & 0x02 != 0 {
        num_glyphs as usize
    } else {
        (n_hm as usize) + (num_glyphs as usize).saturating_sub(n_hm as usize)
    };
    let mut lsbs = Vec::with_capacity(n_lsb);
    for _ in 0..n_lsb {
        lsbs.push(be16(d, pos)?);
        pos += 2;
    }
    let mut out = Vec::new();
    for i in 0..n_hm as usize {
        let a = adv
            .get(i)
            .or(adv.last())
            .copied()
            .ok_or_else(|| bad("no advance widths"))?;
        out.extend((a as u16).to_be_bytes());
        out.extend(lsbs.get(i).copied().unwrap_or(0).to_be_bytes());
    }
    for i in n_hm as usize..num_glyphs as usize {
        out.extend(lsbs.get(i).copied().unwrap_or(0).to_be_bytes());
    }
    Ok(out)
}

struct Stream<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    /// `StringIO#read(n)`: short at the end, nothing (nil) once exhausted.
    fn read(&mut self, n: usize) -> Option<&'a [u8]> {
        if n == 0 {
            return Some(&[]);
        }
        if self.pos >= self.d.len() {
            return None;
        }
        let end = (self.pos + n).min(self.d.len());
        let s = &self.d[self.pos..end];
        self.pos = end;
        Some(s)
    }
    fn eof(&self) -> bool {
        self.pos >= self.d.len()
    }
    fn u255(&mut self) -> R<u32> {
        let mut p = self.pos;
        let v = u255(self.d, &mut p)?;
        self.pos = p;
        Ok(v)
    }
}

fn bit_set(map: &[u8], id: usize) -> bool {
    map.get(id >> 3)
        .is_some_and(|b| b & (0x80 >> (id & 7)) != 0)
}

/// `Woff2::GlyfLocaReconstruct#reconstruct`: `(glyf, loca)`.
fn reconstruct_glyf(d: &[u8], num_glyphs: u16, index_format: u16) -> R<(Vec<u8>, Vec<u8>)> {
    if d.len() < HEADER_SIZE {
        return Err(bad("transformed glyf too short"));
    }
    let option_flags = be16(d, 2)?;
    let ng = be16(d, 4)?;
    let fmt = be16(d, 6)?;
    if ng != num_glyphs {
        return Err(Error::new(
            ErrorClass::InvalidFont,
            format!("WOFF2 glyf numGlyphs mismatch: header says {ng}, expected {num_glyphs}"),
        ));
    }
    if fmt != index_format {
        return Err(Error::new(
            ErrorClass::InvalidFont,
            format!("WOFF2 glyf indexFormat mismatch: header says {fmt}, expected {index_format}"),
        ));
    }
    let n = num_glyphs as usize;
    let mut sizes = [0usize; 7];
    for (i, s) in sizes.iter_mut().enumerate() {
        *s = be32(d, 8 + 4 * i)? as usize;
    }
    let mut pos = HEADER_SIZE;
    let mut streams: Vec<&[u8]> = Vec::new();
    for s in sizes {
        streams.push(slice(d, pos, s).ok_or_else(|| bad("glyf stream out of range"))?);
        pos = pos.saturating_add(s);
    }
    let overlap: &[u8] = if option_flags & 1 != 0 {
        slice(d, pos, (n + 7) >> 3).unwrap_or(&[])
    } else {
        &[]
    };
    let bitmap_size = ((n + 31) >> 5) << 2;
    let bbox_blob = streams[5];
    let bbox_bitmap = &bbox_blob[..bitmap_size.min(bbox_blob.len())];
    let mut bbox_entries = Stream::new(bbox_blob.get(bitmap_size..).unwrap_or(&[]));
    let n_contours: Vec<i16> = streams[0]
        .chunks_exact(2)
        .map(|c| i16::from_be_bytes([c[0], c[1]]))
        .collect();
    let mut n_points = Stream::new(streams[1]);
    let mut flags = Stream::new(streams[2]);
    let mut glyph = Stream::new(streams[3]);
    let mut composite = Stream::new(streams[4]);
    let mut instr = Stream::new(streams[6]);

    let align = if index_format == 0 { 2 } else { 1 };
    let mut glyf: Vec<u8> = Vec::new();
    let mut offsets = vec![0usize];
    for id in 0..n {
        let nc = *n_contours
            .get(id)
            .ok_or_else(|| bad("missing contour count"))?;
        match nc {
            0 => {}
            -1 => decode_composite(
                &mut glyf,
                &mut composite,
                &mut glyph,
                &mut instr,
                &mut bbox_entries,
            )?,
            _ => decode_simple(
                &mut glyf,
                nc,
                id,
                (
                    &mut n_points,
                    &mut flags,
                    &mut glyph,
                    &mut instr,
                    &mut bbox_entries,
                ),
                bbox_bitmap,
                bit_set(overlap, id),
            )?,
        }
        if align > 1 && glyf.len() % align != 0 {
            glyf.resize(glyf.len() + align - glyf.len() % align, 0);
        }
        offsets.push(glyf.len());
    }
    let loca: Vec<u8> = if index_format == 0 {
        offsets
            .iter()
            .flat_map(|o| ((o / 2) as u16).to_be_bytes())
            .collect()
    } else {
        offsets
            .iter()
            .flat_map(|o| (*o as u32).to_be_bytes())
            .collect()
    };
    Ok((glyf, loca))
}

fn read_bbox(s: &mut Stream) -> [u8; 8] {
    match s.read(8) {
        Some(b) if b.len() == 8 => b.try_into().unwrap(),
        _ => [0; 8],
    }
}

fn decode_composite(
    out: &mut Vec<u8>,
    composite: &mut Stream,
    glyph: &mut Stream,
    instr: &mut Stream,
    bbox: &mut Stream,
) -> R<()> {
    let start = composite.pos;
    let mut have_instr = false;
    while !composite.eof() {
        let flags = composite
            .read(2)
            .filter(|b| b.len() == 2)
            .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]));
        composite.read(2);
        composite.read(if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            4
        } else {
            2
        });
        let t = if flags & WE_HAVE_A_SCALE != 0 {
            2
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            4
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        composite.read(t);
        have_instr = flags & WE_HAVE_INSTRUCTIONS != 0;
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    let components = &composite.d[start..composite.pos];
    let mut instructions: &[u8] = &[];
    if have_instr {
        let len = glyph.u255()? as usize;
        instructions = instr
            .read(len)
            .ok_or_else(|| bad("instruction stream exhausted"))?;
    }
    out.extend((-1i16).to_be_bytes());
    out.extend(read_bbox(bbox));
    out.extend(components);
    if have_instr {
        out.extend((instructions.len() as u16).to_be_bytes());
        out.extend(instructions);
    }
    Ok(())
}

type SimpleStreams<'s, 'a> = (
    &'s mut Stream<'a>,
    &'s mut Stream<'a>,
    &'s mut Stream<'a>,
    &'s mut Stream<'a>,
    &'s mut Stream<'a>,
);

fn decode_simple(
    out: &mut Vec<u8>,
    num_contours: i16,
    id: usize,
    (n_points, flags, glyph, instr, bbox): SimpleStreams,
    bbox_bitmap: &[u8],
    overlap: bool,
) -> R<()> {
    if num_contours < 0 {
        return Err(bad("negative contour count"));
    }
    let mut end_pts: Vec<u16> = Vec::new();
    let mut cumulative: i64 = -1;
    for _ in 0..num_contours {
        cumulative += i64::from(n_points.u255()?);
        end_pts.push(cumulative as u16);
    }
    let total = (cumulative + 1) as usize;
    let fl = flags
        .read(total)
        .filter(|f| f.len() == total)
        .ok_or_else(|| bad("flag stream exhausted"))?;
    let (pts, used) = triplet_decode(fl, &glyph.d[glyph.pos..], total).map_err(bad)?;
    glyph.pos += used;
    let inst_len = glyph.u255()? as usize;
    let instructions = instr
        .read(inst_len)
        .ok_or_else(|| bad("instruction stream exhausted"))?;
    let bb: [i16; 4] = if bit_set(bbox_bitmap, id) {
        let b = read_bbox(bbox);
        std::array::from_fn(|i| i16::from_be_bytes([b[2 * i], b[2 * i + 1]]))
    } else if pts.is_empty() {
        [0; 4]
    } else {
        let xs = pts.iter().map(|p| p.x);
        let ys = pts.iter().map(|p| p.y);
        [
            xs.clone().min().unwrap() as i16,
            ys.clone().min().unwrap() as i16,
            xs.max().unwrap() as i16,
            ys.max().unwrap() as i16,
        ]
    };
    out.extend(num_contours.to_be_bytes());
    out.extend(bb.iter().flat_map(|v| v.to_be_bytes()));
    out.extend(end_pts.iter().flat_map(|v| v.to_be_bytes()));
    out.extend((instructions.len() as u16).to_be_bytes());
    out.extend(instructions);

    let (mut px, mut py) = (0i32, 0i32);
    let mut tt: Vec<u8> = Vec::new();
    let (mut xb, mut yb) = (Vec::new(), Vec::new());
    for (i, p) in pts.iter().enumerate() {
        let (dx, dy) = (p.x - px, p.y - py);
        (px, py) = (p.x, p.y);
        let mut f = if p.on_curve { FLAG_ON_CURVE } else { 0 };
        if overlap && i == 0 {
            f |= FLAG_OVERLAP_SIMPLE;
        }
        f = axis_bits(f, &mut xb, FLAG_X_SHORT, FLAG_X_SAME_OR_POS, dx);
        f = axis_bits(f, &mut yb, FLAG_Y_SHORT, FLAG_Y_SAME_OR_POS, dy);
        tt.push(f);
    }
    let mut i = 0;
    while i < tt.len() {
        let f = tt[i];
        let mut count = 1;
        while i + count < tt.len() && tt[i + count] == f && count < 255 {
            count += 1;
        }
        if count > 1 {
            // Ruby writes the whole run length here (the TrueType repeat count is run - 1).
            out.extend([f | FLAG_REPEAT, count as u8]);
        } else {
            out.push(f);
        }
        i += count;
    }
    out.extend(xb);
    out.extend(yb);
    Ok(())
}

fn axis_bits(base: u8, out: &mut Vec<u8>, short: u8, same: u8, v: i32) -> u8 {
    if v == 0 {
        base | same
    } else if v.abs() <= 255 {
        out.push((v.unsigned_abs() & 0xff) as u8);
        base | short | if v > 0 { same } else { 0 }
    } else {
        out.extend(((v & 0xffff) as u16).to_be_bytes());
        base
    }
}
