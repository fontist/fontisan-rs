//! glyf/loca/head bbox, closure, hhea and hmtx strategies.

use std::collections::BTreeSet;

use super::{Mapping, SubsetError, be16, find};
use crate::convert::Tables;

/// Parsed `loca` (`Tables::Loca#parse_with_context`).
fn parse_loca(tables: &Tables, num_glyphs: u32) -> Result<Vec<usize>, SubsetError> {
    let head = find(tables, "head").unwrap_or(&[]);
    let format = be16(head, 50).ok_or_else(|| SubsetError::corrupted("head table is too short"))?;
    if format > 1 {
        return Err(SubsetError::arg(format!(
            "indexToLocFormat must be 0 (short) or 1 (long), got: {format}"
        )));
    }
    if num_glyphs < 1 {
        return Err(SubsetError::arg(format!(
            "numGlyphs must be >= 1, got: {num_glyphs}"
        )));
    }
    let loca = find(tables, "loca")
        .ok_or_else(|| SubsetError::new("Fontisan::MissingTableError", "Table 'loca' not found"))?;
    let count = num_glyphs as usize + 1;
    let mut offs = Vec::with_capacity(count);
    for i in 0..count {
        let (v, kind) = if format == 0 {
            (be16(loca, 2 * i).map(|v| usize::from(v) * 2), "short")
        } else {
            (
                loca.get(4 * i..4 * i + 4)
                    .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize),
                "long",
            )
        };
        offs.push(v.ok_or_else(|| {
            SubsetError::corrupted(format!("Insufficient data for {kind} offset at index {i}"))
        })?);
    }
    for (i, w) in offs.windows(2).enumerate() {
        if w[1] < w[0] {
            return Err(SubsetError::corrupted(format!(
                "Offsets are not monotonically increasing: offset[{i}]={}, offset[{}]={}",
                w[0],
                i + 1,
                w[1]
            )));
        }
    }
    Ok(offs)
}

/// `Glyf#glyph_for` as far as the closure needs it: the component glyph ids of
/// a compound glyph, `None` for an empty or simple glyph.
fn components_of(glyf: &[u8], loca: &[usize], gid: u32) -> Result<Option<Vec<u32>>, SubsetError> {
    let (off, next) = (loca[gid as usize], loca[gid as usize + 1]);
    let size = next - off;
    if size == 0 {
        return Ok(None);
    }
    if off + size > glyf.len() {
        return Err(SubsetError::corrupted(format!(
            "Glyph {gid} extends beyond glyf table: offset={off}, size={size}, table_size={}",
            glyf.len()
        )));
    }
    let d = &glyf[off..off + size];
    if d.len() < 10 {
        return Err(SubsetError::corrupted(format!(
            "Insufficient glyph data for glyph {gid}: need at least 10 bytes, got {}",
            d.len()
        )));
    }
    let nc = i16::from_be_bytes([d[0], d[1]]);
    if nc >= 0 {
        return Ok(None);
    }
    if nc != -1 {
        return Err(SubsetError::corrupted(format!(
            "Invalid numberOfContours for glyph {gid}: {nc}. Must be >= 0 for simple glyphs or -1 for compound glyphs."
        )));
    }
    let mut out = Vec::new();
    let mut p = 10usize;
    loop {
        let h = d.get(p..p + 4).ok_or_else(|| {
            SubsetError::corrupted(format!(
                "Insufficient component header data for compound glyph {gid}"
            ))
        })?;
        let flags = u16::from_be_bytes([h[0], h[1]]);
        out.push(u32::from(u16::from_be_bytes([h[2], h[3]])));
        p += 4;
        let args = if flags & 0x0001 == 0 { 2 } else { 4 };
        if d.len() < p + args {
            return Err(SubsetError::corrupted(format!(
                "Insufficient argument data for compound glyph {gid}"
            )));
        }
        p += args;
        let tr = if flags & 0x0008 != 0 {
            2
        } else if flags & 0x0040 != 0 {
            4
        } else if flags & 0x0080 != 0 {
            8
        } else {
            0
        };
        if d.len() < p + tr {
            return Err(SubsetError::corrupted(format!(
                "Insufficient transformation data for compound glyph {gid}"
            )));
        }
        p += tr;
        if flags & 0x0020 == 0 {
            break;
        }
    }
    Ok(Some(out))
}

