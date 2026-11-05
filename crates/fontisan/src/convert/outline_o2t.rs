//! OpenType(CFF) to TrueType outline conversion (card 1h-o2t): a port of
//! Ruby's `OutlineConverter#convert_otf_to_ttf` with `Tables::Cff::CharString`,
//! `Models::Outline` and `Tables::GlyphBuilder`, quirks included (see
//! `charstring.rs`). The cubic is approximated by ONE quadratic whose control
//! point is the mean of the two cubic controls.

mod cff;
mod charstring;

use super::{ConvertError, Tables};
use charstring::{Interp, Seg};

#[derive(Clone, Copy)]
enum Cmd {
    Move(i64, i64),
    Line(i64, i64),
    Curve([i64; 6]),
    Close,
}

type Pt = (i64, i64, bool);

fn r(v: f64) -> Result<i64, String> {
    if v.is_finite() {
        Ok(v.round() as i64) // half away from zero, like Ruby's Float#round
    } else {
        Err("Infinity".into())
    }
}

/// `Outline.convert_cff_path_to_commands`.
fn commands(path: &[Seg]) -> Result<Vec<Cmd>, String> {
    let mut out: Vec<Cmd> = vec![];
    let mut start: Option<(i64, i64)> = None;
    let close_if_back = |out: &mut Vec<Cmd>, start: Option<(i64, i64)>| {
        let Some(s) = start else { return };
        let last = match out.last() {
            Some(Cmd::Line(x, y)) => (*x, *y),
            Some(Cmd::Curve(c)) => (c[4], c[5]),
            _ => return,
        };
        if (last.0 - s.0).abs() <= 1 && (last.1 - s.1).abs() <= 1 {
            out.push(Cmd::Close);
        }
    };
    for seg in path {
        match *seg {
            Seg::Move(x, y) => {
                close_if_back(&mut out, start);
                let p = (r(x)?, r(y)?);
                start = Some(p);
                out.push(Cmd::Move(p.0, p.1));
            }
            Seg::Line(x, y) => out.push(Cmd::Line(r(x)?, r(y)?)),
            Seg::Curve(c) => {
                let mut v = [0i64; 6];
                for (d, s) in v.iter_mut().zip(c) {
                    *d = r(s)?;
                }
                out.push(Cmd::Curve(v));
            }
        }
    }
    close_if_back(&mut out, start);
    Ok(out)
}

