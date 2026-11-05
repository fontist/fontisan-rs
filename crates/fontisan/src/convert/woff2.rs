//! Pure-Rust WOFF2 encoder for a single font (card 1h-woff2enc), matching the Ruby
//! `Converters::Woff2Encoder` at its defaults (`transform_tables: false`, Brotli quality 11,
//! mode font): `glyf`/`loca` are transformed, `hmtx` is not, `DSIG` is dropped, `head` gets
//! flag bit 11, the (possibly changed) `indexToLocFormat`, a fixed or clock `modified` stamp
//! and a recomputed `checkSumAdjustment`.
//!
//! Output equivalence with Ruby is `decoded-exact`: the decoded tables are byte-equal; the
//! Brotli bytes may differ.

mod transform;

use std::collections::BTreeMap;
use std::fmt;

use crate::woff::{checksum, offset_table, round4};

/// Seconds between 1904-01-01 (sfnt epoch) and 1970-01-01.
const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;
/// Highest glyf size addressable by a short loca (`uint16::MAX * 2`).
const SHORT_LOCA_GLYF_MAX: usize = 0x1FFFE;
const CHECKSUM_ADJUSTMENT_MAGIC: u32 = 0xB1B0_AFBA;
const HEAD_LEN: usize = 54;

const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];
/// Ruby's default `brotli_quality`.
pub const DEFAULT_BROTLI_QUALITY: u8 = 11;

pub type Tag = [u8; 4];

/// Why an encode was rejected (message text follows the Ruby encoder where it has one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn tag_str(t: &Tag) -> String {
    String::from_utf8_lossy(t).into_owned()
}

/// The tables of a single-font sfnt (empty tables included; the encoder skips them).
pub fn sfnt_tables(sfnt: &[u8]) -> Result<Vec<(Tag, Vec<u8>)>, Error> {
    if sfnt.len() < 12 {
        return Err(Error::new("sfnt too short"));
    }
    match &sfnt[0..4] {
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1" => {}
        _ => return Err(Error::new("not a single-font sfnt")),
    }
    let n = usize::from(u16::from_be_bytes([sfnt[4], sfnt[5]]));
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = sfnt
            .get(12 + 16 * i..28 + 16 * i)
            .ok_or_else(|| Error::new("truncated table directory"))?;
        let tag = [e[0], e[1], e[2], e[3]];
        let off = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let len = u32::from_be_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let data = off
            .checked_add(len)
            .and_then(|end| sfnt.get(off..end))
            .ok_or_else(|| Error::new(format!("table {} out of bounds", tag_str(&tag))))?;
        out.push((tag, data.to_vec()));
    }
    Ok(out)
}

/// Encode a single-font sfnt as WOFF2. `timestamp` is Unix seconds for `head.modified`
/// (`None` = the clock, like Ruby).
pub fn encode_woff2(sfnt: &[u8], timestamp: Option<i64>) -> Result<Vec<u8>, Error> {
    encode_woff2_with_quality(sfnt, timestamp, DEFAULT_BROTLI_QUALITY)
}