/// `GlyphAccessor#closure_for` (with notdef first).
pub(super) fn closure(
    tables: &Tables,
    glyph_ids: &[u32],
    num_glyphs: u32,
) -> Result<BTreeSet<u32>, SubsetError> {
    let mut result: BTreeSet<u32> = BTreeSet::from([0]);
    result.extend(glyph_ids.iter().copied().filter(|g| *g < num_glyphs));
    if find(tables, "CFF ").is_some() {
        return Ok(result);
    }
    let Some(glyf) = find(tables, "glyf") else {
        return Err(SubsetError::new(
            "Fontisan::MissingTableError",
            "Font has neither glyf nor CFF table",
        ));
    };
    let loca = parse_loca(tables, num_glyphs)?;
    let mut todo: Vec<u32> = result.iter().copied().collect();
    todo.reverse();
    let mut done = BTreeSet::new();
    while let Some(g) = todo.pop() {
        if !done.insert(g) {
            continue;
        }
        if let Some(cs) = components_of(glyf, &loca, g)? {
            for c in cs {
                if c < num_glyphs && result.insert(c) {
                    todo.insert(0, c);
                }
            }
        }
    }
    Ok(result)
}

/// Shared state of `GlyfLocaBuilder` (`SharedState`).
#[derive(Default)]
pub(super) struct State {
    built: bool,
    pub glyf: Vec<u8>,
    offsets: Vec<usize>,
    pub bbox: Option<[i16; 4]>,
}

impl State {
    pub fn build(
        &mut self,
        tables: &Tables,
        mapping: &Mapping,
        num_glyphs: u32,
    ) -> Result<(), SubsetError> {
        if self.built {
            return Ok(());
        }
        let glyf = find(tables, "glyf").ok_or_else(|| {
            SubsetError::new("Fontisan::MissingTableError", "Table 'glyf' not found")
        })?;
        let loca = parse_loca(tables, num_glyphs)?;
        let (mut x0, mut y0, mut x1, mut y1) =
            (1i32 << 30, 1i32 << 30, -(1i32 << 30), -(1i32 << 30));
        let mut cur = 0usize;
        for &old in &mapping.old_ids {
            self.offsets.push(cur);
            let off = loca[old as usize];
            let size = loca[old as usize + 1] - off;
            if size == 0 {
                continue;
            }
            // Ruby `String#[offset, size]`: short at the end, nil past it.
            if off > glyf.len() {
                return Err(SubsetError::new(
                    "NoMethodError",
                    "undefined method 'bytesize' for nil",
                ));
            }
            let mut g = glyf[off..(off + size).min(glyf.len())].to_vec();
            if g.len() >= 10 {
                let v = |i: usize| i32::from(i16::from_be_bytes([g[i], g[i + 1]]));
                x0 = x0.min(v(2));
                y0 = y0.min(v(4));
                x1 = x1.max(v(6));
                y1 = y1.max(v(8));
            }
            if g.len() >= 2 && i16::from_be_bytes([g[0], g[1]]) == -1 {
                remap_compound(&mut g, mapping)?;
            }
            cur += g.len();
            self.glyf.extend_from_slice(&g);
        }
        self.offsets.push(cur);
        self.built = true;
        if x0 <= x1 {
            self.bbox = Some([x0 as i16, y0 as i16, x1 as i16, y1 as i16]);
        }
        Ok(())
    }

    pub fn loca(&self, tables: &Tables) -> Vec<u8> {
        let head = find(tables, "head").unwrap_or(&[]);
        let mut out = Vec::new();
        if be16(head, 50) == Some(0) {
            for o in &self.offsets {
                out.extend_from_slice(&((o / 2) as u16).to_be_bytes());
            }
        } else {
            for o in &self.offsets {
                out.extend_from_slice(&(*o as u32).to_be_bytes());
            }
        }
        out
    }
}

