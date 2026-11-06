//! Equivalence of the bounded GSUB/GPOS feature enumeration with the original
//! (quadratic/cubic) algorithm, kept here as a reference. Compared on generated
//! tables (overlapping LangSys arrays at both byte parities, duplicates, short
//! reads, out-of-range counts and offsets; fixed seed) and on every corpus font's
//! GSUB/GPOS. `scripts()` and `features()` must agree for every script; a failing
//! read yields `[]` on both sides, so an `Err` difference would show as a diff.
mod common;

use common::*;
use fontisan::sfnt::*;

type T = [u8; 4];

// ---- reference: the pre-fix algorithm, verbatim semantics, over raw bytes ----
fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn rb_from(data: &[u8], start: i64) -> Option<&[u8]> {
    let len = data.len() as i64;
    let at = if start < 0 { len + start } else { start };
    if at < 0 || at > len {
        None
    } else {
        Some(&data[at as usize..])
    }
}

fn non_empty(data: &[u8], start: i64) -> Option<&[u8]> {
    rb_from(data, start).filter(|s| !s.is_empty())
}

fn ref_records(d: &[u8], at: usize, count: u16) -> Option<Vec<(T, u16)>> {
    let mut v = Vec::new();
    for i in 0..usize::from(count) {
        let p = at + i * 6;
        v.push((d.get(p..p + 4)?.try_into().ok()?, u16_at(d, p + 4)?));
    }
    Some(v)
}

fn ref_tag_list(d: &[u8]) -> Option<Vec<(T, u16)>> {
    ref_records(d, 2, u16_at(d, 0)?)
}

fn ref_lang_sys(d: &[u8]) -> Option<Vec<u16>> {
    u16_at(d, 0)?;
    u16_at(d, 2)?;
    let count = u16_at(d, 4)?;
    (0..usize::from(count))
        .map(|i| u16_at(d, 6 + 2 * i))
        .collect()
}

fn ref_indices(table_data: &[u8], script_off: u16, script: &T) -> Option<Vec<u16>> {
    let Some(list) = non_empty(table_data, i64::from(script_off) - 10) else {
        return Some(vec![]);
    };
    let scripts = ref_tag_list(list)?;
    let Some(&(_, offset)) = scripts.iter().find(|(t, _)| t == script) else {
        return Some(vec![]);
    };
    let Some(data) = non_empty(list, i64::from(offset)) else {
        return Some(vec![]);
    };
    let default_offset = u16_at(data, 0)?;
    let langs = ref_records(data, 4, u16_at(data, 2)?)?;
    let mut indices = Vec::new();
    if default_offset != 0 {
        if let Some(d) = non_empty(data, i64::from(default_offset)) {
            indices.extend(ref_lang_sys(d)?);
        }
    }
    for (_, off) in langs {
        if let Some(d) = non_empty(data, i64::from(off)) {
            indices.extend(ref_lang_sys(d)?);
        }
    }
    let mut seen = std::collections::HashSet::new();
    indices.retain(|i| seen.insert(*i));
    Some(indices)
}

/// Reference `(scripts, features-per-script)` of a whole table (>= 10 bytes).
fn reference(t: &[u8]) -> (Vec<T>, Vec<Vec<T>>) {
    let (so, fo) = (u16_at(t, 4).unwrap(), u16_at(t, 6).unwrap());
    let td = &t[10..];
    let scripts: Vec<T> = if so == 0 {
        vec![]
    } else {
        non_empty(td, i64::from(so) - 10)
            .and_then(ref_tag_list)
            .map(|r| r.into_iter().map(|(t, _)| t).collect())
            .unwrap_or_default()
    };
    let feats = scripts
        .iter()
        .map(|s| {
            if so == 0 || fo == 0 {
                return vec![];
            }
            let idx = ref_indices(td, so, s).unwrap_or_default();
            if idx.is_empty() {
                return vec![];
            }
            let Some(list) = non_empty(td, i64::from(fo) - 10) else {
                return vec![];
            };
            let Some(features) = ref_tag_list(list) else {
                return vec![];
            };
            let mut out: Vec<T> = Vec::new();
            for i in idx {
                if let Some((tag, _)) = features.get(usize::from(i)) {
                    if !out.contains(tag) {
                        out.push(*tag);
                    }
                }
            }
            out
        })
        .collect();
    (scripts, feats)
}

fn same(bytes: &[u8]) -> bool {
    if bytes.len() < 10 {
        return LayoutTable::from_bytes(bytes).is_err() || bytes.is_empty();
    }
    let t = LayoutTable::from_bytes(bytes).unwrap();
    let (rs, rf) = reference(bytes);
    let scripts = t.scripts();
    let paired: Vec<(T, Vec<T>)> = rs.iter().copied().zip(rf.iter().cloned()).collect();
    scripts == rs
        && scripts.iter().zip(&rf).all(|(s, f)| &t.features(s) == f)
        && t.script_features() == paired
}

// ---- generator ----
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn put16(v: &mut Vec<u8>, x: usize) {
    v.extend_from_slice(&(x as u16).to_be_bytes());
}

