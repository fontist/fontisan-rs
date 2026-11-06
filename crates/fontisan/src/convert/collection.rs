//! Collection conversion (card 1h-coll): Ruby `Converters::CollectionConverter` for
//! TTC/OTC and WOFF2-collection sources.
//!
//! Ruby decides the source type from the input file's extension (`.otc` is OTC, any other
//! `ttcf` file TTC) and the target from the requested format. Bytes carry no path, so a
//! `ttcf` input is treated as `.ttc`, like [`super::extract`]. Consequences, all measured
//! against the goldens:
//!
//! * same source and target type (`ttc` to `ttc`, WOFF2 collection to `woff2`) copies the input;
//! * otherwise the members are unpacked, `Collection::Builder` repacks them for `ttc`/`otc`
//!   (it ignores the format: both targets give the same bytes), or
//!   `Woff2::CollectionEncoder` for `woff2` (decoded-exact, the Brotli stream may differ).
//!
//! No outline conversion happens (`target_format` is "preserve" for `Fontisan.convert`).

use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};

use super::woff2 as w2;
use super::{ConvertError, Tables, Target, tables_of};
use crate::woff::checksum;

const TTCF: u32 = 0x7474_6366;
const SFNT_TRUETYPE: u32 = 0x0001_0000;
const SFNT_OTTO: u32 = 0x4F54_544F;

/// One unpacked collection member: Ruby's `font.header.sfnt_version`, `table_names`
/// (directory order) and `table_data`.
struct Member {
    sfnt_version: u32,
    tables: Tables,
}

fn fail(msg: impl AsRef<str>) -> ConvertError {
    ConvertError::transform(msg)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Members of a `ttcf` file (`SfntFont.from_collection`: table bytes read at their
/// absolute offsets).
fn ttc_members(input: &[u8]) -> Result<Vec<Member>, ConvertError> {
    let bad = || fail("invalid collection header");
    let count = be32(input, 8).ok_or_else(bad)? as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let off = be32(input, 12 + 4 * i).ok_or_else(bad)? as usize;
        let hdr = input.get(off..off + 12).ok_or_else(bad)?;
        let n = usize::from(u16::from_be_bytes([hdr[4], hdr[5]]));
        let mut tables: Tables = Vec::with_capacity(n);
        for t in 0..n {
            let e = input
                .get(off + 12 + 16 * t..off + 28 + 16 * t)
                .ok_or_else(bad)?;
            let tag = [e[0], e[1], e[2], e[3]];
            let start = (be32(e, 8).unwrap() as usize).min(input.len());
            let end = start
                .saturating_add(be32(e, 12).unwrap() as usize)
                .min(input.len());
            let data = input[start..end].to_vec();
            match tables.iter_mut().find(|(x, _)| *x == tag) {
                Some(slot) => slot.1 = data,
                None => tables.push((tag, data)),
            }
        }
        out.push(Member {
            sfnt_version: u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]),
            tables,
        });
    }
    Ok(out)
}

/// Face 0 of a `ttcf` file as a standalone sfnt (what Ruby's loader hands the subsetter).
pub(crate) fn first_face(input: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let member = ttc_members(input)?
        .into_iter()
        .next()
        .ok_or_else(|| fail("collection index out of range"))?;
    Ok(super::write_font(&member.tables, member.sfnt_version))
}

/// Members of a WOFF2 collection, as `Woff2Collection#font` rebuilds them.
fn woff2_members(input: &[u8]) -> Result<Vec<Member>, ConvertError> {
    let count = match crate::sfnt::woff2_view::collection(input, usize::MAX)
        .map_err(|e| fail(format!("{e:?}")))?
    {
        Err(n) => n,
        Ok(_) => return Err(fail("collection index out of range")),
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let sfnt = crate::sfnt::woff2_view::collection(input, i)
            .map_err(|e| fail(format!("{e:?}")))?
            .map_err(|_| fail("collection index out of range"))?;
        let (sfnt_version, tables) = tables_of(&sfnt)?;
        out.push(Member {
            sfnt_version,
            tables,
        });
    }
    Ok(out)
}