/// [`encode_woff2`] with Ruby's `brotli_quality` option (0..=11; larger is smaller and slower).
pub fn encode_woff2_with_quality(
    sfnt: &[u8],
    timestamp: Option<i64>,
    brotli_quality: u8,
) -> Result<Vec<u8>, Error> {
    encode_tables(sfnt_tables(sfnt)?, timestamp, brotli_quality)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn u16_at(b: &[u8], off: usize) -> Result<u16, Error> {
    b.get(off..off + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| Error::new("table too short"))
}

fn base128(out: &mut Vec<u8>, v: u32) {
    let mut bytes = vec![(v & 0x7F) as u8];
    let mut rest = v >> 7;
    while rest > 0 {
        bytes.push((rest & 0x7F) as u8 | 0x80);
        rest >>= 7;
    }
    bytes.reverse();
    out.extend_from_slice(&bytes);
}

struct Entry {
    tag: Tag,
    orig_length: u32,
    transform_length: Option<u32>,
}

impl Entry {
    fn flags(&self) -> u8 {
        let idx = KNOWN_TAGS
            .iter()
            .position(|k| **k == self.tag)
            .map_or(0x3F, |i| i as u8);
        let version = match &self.tag {
            b"glyf" | b"loca" => {
                if self.transform_length.is_some() {
                    0
                } else {
                    3
                }
            }
            _ => 0,
        };
        (version << 6) | idx
    }
}

/// Encode from `(tag, bytes)` pairs (later duplicates win).
pub fn encode_woff2_tables(
    tables: Vec<(Tag, Vec<u8>)>,
    timestamp: Option<i64>,
) -> Result<Vec<u8>, Error> {
    encode_tables(tables, timestamp, DEFAULT_BROTLI_QUALITY)
}

fn encode_tables(
    tables: Vec<(Tag, Vec<u8>)>,
    timestamp: Option<i64>,
    brotli_quality: u8,
) -> Result<Vec<u8>, Error> {
    if brotli_quality > 11 {
        return Err(Error::new("brotli_quality must be 0..11"));
    }
    let mut t: BTreeMap<Tag, Vec<u8>> = BTreeMap::new();
    for (tag, data) in tables {
        t.insert(tag, data);
    }
    t.retain(|_, v| !v.is_empty());
    for req in [b"head", b"hhea", b"maxp"] {
        if !t.contains_key(req) {
            return Err(Error::new(format!(
                "Font is missing required table: {}",
                tag_str(req)
            )));
        }
    }
    let has_cff = t.contains_key(b"CFF ") || t.contains_key(b"CFF2");
    if !has_cff && !t.contains_key(b"glyf") {
        return Err(Error::new("Font must have either glyf or CFF/CFF2 table"));
    }
    let flavor: u32 = if has_cff { 0x4F54_544F } else { 0x0001_0000 };

    // EncoderRules: drop DSIG, head flag bit 11, head.modified.
    t.remove(b"DSIG");
    {
        let head = t.get_mut(b"head").expect("checked above");
        if head.len() < HEAD_LEN {
            return Err(Error::new("head: table too short"));
        }
        let flags = u16::from_be_bytes([head[16], head[17]]) | 0x0800;
        head[16..18].copy_from_slice(&flags.to_be_bytes());
        let modified = timestamp.unwrap_or_else(now) + MAC_EPOCH_OFFSET;
        head[28..36].copy_from_slice(&modified.to_be_bytes());
    }

    // glyf/loca canonicalisation + transform.
    let mut transformed_glyf: Option<Vec<u8>> = None;
    let mut loca_canon: Option<Vec<u8>> = None;
    if t.contains_key(b"glyf") && t.contains_key(b"loca") {
        let num_glyphs = usize::from(u16_at(&t[b"maxp"], 4)?);
        let source_long = u16_at(&t[b"head"], 50)? != 0;
        let glyf_len = t[b"glyf"].len();
        let target_long = if glyf_len + 3 * num_glyphs <= SHORT_LOCA_GLYF_MAX {
            false
        } else {
            source_long
        };
        let canon = transform::canonicalize(
            &t[b"glyf"],
            &t[b"loca"],
            num_glyphs,
            source_long,
            target_long,
        )?;
        let out = transform::transform(&canon.glyf, &canon.loca, num_glyphs, target_long)?;
        if target_long != source_long {
            let head = t.get_mut(b"head").expect("checked above");
            head[50..52].copy_from_slice(&u16::from(target_long).to_be_bytes());
        }
        t.insert(*b"glyf", canon.glyf);
        t.remove(b"loca");
        transformed_glyf = Some(out);
        loca_canon = Some(canon.loca);
    }

    // head.checkSumAdjustment over the sfnt the decoder reconstructs.
    set_checksum_adjustment(&mut t, flavor, loca_canon.as_deref());

    // Directory entries in tag order; the synthetic loca follows glyf.
    let mut entries: Vec<Entry> = Vec::new();
    let mut stream: Vec<u8> = Vec::new();
    for (tag, data) in &t {
        if let (b"glyf", Some(tg)) = (tag, transformed_glyf.as_ref()) {
            entries.push(Entry {
                tag: *tag,
                orig_length: data.len() as u32,
                transform_length: Some(tg.len() as u32),
            });
            entries.push(Entry {
                tag: *b"loca",
                orig_length: loca_canon.as_ref().map_or(0, |l| l.len() as u32),
                transform_length: Some(0),
            });
            stream.extend_from_slice(tg);
        } else {
            entries.push(Entry {
                tag: *tag,
                orig_length: data.len() as u32,
                transform_length: None,
            });
            stream.extend_from_slice(data);
        }
    }

    let compressed = brotli_font(&stream, brotli_quality)?;

    let mut total_sfnt = 12 + 16 * t.len() as u64;
    if let Some(l) = &loca_canon {
        total_sfnt += 16 + round4(l.len() as u64);
    }
    total_sfnt += t.values().map(|d| round4(d.len() as u64)).sum::<u64>();

    let mut body = Vec::new();
    for e in &entries {
        let f = e.flags();
        body.push(f);
        if f & 0x3F == 0x3F {
            body.extend_from_slice(&e.tag);
        }
        base128(&mut body, e.orig_length);
        if let Some(tl) = e.transform_length {
            base128(&mut body, tl);
        }
    }
    body.extend_from_slice(&compressed);

    let mut out = Vec::with_capacity(48 + body.len() + 3);
    out.extend_from_slice(b"wOF2");
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // length, patched below
    out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(total_sfnt as u32).to_be_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    out.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    out.extend_from_slice(&[0u8; 20]); // meta + private block
    out.extend_from_slice(&body);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_be_bytes());
    Ok(out)
}

