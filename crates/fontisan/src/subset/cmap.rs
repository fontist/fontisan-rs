//! `TableStrategy::Cmap` and its format 4 / 12 builder.

use std::collections::BTreeMap;

use super::{Mapping, SubsetError};
use crate::sfnt::Cmap;

struct Seg {
    start_cp: u32,
    end_cp: u32,
    start_gid: u32,
}

fn coalesce(m: &BTreeMap<u32, u32>) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    for (&cp, &gid) in m {
        if let Some(c) = segs.last_mut() {
            if cp == c.end_cp + 1 && gid == c.start_gid + (cp - c.start_cp) {
                c.end_cp = cp;
                continue;
            }
        }
        segs.push(Seg {
            start_cp: cp,
            end_cp: cp,
            start_gid: gid,
        });
    }
    segs
}

fn format4(bmp: &BTreeMap<u32, u32>) -> Vec<u8> {
    let mut segs = coalesce(bmp);
    segs.push(Seg {
        start_cp: 0xFFFF,
        end_cp: 0xFFFF,
        start_gid: 0,
    });
    let seg_count = segs.len();
    let x2 = seg_count * 2;
    let mut search_range = (1usize << seg_count.ilog2()) * 2;
    if search_range < 2 {
        search_range = 2;
    }
    let entry_selector = (search_range / 2).ilog2() as u16;
    let range_shift = x2 - search_range;
    let mut o = Vec::new();
    for v in [
        4u16,
        0,
        0,
        x2 as u16,
        search_range as u16,
        entry_selector,
        range_shift as u16,
    ] {
        o.extend_from_slice(&v.to_be_bytes());
    }
    for s in &segs {
        o.extend_from_slice(&(s.end_cp as u16).to_be_bytes());
    }
    o.extend_from_slice(&0u16.to_be_bytes());
    for s in &segs {
        o.extend_from_slice(&(s.start_cp as u16).to_be_bytes());
    }
    for s in &segs {
        let d = (i64::from(s.start_gid) - i64::from(s.start_cp)) & 0xFFFF;
        o.extend_from_slice(&(d as u16).to_be_bytes());
    }
    for _ in &segs {
        o.extend_from_slice(&0u16.to_be_bytes());
    }
    let len = o.len() as u16;
    o[2..4].copy_from_slice(&len.to_be_bytes());
    o
}

fn format12(all: &BTreeMap<u32, u32>) -> Vec<u8> {
    let groups = coalesce(all);
    let mut o = Vec::new();
    o.extend_from_slice(&12u16.to_be_bytes());
    o.extend_from_slice(&0u16.to_be_bytes());
    o.extend_from_slice(&0u32.to_be_bytes());
    o.extend_from_slice(&0u32.to_be_bytes());
    o.extend_from_slice(&(groups.len() as u32).to_be_bytes());
    for g in &groups {
        o.extend_from_slice(&g.start_cp.to_be_bytes());
        o.extend_from_slice(&g.end_cp.to_be_bytes());
        o.extend_from_slice(&g.start_gid.to_be_bytes());
    }
    let len = o.len() as u32;
    o[4..8].copy_from_slice(&len.to_be_bytes());
    o
}

pub(super) fn subset(raw: &[u8], mapping: &Mapping) -> Result<Vec<u8>, SubsetError> {
    let table = Cmap::parse(raw).map_err(|e| SubsetError::corrupted(e.to_string()))?;
    let mut m: BTreeMap<u32, u32> = BTreeMap::new();
    for (cp, old) in table.mappings() {
        if let Some(new) = mapping.new_id(old) {
            m.insert(cp, new);
        }
    }
    if m.is_empty() {
        m.insert(0, 0);
    }
    let bmp: BTreeMap<u32, u32> = m
        .iter()
        .filter(|(c, _)| **c <= 0xFFFF)
        .map(|(c, g)| (*c, *g))
        .collect();
    let has_supp = m.keys().any(|c| *c > 0xFFFF);
    let mut subtables: Vec<Vec<u8>> = Vec::new();
    let mut records: Vec<(u16, u16, usize)> = Vec::new();
    if !bmp.is_empty() {
        subtables.push(format4(&bmp));
        let i = subtables.len() - 1;
        records.push((3, 1, i));
        records.push((0, 3, i));
    }
    if has_supp {
        subtables.push(format12(&m));
        let i = subtables.len() - 1;
        records.push((3, 10, i));
        records.push((0, 4, i));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    let mut offsets = Vec::new();
    let mut run = 4 + 8 * records.len();
    for s in &subtables {
        offsets.push(run);
        run += s.len();
    }
    for (p, e, i) in &records {
        out.extend_from_slice(&p.to_be_bytes());
        out.extend_from_slice(&e.to_be_bytes());
        out.extend_from_slice(&(offsets[*i] as u32).to_be_bytes());
    }
    for s in &subtables {
        out.extend_from_slice(s);
    }
    Ok(out)
}