fn sha(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn align4(n: usize) -> usize {
    n.next_multiple_of(4)
}

fn search_params(num_tables: usize) -> (u16, u16, u16) {
    let mut max_power = 0u32;
    let mut n = num_tables;
    while n > 1 {
        n >>= 1;
        max_power += 1;
    }
    let search_range = (1usize << max_power) * 16;
    (
        search_range as u16,
        max_power as u16,
        (num_tables * 16).wrapping_sub(search_range) as u16,
    )
}

/// `Collection::Builder#build`: TableDeduplicator + OffsetCalculator + Writer.
fn build_ttc(members: &[Member]) -> Vec<u8> {
    // Canonical tables in first-seen order over fonts then directory order; a table is
    // shared when more than one font uses it. Identity is (tag, sha256 of the data).
    struct Canon {
        key: ([u8; 4], [u8; 32]),
        size: usize,
        uses: usize,
        offset: usize,
    }
    let mut canon: Vec<Canon> = Vec::new();
    let mut index: HashMap<([u8; 4], [u8; 32]), usize> = HashMap::new();
    let mut font_ids: Vec<Vec<([u8; 4], usize)>> = Vec::new();
    for m in members {
        let mut ids = Vec::new();
        for (tag, data) in &m.tables {
            let key = (*tag, sha(data));
            let id = *index.entry(key).or_insert_with(|| {
                canon.push(Canon {
                    key,
                    size: data.len(),
                    uses: 0,
                    offset: 0,
                });
                canon.len() - 1
            });
            canon[id].uses += 1;
            ids.push((*tag, id));
        }
        font_ids.push(ids);
    }

    // OffsetCalculator.
    let mut dir_offsets = Vec::with_capacity(members.len());
    let mut cur = 12 + 4 * members.len();
    for m in members {
        dir_offsets.push(cur);
        cur = align4(cur + 12 + 16 * m.tables.len());
    }
    for shared in [true, false] {
        for c in canon.iter_mut().filter(|c| (c.uses > 1) == shared) {
            c.offset = cur;
            cur = align4(cur + c.size);
        }
    }

    // Writer.
    let mut out = Vec::new();
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(members.len() as u32).to_be_bytes());
    for o in &dir_offsets {
        out.extend_from_slice(&(*o as u32).to_be_bytes());
    }
    let pad_to = |out: &mut Vec<u8>, target: usize| {
        if out.len() < target {
            out.resize(target, 0);
        }
    };
    for (fi, m) in members.iter().enumerate() {
        pad_to(&mut out, dir_offsets[fi]);
        let mut sorted: Vec<&([u8; 4], usize)> = font_ids[fi].iter().collect();
        sorted.sort_by_key(|(t, _)| *t);
        let (sr, es, rs) = search_params(sorted.len());
        out.extend_from_slice(&m.sfnt_version.to_be_bytes());
        out.extend_from_slice(&(sorted.len() as u16).to_be_bytes());
        out.extend_from_slice(&sr.to_be_bytes());
        out.extend_from_slice(&es.to_be_bytes());
        out.extend_from_slice(&rs.to_be_bytes());
        for (tag, id) in sorted {
            let data = &m.tables.iter().find(|(t, _)| t == tag).unwrap().1;
            out.extend_from_slice(tag);
            out.extend_from_slice(&checksum(data).to_be_bytes());
            out.extend_from_slice(&(canon[*id].offset as u32).to_be_bytes());
            out.extend_from_slice(&(canon[*id].size as u32).to_be_bytes());
        }
    }
    // Table data by offset; a later canonical table at the same offset replaces an
    // earlier one (Ruby's `tables_by_offset` hash).
    let mut by_offset: BTreeMap<usize, &[u8]> = BTreeMap::new();
    for shared in [true, false] {
        for c in canon.iter().filter(|c| (c.uses > 1) == shared) {
            let data = members
                .iter()
                .flat_map(|m| m.tables.iter())
                .find(|(t, d)| *t == c.key.0 && sha(d) == c.key.1)
                .map(|(_, d)| d.as_slice())
                .unwrap_or(&[]);
            by_offset.insert(c.offset, data);
        }
    }
    for (offset, data) in by_offset {
        pad_to(&mut out, offset);
        out.extend_from_slice(data);
        out.resize(out.len() + (4 - data.len() % 4) % 4, 0);
    }
    out
}