/// `GlyfLocaBuilder#remap_compound!`.
fn remap_compound(d: &mut [u8], mapping: &Mapping) -> Result<(), SubsetError> {
    let mut off = 10usize;
    while off + 4 < d.len() {
        let flags = u16::from_be_bytes([d[off], d[off + 1]]);
        let old = u32::from(u16::from_be_bytes([d[off + 2], d[off + 3]]));
        let new = mapping.new_id(old).ok_or_else(|| {
            SubsetError::new(
                "Fontisan::SubsettingError",
                format!("Component glyph {old} not in subset"),
            )
        })?;
        d[off + 2..off + 4].copy_from_slice(&(new as u16).to_be_bytes());
        off += 4;
        off += if flags & 0x0001 == 0 { 2 } else { 4 };
        if flags & 0x0080 != 0 {
            off += 8;
        } else if flags & 0x0040 != 0 {
            off += 4;
        } else if flags & 0x0008 != 0 {
            off += 2;
        }
        if flags & 0x0020 == 0 {
            break;
        }
    }
    Ok(())
}

/// `Hmtx#parse_with_context`: (advance, lsb-bits) per glyph, `metric_for`
/// semantics.
pub(super) fn hmtx_metrics(
    tables: &Tables,
    raw: &[u8],
    num_glyphs: u32,
) -> Result<Vec<(u16, u16)>, SubsetError> {
    let hhea = find(tables, "hhea").unwrap_or(&[]);
    let nhm =
        u32::from(be16(hhea, 34).ok_or_else(|| SubsetError::corrupted("hhea table is too short"))?);
    if nhm < 1 {
        return Err(SubsetError::arg(format!(
            "numberOfHMetrics must be >= 1, got: {nhm}"
        )));
    }
    if num_glyphs < 1 {
        return Err(SubsetError::arg(format!(
            "numGlyphs must be >= 1, got: {num_glyphs}"
        )));
    }
    if nhm > num_glyphs {
        return Err(SubsetError::arg(format!(
            "numberOfHMetrics ({nhm}) cannot exceed numGlyphs ({num_glyphs})"
        )));
    }
    let short = |what: &str, i: u32| {
        SubsetError::corrupted(format!("Insufficient data for {what} at index {i}"))
    };
    let mut hm = Vec::new();
    for i in 0..nhm {
        let p = 4 * i as usize;
        let a = be16(raw, p).ok_or_else(|| short("hMetric", i))?;
        let l = be16(raw, p + 2).ok_or_else(|| short("hMetric", i))?;
        hm.push((a, l));
    }
    let mut lsbs = Vec::new();
    for i in 0..(num_glyphs - nhm) {
        let p = 4 * nhm as usize + 2 * i as usize;
        lsbs.push(be16(raw, p).ok_or_else(|| short("left side bearing", i))?);
    }
    let last = hm.last().map_or(0, |m| m.0);
    let mut out = hm;
    out.extend(lsbs.into_iter().map(|l| (last, l)));
    Ok(out)
}

pub(super) fn hmtx(
    mapping: &Mapping,
    raw: &[u8],
    tables: &Tables,
    num_glyphs: u32,
) -> Result<Vec<u8>, SubsetError> {
    let m = hmtx_metrics(tables, raw, num_glyphs)?;
    let mut out = Vec::new();
    for &old in &mapping.old_ids {
        let (a, l) = m[old as usize];
        out.extend_from_slice(&a.to_be_bytes());
        out.extend_from_slice(&l.to_be_bytes());
    }
    Ok(out)
}

pub(super) fn hhea(
    tables: &Tables,
    mapping: &Mapping,
    raw: &[u8],
    num_glyphs: u32,
) -> Result<Vec<u8>, SubsetError> {
    let mut d = raw
        .get(..36)
        .ok_or_else(|| SubsetError::corrupted("hhea table is too short"))?
        .to_vec();
    // The strategy reads numberOfHMetrics from a still-unparsed hmtx: the
    // subset glyph count.
    d[34..36].copy_from_slice(&(mapping.size() as u16).to_be_bytes());
    if let Some(hmtx) = find(tables, "hmtx") {
        let m = hmtx_metrics(tables, hmtx, num_glyphs)?;
        let max = mapping
            .old_ids
            .iter()
            .map(|&o| m[o as usize].0)
            .max()
            .unwrap_or(0);
        if max > 0 {
            d[10..12].copy_from_slice(&max.to_be_bytes());
        }
    }
    Ok(d)
}