fn set_checksum_adjustment(t: &mut BTreeMap<Tag, Vec<u8>>, flavor: u32, loca: Option<&[u8]>) {
    let mut view: Vec<(Tag, &[u8])> = t.iter().map(|(k, v)| (*k, v.as_slice())).collect();
    if let Some(l) = loca {
        view.push((*b"loca", l));
        view.sort_by_key(|(k, _)| *k);
    }
    let n = view.len();
    let mut cursor = 12 + 16 * n;
    let mut dir = Vec::with_capacity(16 * n);
    let mut sum = 0u32;
    for (tag, data) in &view {
        let cs = if tag == b"head" {
            let mut h = data.to_vec();
            h[8..12].fill(0);
            checksum(&h)
        } else {
            checksum(data)
        };
        dir.extend_from_slice(tag);
        dir.extend_from_slice(&cs.to_be_bytes());
        dir.extend_from_slice(&(cursor as u32).to_be_bytes());
        dir.extend_from_slice(&(data.len() as u32).to_be_bytes());
        cursor += round4(data.len() as u64) as usize;
        sum = sum.wrapping_add(cs);
    }
    let total = checksum(&offset_table(flavor, n as u16))
        .wrapping_add(checksum(&dir))
        .wrapping_add(sum);
    let adj = CHECKSUM_ADJUSTMENT_MAGIC.wrapping_sub(total);
    if let Some(head) = t.get_mut(b"head") {
        head[8..12].copy_from_slice(&adj.to_be_bytes());
    }
}

fn brotli_font(data: &[u8], quality: u8) -> Result<Vec<u8>, Error> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: i32::from(quality),
        mode: brotli::enc::backward_references::BrotliEncoderMode::BROTLI_MODE_FONT,
        size_hint: data.len(),
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut input = data;
    brotli::BrotliCompress(&mut input, &mut out, &params)
        .map_err(|e| Error::new(format!("Brotli compression failed: {e}")))?;
    Ok(out)
}