fn uint255(out: &mut Vec<u8>, v: usize) -> Result<(), ConvertError> {
    match v {
        0..=252 => out.push(v as u8),
        253..=505 => out.extend_from_slice(&[255, (v - 253) as u8]),
        506..=761 => out.extend_from_slice(&[254, (v - 506) as u8]),
        762..=65_535 => {
            out.push(253);
            out.extend_from_slice(&(v as u16).to_be_bytes());
        }
        _ => return Err(fail("255UInt16 requires 0 <= value <= 65535")),
    }
    Ok(())
}

/// One encoder layout row (`CollectionEncoder#build_layout`).
struct Row {
    tag: [u8; 4],
    data: Option<Vec<u8>>,
    orig_length: usize,
    transform_length: Option<usize>,
}

struct Prepared {
    tables: Vec<([u8; 4], Vec<u8>)>,
    loca_orig_length: Option<usize>,
}

fn prepare(m: &Member, timestamp: i64) -> Result<Prepared, ConvertError> {
    let werr = |e: w2::Error| fail(e.to_string());
    let mut tables: Vec<([u8; 4], Vec<u8>)> = m
        .tables
        .iter()
        .filter(|(_, d)| !d.is_empty())
        .cloned()
        .collect();
    // EncoderRules: drop DSIG, head flag bit 11, head.modified.
    tables.retain(|(t, _)| t != b"DSIG");
    if let Some((_, head)) = tables.iter_mut().find(|(t, _)| t == b"head") {
        if head.len() < w2::HEAD_LEN {
            return Err(fail("head: table too short"));
        }
        head.truncate(w2::HEAD_LEN);
        let flags = u16::from_be_bytes([head[16], head[17]]) | 0x0800;
        head[16..18].copy_from_slice(&flags.to_be_bytes());
        head[28..36].copy_from_slice(&(timestamp + w2::MAC_EPOCH_OFFSET).to_be_bytes());
    }
    let get = |tables: &[([u8; 4], Vec<u8>)], tag: &[u8; 4]| {
        tables
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, d)| d.clone())
    };
    let mut loca_orig_length = None;
    if let (Some(glyf), Some(loca), Some(maxp), Some(head)) = (
        get(&tables, b"glyf"),
        get(&tables, b"loca"),
        get(&tables, b"maxp"),
        get(&tables, b"head"),
    ) {
        let num_glyphs = usize::from(w2::u16_at(&maxp, 4).map_err(werr)?);
        let mut long = w2::u16_at(&head, 50).map_err(werr)? != 0;
        let (mut glyf, mut loca) = (glyf, loca);
        if !long && w2::short_loca_overflows(glyf.len(), num_glyphs) {
            (glyf, loca) = w2::canonical_long_loca(&glyf, &loca, num_glyphs).map_err(werr)?;
            long = true;
            for (t, d) in tables.iter_mut() {
                if t == b"head" {
                    d[50..52].copy_from_slice(&1u16.to_be_bytes());
                }
            }
        }
        let transformed = w2::transform_glyf(&glyf, &loca, num_glyphs, long).map_err(werr)?;
        loca_orig_length = Some(loca.len());
        for (t, d) in tables.iter_mut() {
            if t == b"glyf" {
                *d = transformed.clone();
            }
        }
        tables.retain(|(t, _)| t != b"loca");
    }
    Ok(Prepared {
        tables,
        loca_orig_length,
    })
}