/// `Outline#to_truetype_contours`.
fn contours(cmds: &[Cmd]) -> Vec<Vec<Pt>> {
    let mut all = vec![];
    let mut cur: Vec<Pt> = vec![];
    for c in cmds {
        match *c {
            Cmd::Move(x, y) => {
                if !cur.is_empty() {
                    all.push(std::mem::take(&mut cur));
                }
                cur.push((x, y, true));
            }
            Cmd::Line(x, y) => cur.push((x, y, true)),
            Cmd::Curve(c) => {
                let q = |a: i64, b: i64| ((a + b) as f64 / 2.0).round() as i64;
                cur.push((q(c[0], c[2]), q(c[1], c[3]), false));
                cur.push((c[4], c[5], true));
            }
            Cmd::Close => {
                if !cur.is_empty() {
                    all.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        all.push(cur);
    }
    all
}

fn put16(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&(v as u16).to_be_bytes()); // Ruby pack("n") wraps
}

/// `GlyphBuilder.build_simple_glyph` (no instructions).
fn simple_glyph(contours: &[Vec<Pt>]) -> Result<Vec<u8>, String> {
    let pts: Vec<Pt> = contours.iter().flatten().copied().collect();
    let on: Vec<&Pt> = pts.iter().filter(|p| p.2).collect();
    if on.is_empty() {
        return Err("Infinity".into());
    }
    let min = |f: fn(&Pt) -> i64| on.iter().map(|p| f(p)).min().unwrap();
    let max = |f: fn(&Pt) -> i64| on.iter().map(|p| f(p)).max().unwrap();
    let mut d = Vec::new();
    put16(&mut d, contours.len() as i64);
    for v in [min(|p| p.0), min(|p| p.1), max(|p| p.0), max(|p| p.1)] {
        put16(&mut d, v);
    }
    let mut total = 0i64;
    for c in contours {
        total += c.len() as i64;
        put16(&mut d, total - 1);
    }
    put16(&mut d, 0); // instruction length

    let (mut flags, mut xs, mut ys) = (vec![], vec![], vec![]);
    let (mut px, mut py) = (0i64, 0i64);
    let axis = |delta: i64, short: u8, same: u8, out: &mut Vec<u8>| -> u8 {
        if delta == 0 {
            same
        } else if (-255..=255).contains(&delta) {
            out.push(delta.unsigned_abs() as u8);
            short | if delta > 0 { same } else { 0 }
        } else {
            put16(out, delta);
            0
        }
    };
    for &(x, y, on_curve) in &pts {
        let mut f = u8::from(on_curve);
        f |= axis(x - px, 0x02, 0x10, &mut xs);
        f |= axis(y - py, 0x04, 0x20, &mut ys);
        flags.push(f);
        (px, py) = (x, y);
    }
    let mut i = 0;
    while i < flags.len() {
        let mut count = 1;
        while i + count < flags.len() && flags[i + count] == flags[i] && count < 256 {
            count += 1;
        }
        if count > 1 {
            d.push(flags[i] | 0x08);
            d.push((count - 1) as u8);
        } else {
            d.push(flags[i]);
        }
        i += count;
    }
    d.extend_from_slice(&xs);
    d.extend_from_slice(&ys);
    Ok(d)
}

fn glyph_error(e: &str, gid: usize) -> ConvertError {
    ConvertError::transform(format!(
        "Failed to get CharString for glyph {gid}: Failed to parse CharString: {e}"
    ))
}

/// `Tables` of a CFF font with `glyf`, `loca`, `maxp`, `head` rebuilt and
/// `CFF `/`CFF2` dropped (position of the other tables unchanged).
pub(crate) fn convert(tables: &Tables) -> Result<Tables, ConvertError> {
    let table = |t: &[u8; 4]| {
        tables
            .iter()
            .find(|(k, _)| k == t)
            .map(|(_, d)| d.as_slice())
    };
    let cff_data = table(b"CFF ").ok_or_else(|| ConvertError::transform("CFF table not found"))?;
    let corrupt = |e: String| ConvertError::transform(format!("Failed to parse CFF table: {e}"));
    let cff = cff::parse(cff_data).map_err(corrupt)?;
    let n = cff.charstrings.len();

    let mut glyf: Vec<u8> = vec![];
    let mut offsets = Vec::with_capacity(n + 1);
    let (mut max_points, mut max_contours) = (0usize, 0usize);
    for (gid, cs) in cff.charstrings.iter().enumerate() {
        offsets.push(glyf.len());
        if !cff.has_private {
            continue; // Ruby: charstring_for_glyph is nil, the glyph is empty
        }
        let mut it = Interp::new(&cff.global_subrs, cff.local_subrs.as_deref());
        it.run_main(cs).map_err(|e| glyph_error(&e, gid))?;
        if it.path.is_empty() {
            continue;
        }
        let cmds = commands(&it.path).map_err(|e| glyph_error(&e, gid))?;
        let cs = contours(&cmds);
        max_contours = max_contours.max(cs.len());
        max_points = max_points.max(cs.iter().map(Vec::len).max().unwrap_or(0));
        glyf.extend(simple_glyph(&cs).map_err(|e| glyph_error(&e, gid))?);
        glyf.resize(glyf.len().next_multiple_of(4), 0);
    }
    offsets.push(glyf.len());

    let long = *offsets.iter().max().unwrap() > 0x1FFFE;
    let loca: Vec<u8> = if long {
        offsets
            .iter()
            .flat_map(|&o| (o as u32).to_be_bytes())
            .collect()
    } else {
        offsets
            .iter()
            .flat_map(|&o| ((o / 2) as u16).to_be_bytes())
            .collect()
    };

    let mut maxp = Vec::with_capacity(32);
    maxp.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for v in [n, max_points, max_contours, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        maxp.extend_from_slice(&(v as u16).to_be_bytes());
    }

    let mut out: Tables = Vec::with_capacity(tables.len() + 2);
    for (tag, data) in tables {
        match tag {
            b"CFF " | b"CFF2" => {}
            b"maxp" => out.push((*tag, maxp.clone())),
            b"head" => {
                let mut h = data.clone();
                if h.len() < 52 {
                    h.resize(52, 0);
                }
                h[50..52].copy_from_slice(&u16::from(long).to_be_bytes());
                out.push((*tag, h));
            }
            b"glyf" | b"loca" => {}
            _ => out.push((*tag, data.clone())),
        }
    }
    if !out.iter().any(|(t, _)| t == b"maxp") {
        out.push((*b"maxp", maxp));
    }
    out.push((*b"glyf", glyf));
    out.push((*b"loca", loca));
    Ok(out)
}
