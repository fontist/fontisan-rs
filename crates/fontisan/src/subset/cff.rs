//! The `CFF ` strategy of `Subset::TableStrategy::Cff`.
//!
//! Ruby does not subset the CFF tables: it converts the whole font to a UFO
//! (`Ufo::Convert::FromBinData`: Type 2 charstring path interpreter, post
//! names, hmtx widths), keeps the mapped glyphs and rebuilds a trivial CFF
//! with `Ufo::Compile::Cff.build` (Type 2 charstrings from the path, empty
//! Private DICT, placeholder charset). The font name is always `Untitled`:
//! `FromBinData.extract_name_records` calls `Name#string_for_record`, which
//! does not exist, and the `NoMethodError` is swallowed by `extract_info`
//! before the PostScript name is set.

use super::glyf::hmtx_metrics;
use super::{Mapping, SubsetError, find};
use crate::cffread;
use crate::convert::Tables;
use crate::sfnt::Post;
use crate::t2interp::{Interp, Seg};

/// One UFO glyph: its (unique) name, advance and path.
struct Glyph {
    key: String,
    width: u16,
    path: Vec<Seg>,
}

pub(super) fn subset(
    tables: &Tables,
    mapping: &Mapping,
    data: &[u8],
    num_glyphs: u32,
) -> Result<Vec<u8>, SubsetError> {
    let cff = cffread::parse(data)
        .map_err(|e| SubsetError::corrupted(format!("Failed to parse CFF table: {e}")))?;
    let names = find(tables, "post")
        .and_then(|d| Post::parse(d).ok())
        .map(|p| p.glyph_names())
        .unwrap_or_default();
    let metrics = find(tables, "hmtx").and_then(|raw| hmtx_metrics(tables, raw, num_glyphs).ok());

    // FromBinData.extract_cff_glyphs: every source glyph, in gid order.
    // Every glyph is still interpreted (a bad charstring anywhere fails the subset, like
    // Ruby), but only the glyphs `filter_ufo_glyphs!` keeps hold their outline: a font of
    // thousands of glyphs with a large outline each must not be held whole.
    let mut all: Vec<Glyph> = Vec::with_capacity(num_glyphs as usize);
    let mut total_ops = crate::t2interp::MAX_TOTAL_OPS;
    let mut seen = std::collections::HashSet::with_capacity(num_glyphs as usize);
    for gid in 0..num_glyphs as usize {
        let key = match names.get(gid) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => format!("gid{gid}"),
        };
        if !seen.insert(key.clone()) {
            return Err(SubsetError::new("Fontisan::Ufo::GlyphExistsError", key));
        }
        let width = metrics.as_ref().and_then(|m| m.get(gid)).map_or(0, |m| m.0);
        let mut path = Vec::new();
        if cff.has_private {
            if let Some(cs) = cff.charstrings.get(gid) {
                let mut it = Interp::new(&cff.global_subrs, cff.local_subrs.as_deref());
                it.run_main(cs).map_err(|e| {
                    SubsetError::corrupted(format!(
                        "Failed to get CharString for glyph {gid}: Failed to parse CharString: {e}"
                    ))
                })?;
                total_ops = total_ops.checked_sub(it.ops_used()).ok_or_else(|| {
                    SubsetError::corrupted(format!(
                        "Failed to get CharString for glyph {gid}: Failed to parse CharString: {}",
                        crate::t2interp::TOTAL_OPS_EXCEEDED
                    ))
                })?;
                if mapping.new_id(gid as u32).is_some() || key == ".notdef" {
                    path = it.path;
                }
            }
        }
        all.push(Glyph { key, width, path });
    }

    // filter_ufo_glyphs!: keep mapped gids, plus any glyph named .notdef.
    let mut kept: Vec<Glyph> = all
        .into_iter()
        .enumerate()
        .filter(|(gid, g)| mapping.new_id(*gid as u32).is_some() || g.key == ".notdef")
        .map(|(_, g)| g)
        .collect();
    // The first glyph is re-keyed .notdef: deleted and re-inserted last, or
    // replacing an existing .notdef entry in place.
    if kept.first().is_some_and(|g| g.key != ".notdef") {
        let mut first = kept.remove(0);
        first.key = ".notdef".into();
        match kept.iter().position(|g| g.key == ".notdef") {
            Some(i) => kept[i] = first,
            None => kept.push(first),
        }
    }

    let charstrings: Vec<Vec<u8>> = kept.iter().map(charstring_for).collect();
    Ok(build(b"Untitled", &charstrings, kept.len()))
}