/// `Woff2::CollectionEncoder#encode_fonts` at its defaults (Brotli quality 11).
fn encode_woff2_collection(members: &[Member], timestamp: i64) -> Result<Vec<u8>, ConvertError> {
    if members.is_empty() {
        return Err(fail("fonts cannot be empty"));
    }
    let prepared = members
        .iter()
        .map(|m| prepare(m, timestamp))
        .collect::<Result<Vec<_>, _>>()?;

    // Deduplicate by (tag, sha256); ckeys are "tag@first-font-index".
    let mut seen: HashMap<([u8; 4], [u8; 32]), String> = HashMap::new();
    let mut font_keys: Vec<Vec<([u8; 4], String)>> = Vec::new();
    for (fi, p) in prepared.iter().enumerate() {
        let mut keys = Vec::new();
        for (tag, data) in &p.tables {
            let ck = seen
                .entry((*tag, sha(data)))
                .or_insert_with(|| format!("{}@{fi}", w2::tag_str(tag)))
                .clone();
            keys.push((*tag, ck));
        }
        font_keys.push(keys);
    }

    // Layout: per font glyf, its loca placeholder, then the rest by tag.
    let mut rows: Vec<Row> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (fi, p) in prepared.iter().enumerate() {
        let mut order: Vec<[u8; 4]> = p.tables.iter().map(|(t, _)| *t).collect();
        if p.loca_orig_length.is_some() {
            order.retain(|t| t != b"glyf");
            order.sort();
            order.insert(0, *b"glyf");
        } else {
            order.sort();
        }
        for tag in order {
            let ck = &font_keys[fi].iter().find(|(t, _)| *t == tag).unwrap().1;
            if !at.contains_key(ck) {
                let data = p.tables.iter().find(|(t, _)| *t == tag).unwrap().1.clone();
                at.insert(ck.clone(), rows.len());
                rows.push(Row {
                    tag,
                    orig_length: data.len(),
                    transform_length: (&tag == b"glyf").then_some(data.len()),
                    data: Some(data),
                });
            }
            if &tag == b"glyf" {
                if let Some(loca_len) = p.loca_orig_length {
                    let pk = format!("loca_placeholder_for:{ck}");
                    if let std::collections::hash_map::Entry::Vacant(e) = at.entry(pk) {
                        e.insert(rows.len());
                        rows.push(Row {
                            tag: *b"loca",
                            data: None,
                            orig_length: loca_len,
                            transform_length: Some(0),
                        });
                    }
                }
            }
        }
    }

    let mut stream = Vec::new();
    for r in &rows {
        if let Some(d) = &r.data {
            stream.extend_from_slice(d);
        }
    }
    let compressed =
        w2::brotli_font(&stream, w2::DEFAULT_BROTLI_QUALITY).map_err(|e| fail(e.to_string()))?;

    let mut out = vec![0u8; 48];
    for r in &rows {
        let e = w2::Entry {
            tag: r.tag,
            orig_length: r.orig_length as u32,
            transform_length: r.transform_length.map(|t| t as u32),
        };
        let f = e.flags();
        out.push(f);
        if f & 0x3F == 0x3F {
            out.extend_from_slice(&r.tag);
        }
        w2::base128(&mut out, e.orig_length);
        if let Some(t) = e.transform_length {
            w2::base128(&mut out, t);
        }
    }
    out.extend_from_slice(&TTCF_HEADER_VERSION.to_be_bytes());
    uint255(&mut out, members.len())?;
    for (fi, m) in members.iter().enumerate() {
        let glyf_key = font_keys[fi]
            .iter()
            .find(|(t, _)| t == b"glyf")
            .map(|(_, k)| k);
        let placeholder = glyf_key.map(|k| format!("loca_placeholder_for:{k}"));
        let has_placeholder = placeholder.as_ref().is_some_and(|p| at.contains_key(p));
        let mut tags: Vec<[u8; 4]> = font_keys[fi].iter().map(|(t, _)| *t).collect();
        if has_placeholder {
            tags.push(*b"loca");
        }
        tags.sort();
        uint255(&mut out, tags.len())?;
        let flavor = if m.tables.iter().any(|(t, _)| t == b"CFF " || t == b"CFF2") {
            SFNT_OTTO
        } else {
            SFNT_TRUETYPE
        };
        out.extend_from_slice(&flavor.to_be_bytes());
        for tag in tags {
            let ck = font_keys[fi]
                .iter()
                .find(|(t, _)| *t == tag)
                .map(|(_, k)| k.clone())
                .or_else(|| placeholder.clone())
                .ok_or_else(|| fail("missing table key"))?;
            uint255(&mut out, at[&ck])?;
        }
    }
    out.extend_from_slice(&compressed);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    let total = out.len() as u32;
    out[0..4].copy_from_slice(b"wOF2");
    out[4..8].copy_from_slice(&TTCF.to_be_bytes());
    out[8..12].copy_from_slice(&total.to_be_bytes());
    out[12..14].copy_from_slice(&(rows.len() as u16).to_be_bytes());
    // 14..16 reserved, 16..20 totalSfntSize (reference only) stay zero.
    out[20..24].copy_from_slice(&(compressed.len() as u32).to_be_bytes());
    out[24..26].copy_from_slice(&1u16.to_be_bytes());
    Ok(out)
}