/// A table with a ScriptList (1-4 scripts, LangSys records that overlap, sit at
/// odd offsets, share bytes and carry duplicate / out-of-range / short counts),
/// a FeatureList of a few tags (some duplicated), then random mutations.
fn gen_table(r: &mut Rng) -> Vec<u8> {
    let nscripts = 1 + r.below(4);
    let nfeat = 1 + r.below(6);
    let tags: [&[u8; 4]; 4] = [b"liga", b"kern", b"mark", b"liga"];
    let mut feat = Vec::new();
    put16(&mut feat, nfeat);
    for i in 0..nfeat {
        feat.extend_from_slice(tags[i % 4]);
        put16(&mut feat, 0);
    }
    // One shared body area, referenced by all scripts at assorted (odd) offsets.
    let mut body = Vec::new();
    for _ in 0..(8 + r.below(40)) {
        put16(
            &mut body,
            if r.below(8) == 0 {
                r.below(70000)
            } else {
                r.below(nfeat + 2)
            },
        );
    }
    if r.below(2) == 0 {
        body.push(r.below(256) as u8);
    }
    let mut scripts = Vec::new();
    put16(&mut scripts, nscripts);
    let rec_end = 2 + 6 * nscripts;
    let mut tables = Vec::new();
    for i in 0..nscripts {
        scripts.extend_from_slice(&[b'S', b'c', b'r', b'a' + i as u8]);
        put16(&mut scripts, rec_end + tables.len());
        let nl = r.below(5);
        let base = 4 + 6 * nl; // script table: default, count, records, then body
        put16(
            &mut tables,
            if r.below(3) == 0 {
                0
            } else {
                base + r.below(body.len() + 2)
            },
        );
        put16(&mut tables, nl);
        for _ in 0..nl {
            tables.extend_from_slice(b"LANG");
            let off = match r.below(6) {
                0 => r.below(70000),
                1 => 0,
                _ => base + r.below(body.len() + 2),
            };
            put16(&mut tables, off);
        }
        // LangSys-looking header words inside the body so counts are plausible.
        let mut b = body.clone();
        for _ in 0..r.below(4) {
            if b.len() > 8 {
                let p = r.below(b.len() - 6);
                b[p..p + 4].copy_from_slice(&[0, 0, 0xff, 0xff]);
                put16(&mut Vec::new(), 0);
                let cnt = (r.below(30) as u16).to_be_bytes();
                b[p + 4..p + 6].copy_from_slice(&cnt);
            }
        }
        tables.extend_from_slice(&b);
    }
    scripts.extend_from_slice(&tables);
    let mut t = Vec::new();
    for x in [1usize, 0] {
        put16(&mut t, x);
    }
    let so = 10 + r.below(3) * 2 * r.below(2);
    put16(&mut t, if r.below(10) == 0 { 0 } else { so });
    put16(&mut t, 10 + scripts.len() + r.below(3) * r.below(2));
    put16(&mut t, 0);
    let fl_at = t.len();
    let _ = fl_at;
    t.extend_from_slice(&scripts);
    t.extend_from_slice(&feat);
    // Fix feature offset to the real position most of the time.
    if r.below(5) != 0 {
        let fo = (10 + scripts.len()) as u16;
        t[6..8].copy_from_slice(&fo.to_be_bytes());
    }
    for _ in 0..r.below(4) {
        match r.below(3) {
            0 if !t.is_empty() => {
                let p = r.below(t.len());
                t[p] = r.below(256) as u8;
            }
            1 => {
                let n = r.below(t.len() + 1);
                t.truncate(n);
            }
            _ => {
                let p = r.below(t.len().max(1));
                t.insert(p.min(t.len()), r.below(256) as u8);
            }
        }
    }
    t
}

#[test]
fn generated_tables_match_reference() {
    let mut r = Rng(0x1F_E4D2_5EED);
    let (mut cases, mut nonempty_feats, mut multi) = (0, 0, 0);
    for _ in 0..12000 {
        let t = gen_table(&mut r);
        assert!(same(&t), "mismatch on {}", hex(&t));
        cases += 1;
        if t.len() >= 10 {
            let lt = LayoutTable::from_bytes(&t).unwrap();
            let f: Vec<_> = lt.scripts().iter().map(|s| lt.features(s)).collect();
            nonempty_feats += f.iter().filter(|x| !x.is_empty()).count();
            multi += f.iter().filter(|x| x.len() > 1).count();
        }
    }
    eprintln!(
        "generated cases: {cases}, non-empty feature lists: {nonempty_feats}, multi-feature: {multi}"
    );
    assert!(nonempty_feats > 300, "generator too weak: {nonempty_feats}");
}

#[test]
fn corpus_layout_tables_match_reference() {
    let Some(corpus) = corpus() else { return };
    let (mut tables, mut fonts) = (0, 0);
    let mut worst = (0.0f64, String::new());
    for d in ["fonts", "derived"] {
        let Ok(rd) = std::fs::read_dir(corpus.join(d)) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else {
                continue;
            };
            for index in 0..4 {
                let Ok(font) = load(&bytes, index, combos()[0].1) else {
                    break;
                };
                fonts += 1;
                for tag in ["GSUB", "GPOS"] {
                    if let Some(raw) = font.table_data(tag) {
                        assert!(same(raw), "{} {tag}", e.path().display());
                        tables += 1;
                        let t = LayoutTable::from_bytes(raw).unwrap();
                        let (_, (used, budget)) = t.script_features_metered();
                        let ratio = used as f64 / budget.max(1) as f64;
                        if ratio > worst.0 {
                            worst = (ratio, format!("{} {tag}", e.path().display()));
                        }
                    }
                }
            }
        }
    }
    eprintln!("corpus faces: {fonts}, layout tables: {tables}");
    eprintln!("max work/budget ratio: {:.6} ({})", worst.0, worst.1);
    assert!(
        worst.0 < 0.25,
        "a real font uses {:.3} of the layout work budget: {}",
        worst.0,
        worst.1
    );
}