/// `FromBinData.convert_cff_path_to_ufo` + `Glyph#to_outline` +
/// `Compile::Cff.charstring_for`.
fn charstring_for(g: &Glyph) -> Vec<u8> {
    let width = i64::from(g.width);
    let mut out = Vec::new();
    // Line and curve segments before the first move are ignored.
    let started = g.path.iter().position(|s| matches!(s, Seg::Move(..)));
    let Some(start) = started else {
        if width != 0 {
            number(&mut out, width);
        }
        out.push(14);
        return out;
    };
    number(&mut out, width);
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut first = true;
    let t = |v: f64| v as i64;
    for seg in &g.path[start..] {
        match *seg {
            Seg::Move(x, y) => {
                let (x, y) = (t(x), t(y));
                let (dx, dy) = (x - cx, y - cy);
                if first && dx == 0 {
                    number(&mut out, dy);
                    out.push(4);
                } else if first && dy == 0 {
                    number(&mut out, dx);
                    out.push(22);
                } else {
                    number(&mut out, dx);
                    number(&mut out, dy);
                    out.push(21);
                }
                first = false;
                cx = x;
                cy = y;
            }
            Seg::Line(x, y) => {
                let (x, y) = (t(x), t(y));
                number(&mut out, x - cx);
                number(&mut out, y - cy);
                out.push(5);
                cx = x;
                cy = y;
            }
            Seg::Curve([x1, y1, x2, y2, x, y]) => {
                let [x1, y1, x2, y2, x, y] = [x1, y1, x2, y2, x, y].map(t);
                for d in [x1 - cx, y1 - cy, x2 - x1, y2 - y1, x - x2, y - y2] {
                    number(&mut out, d);
                }
                out.push(8);
                cx = x;
                cy = y;
            }
        }
    }
    out.push(14);
    out
}

fn number(o: &mut Vec<u8>, v: i64) {
    if (-107..=107).contains(&v) {
        o.push((v + 139) as u8);
    } else if (108..=1131).contains(&v) {
        let a = v - 108;
        o.push((247 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-1131..=-108).contains(&v) {
        let a = -v - 108;
        o.push((251 + a / 256) as u8);
        o.push((a % 256) as u8);
    } else if (-32768..=32767).contains(&v) {
        o.push(28);
        o.extend_from_slice(&(v as i16).to_be_bytes());
    } else {
        o.push(255);
        let fixed = ((v as f64) * 65536.0).round() as i64;
        o.extend_from_slice(&(fixed as i32).to_be_bytes());
    }
}

/// `Compile::Cff.index_bytes`.
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut offsets = vec![1usize];
    for it in items {
        offsets.push(offsets[offsets.len() - 1] + it.len());
    }
    let max = offsets[offsets.len() - 1];
    let off_size: usize = match max {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFF_FFFF => 3,
        _ => 4,
    };
    let mut o = Vec::new();
    o.extend_from_slice(&(items.len() as u16).to_be_bytes());
    o.push(off_size as u8);
    for v in offsets {
        o.extend_from_slice(&(v as u32).to_be_bytes()[4 - off_size..]);
    }
    for it in items {
        o.extend_from_slice(it);
    }
    o
}

/// `Compile::Cff.encode_int` (including its wrap-around `pack("C")` for
/// negative two-byte values, which the offsets never reach).
fn encode_int(o: &mut Vec<u8>, v: i64) {
    if (-107..=107).contains(&v) {
        o.push((v + 139) as u8);
    } else if (108..=1131).contains(&v) {
        let a = v - 108;
        o.push((a / 256 + 247) as u8);
        o.push((a % 256) as u8);
    } else if (-1131..=-108).contains(&v) {
        let a = -v - 108;
        o.push((-(a / 256) - 247) as u8);
        o.push((a % 256) as u8);
    } else if (-32768..=32767).contains(&v) {
        o.push(28);
        o.extend_from_slice(&(v as i16).to_be_bytes());
    } else {
        o.push(29);
        o.extend_from_slice(&(v as i32).to_be_bytes());
    }
}

fn top_dict(
    charset: usize,
    charstrings: usize,
    private_size: usize,
    private_off: usize,
) -> Vec<u8> {
    let mut o = Vec::new();
    encode_int(&mut o, charset as i64);
    o.push(0x0f);
    encode_int(&mut o, charstrings as i64);
    o.push(0x11);
    encode_int(&mut o, private_size as i64);
    encode_int(&mut o, private_off as i64);
    o.push(0x12);
    o
}

/// `Compile::Cff.compute_layout` + `assemble`.
fn build(name: &[u8], charstrings: &[Vec<u8>], glyphs: usize) -> Vec<u8> {
    let name_index = index(&[name.to_vec()]);
    let string_index = index(&[]);
    let global_index = index(&[]);
    let mut charset = Vec::new();
    if glyphs > 1 {
        charset.push(0);
        charset.resize(1 + 2 * (glyphs - 1), 0);
    }
    let cs_index = index(charstrings);
    let private: Vec<u8> = Vec::new();

    let mut top_index = index(&[top_dict(0, 0, 0, 0)]);
    for _ in 0..10 {
        let post_top =
            4 + name_index.len() + top_index.len() + string_index.len() + global_index.len();
        let charset_off = post_top;
        let cs_off = charset_off + charset.len();
        let private_off = cs_off + cs_index.len();
        let new_index = index(&[top_dict(charset_off, cs_off, private.len(), private_off)]);
        let converged = new_index.len() == top_index.len();
        top_index = new_index;
        if converged {
            break;
        }
    }

    let mut out = vec![1, 0, 4, 1];
    for part in [
        &name_index,
        &top_index,
        &string_index,
        &global_index,
        &charset,
        &cs_index,
        &private,
    ] {
        out.extend_from_slice(part);
    }
    out
}