/// CollectionHeader version 1.0.
const TTCF_HEADER_VERSION: u32 = 0x0001_0000;

/// What a collection input is, from its bytes alone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// `ttcf` (Ruby: `.ttc` extension; `.otc` is not distinguishable here).
    Ttc,
    Woff2Collection,
}

fn has_outlines(tables: &Tables) -> bool {
    tables
        .iter()
        .any(|(t, _)| t == b"glyf" || t == b"CFF " || t == b"CFF2")
}

/// `CollectionConverter#convert` for `target` in {ttc, otc, woff2}.
pub(crate) fn convert(
    input: &[u8],
    source: Source,
    target: Target,
    timestamp: i64,
) -> Result<Vec<u8>, ConvertError> {
    match (source, target) {
        (Source::Ttc, Target::Ttc) | (Source::Woff2Collection, Target::Woff2) => Ok(input.to_vec()),
        (_, Target::Ttc | Target::Otc | Target::Woff2) => {
            let members = match source {
                Source::Ttc => ttc_members(input)?,
                Source::Woff2Collection => woff2_members(input)?,
            };
            // Ruby's `detect_font_format` runs on every unpacked member: an
            // outline-less member (no glyf, CFF or CFF2) aborts the conversion.
            if members.iter().any(|m| !has_outlines(&m.tables)) {
                return Err(ConvertError::new(
                    super::ERROR_CLASS,
                    "Conversion failed: Cannot detect font format",
                ));
            }
            if target == Target::Woff2 {
                encode_woff2_collection(&members, timestamp)
            } else {
                Ok(build_ttc(&members))
            }
        }
        _ => Err(ConvertError::new(
            "ArgumentError",
            "Target format is not a collection format. Use ttc, otc, or dfont for collection conversion.",
        )),
    }
}

#[cfg(test)]
mod first_face_tests {
    use super::*;

    #[test]
    fn first_face_is_member_zero_as_a_standalone_sfnt() {
        let member = |tag: &[u8; 4]| Member {
            sfnt_version: SFNT_TRUETYPE,
            tables: vec![(*tag, vec![1, 2, 3, 4])],
        };
        let ttc = build_ttc(&[member(b"AAAA"), member(b"BBBB")]);
        let face = first_face(&ttc).expect("face 0");
        let (version, tables) = tables_of(&face).unwrap();
        assert_eq!(version, SFNT_TRUETYPE);
        assert_eq!(tables, vec![(*b"AAAA", vec![1, 2, 3, 4])]);
    }
}
