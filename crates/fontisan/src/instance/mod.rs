//! Static instancing of gvar variable fonts (card 1i-instance): a port of
//! Ruby's `Variation::Instancing::TrueTypeInstancer` and the
//! `Variable::Instancer#instance` wrapper, float arithmetic and all.
//!
//! Like the Ruby reference this applies gvar (with IUP) to glyf, HVAR (or the
//! phantom points) to hmtx, rewrites OS/2 wght/wdth and the name table, and
//! drops fvar/gvar/avar/HVAR/VVAR/MVAR/STAT/cvar/DSIG. It does NOT apply MVAR,
//! cvar or GPOS/GDEF variations and leaves their (now dangling) store
//! references alone, which is why Ruby's outputs fail OTS.

mod axis;
mod bytes;
mod gvar;
mod hvar;
mod name;
mod outline;

use crate::convert::{ConvertError, Tables, tables_of, write_font};
use crate::sfnt::{self, Format};
use axis::{AxisSpace, named_instances, tag_str};
use bytes::*;
use gvar::{Gvar, Tuple};
use hvar::Hvar;
use name::NameRewriter;
use outline::Outline;

const DROPPED: [&[u8; 4]; 9] = [
    b"fvar", b"gvar", b"avar", b"HVAR", b"VVAR", b"MVAR", b"STAT", b"cvar", b"DSIG",
];
const WIDTH_CLASSES: [f64; 9] = [50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 150.0, 200.0];

const MISSING_CLASS: &str = "Fontisan::MissingVariationTableError";
const NO_GVAR: &str = "Static instancing needs TrueType outlines with gvar (CFF2 variable fonts are not supported yet)";

/// `Variable::Instancer#instance`: the static sfnt for the user location
/// `coords` (axis tag, value; axes left out take their default).
pub fn instance(input: &[u8], coords: &[([u8; 4], f64)]) -> Result<Vec<u8>, ConvertError> {
    let decoded: Vec<u8>;
    let raw: &[u8] = match sfnt::detect_format(input) {
        Some(Format::Ttf | Format::Otf) => input,
        Some(Format::Woff) => {
            decoded = crate::woff::decode_woff(input)
                .map_err(|e| ConvertError::new(crate::convert::ERROR_CLASS, e.to_string()))?;
            &decoded
        }
        Some(Format::Woff2) => {
            decoded = crate::woff::decode_woff2_ruby_padding(input)
                .map_err(|e| ConvertError::new(crate::convert::ERROR_CLASS, e.to_string()))?;
            &decoded
        }
        _ => {
            return Err(ConvertError::new(
                crate::convert::ERROR_CLASS,
                "unrecognised font format",
            ));
        }
    };
    let (_, tables) = tables_of(raw)?;
    if !tables.iter().any(|(t, d)| t == b"fvar" && !d.is_empty()) {
        return Err(ConvertError::new(
            "ArgumentError",
            "Font is not a variable font (missing fvar table)",
        ));
    }
    Ok(write_font(&instance_tables(&tables, coords)?, 0x0001_0000))
}

/// A variable font with neither gvar nor CFF2 is `:static` to Ruby's
/// `FormatDetector#variation_type`: `resolve_static_font` hands every table
/// through, fvar/avar/HVAR/MVAR/STAT/DSIG included. (The name is historical.)
pub(crate) fn drop_variation_tables(tables: Tables) -> Tables {
    tables
}

fn get<'a>(t: &'a Tables, tag: &[u8; 4]) -> Option<&'a Vec<u8>> {
    t.iter().find(|(k, _)| k == tag).map(|(_, v)| v)
}

fn put(t: &mut Tables, tag: &[u8; 4], data: Vec<u8>) {
    match t.iter_mut().find(|(k, _)| k == tag) {
        Some(slot) => slot.1 = data,
        None => t.push((*tag, data)),
    }
}

struct Glyph {
    outline: Outline,
    points: Vec<(i64, i64)>,
    advance: i64,
    bbox: Option<[i64; 4]>,
}

fn round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// `TrueTypeInstancer#call` on a table map (directory order kept).
pub(crate) fn instance_tables(
    tables: &Tables,
    coords: &[([u8; 4], f64)],
) -> Result<Tables, ConvertError> {
    let Some(fvar) = get(tables, b"fvar") else {
        return Err(ConvertError::new(
            MISSING_CLASS,
            "Missing required variation table: fvar",
        ));
    };
    let (Some(gvar_data), Some(glyf)) = (get(tables, b"gvar"), get(tables, b"glyf")) else {
        return Err(ConvertError::new(MISSING_CLASS, NO_GVAR));
    };
    // `check_font!` runs before the axis space is built.
    for tag in [b"head", b"hhea", b"hmtx", b"maxp", b"loca"] {
        if get(tables, tag).is_none() {
            return Err(ConvertError::new(
                "Fontisan::MissingTableError",
                format!(
                    "Font missing required {} table",
                    String::from_utf8_lossy(tag)
                ),
            ));
        }
    }
    let space = AxisSpace::new(fvar, get(tables, b"avar").map(Vec::as_slice))?;
    let location = space.validate(coords)?;
    let norm = space.normalize(&location);

    let need = |t: &[u8; 4]| get(tables, t).ok_or_else(bad);
    let maxp = need(b"maxp")?;
    let hhea = need(b"hhea")?;
    let hmtx = need(b"hmtx")?;
    let head = need(b"head")?;
    let loca = need(b"loca")?;
    let num_glyphs = usize::from(u16_(maxp, 4)?);

    // read_metrics
    let nhm = usize::from(u16_(hhea, 34)?);
    let mut advances: Vec<i64> = Vec::with_capacity(num_glyphs);
    let mut lsbs: Vec<i64> = Vec::with_capacity(num_glyphs);
    for gid in 0..num_glyphs {
        if gid < nhm {
            advances.push(i64::from(u16_(hmtx, gid * 4)?));
            lsbs.push(i64::from(i16_(hmtx, gid * 4 + 2)?));
        } else {
            advances.push(*advances.last().ok_or_else(bad)?);
            lsbs.push(i64::from(i16_(hmtx, nhm * 4 + (gid - nhm) * 2)?));
        }
    }
    let long = i16_(head, 50)? == 1;
    let loca_vals: Vec<usize> = if long {
        loca.chunks_exact(4)
            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize)
            .collect()
    } else {
        loca.chunks_exact(2)
            .map(|c| usize::from(u16::from_be_bytes([c[0], c[1]])) * 2)
            .collect()
    };
    let gvar = Gvar::new(gvar_data)?;
    let hvar = get(tables, b"HVAR").map(|h| Hvar::new(h)).transpose()?;

    let raw_glyph = |gid: usize| -> &[u8] {
        let start = loca_vals.get(gid).copied().unwrap_or(0);
        let end = loca_vals.get(gid + 1).copied().unwrap_or(glyf.len());
        if start > glyf.len() || end <= start {
            return &[];
        }
        &glyf[start..end.min(glyf.len())]
    };

    let mut glyphs: Vec<Glyph> = Vec::with_capacity(num_glyphs);
    for gid in 0..num_glyphs {
        let rg = raw_glyph(gid);
        let outline = Outline::parse(rg)?;
        let orig = outline.points.clone();
        let n = orig.len();
        let xmin = if outline.is_empty() {
            0
        } else {
            i64::from(i16_(rg, 2)?)
        };
        let pp1x = xmin - lsbs[gid];
        let mut total = vec![[0.0f64; 2]; n + 4];
        for t in gvar.tuples_for(gid, n + 4)? {
            let s = tuple_scalar(&t, &norm);
            if s == 0.0 {
                continue;
            }
            let (dx, dy) = expand_deltas(&t, &outline, &orig);
            if dx.len() < n + 4 || dy.len() < n + 4 {
                return Err(bad());
            }
            for (i, tot) in total.iter_mut().enumerate() {
                tot[0] += s * dx[i];
                tot[1] += s * dy[i];
            }
        }
        let pts: Vec<(i64, i64)> = orig
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| (round(x + total[i][0]), round(y + total[i][1])))
            .collect();
        let advance = match &hvar {
            Some(h) => advances[gid] + round(h.advance_delta(gid, &norm)?),
            None => {
                round((pp1x + advances[gid]) as f64 + total[n + 1][0])
                    - round(pp1x as f64 + total[n][0])
            }
        };
        glyphs.push(Glyph {
            outline,
            points: pts,
            advance: advance.max(0),
            bbox: None,
        });
    }
    resolve_bounds(&mut glyphs)?;

    let mut out: Tables = tables
        .iter()
        .filter(|(t, _)| !DROPPED.contains(&t))
        .cloned()
        .collect();
    write_glyf_loca(&mut out, &glyphs)?;
    let lsb_of = |gid: usize, g: &Glyph| -> i64 {
        if g.outline.is_empty() {
            lsbs[gid]
        } else {
            g.bbox.map_or(0, |b| b[0])
        }
    };
    write_hmtx(&mut out, &glyphs, &lsb_of)?;
    update_head_hhea(&mut out, &glyphs, &lsb_of)?;
    update_os2(&mut out, &space, &location);
    update_name(&mut out, fvar, &space, &location)?;
    Ok(out)
}

fn tuple_scalar(t: &Tuple, norm: &[f64]) -> f64 {
    let mut scalar = 1.0f64;
    for (i, &peak) in t.peak.iter().enumerate() {
        if peak == 0.0 {
            continue;
        }
        let v = norm.get(i).copied().unwrap_or(0.0);
        if v == peak {
            continue;
        }
        let (lo, hi) = match (&t.start, &t.end) {
            (Some(s), Some(e)) => (s[i], e[i]),
            _ => (0.0f64.min(peak), 0.0f64.max(peak)),
        };
        if lo > peak || peak > hi || (lo < 0.0 && hi > 0.0) {
            continue;
        }
        if v <= lo || hi <= v {
            return 0.0;
        }
        scalar *= if v < peak {
            (v - lo) / (peak - lo)
        } else {
            (hi - v) / (hi - peak)
        };
    }
    scalar
}

/// Per-point delta arrays of length n + 4, IUP applied when only some points
/// were given (never to composites or phantom points).
fn expand_deltas(t: &Tuple, outline: &Outline, orig: &[(f64, f64)]) -> (Vec<f64>, Vec<f64>) {
    let Some(points) = &t.points else {
        return (
            t.dx.iter().map(|&v| f64::from(v)).collect(),
            t.dy.iter().map(|&v| f64::from(v)).collect(),
        );
    };
    let total = orig.len() + 4;
    let mut dx: Vec<Option<f64>> = vec![None; total];
    let mut dy: Vec<Option<f64>> = vec![None; total];
    for (i, &p) in points.iter().enumerate() {
        if p >= total {
            continue;
        }
        dx[p] = Some(f64::from(t.dx[i]));
        dy[p] = Some(f64::from(t.dy[i]));
    }
    if !outline.is_composite() {
        let xs: Vec<f64> = orig.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = orig.iter().map(|p| p.1).collect();
        iup(&xs, &mut dx, &outline.end_pts);
        iup(&ys, &mut dy, &outline.end_pts);
    }
    (
        dx.into_iter().map(|v| v.unwrap_or(0.0)).collect(),
        dy.into_iter().map(|v| v.unwrap_or(0.0)).collect(),
    )
}

fn iup(coords: &[f64], deltas: &mut [Option<f64>], ends: &[usize]) {
    let mut start = 0usize;
    for &last in ends {
        let touched: Vec<usize> = (start..=last).filter(|&i| deltas[i].is_some()).collect();
        if touched.is_empty() {
            (start..=last).for_each(|i| deltas[i] = Some(0.0));
        } else if touched.len() == 1 {
            let d = deltas[touched[0]];
            (start..=last).for_each(|i| deltas[i] = d);
        } else {
            for (k, &t) in touched.iter().enumerate() {
                let nxt = touched[(k + 1) % touched.len()];
                let mut i = t;
                loop {
                    i = if i == last { start } else { i + 1 };
                    if i == nxt {
                        break;
                    }
                    deltas[i] = Some(iup_value(
                        coords[i],
                        coords[t],
                        deltas[t].unwrap_or(0.0),
                        coords[nxt],
                        deltas[nxt].unwrap_or(0.0),
                    ));
                }
            }
        }
        start = last + 1;
    }
}

fn iup_value(coord: f64, mut ref1: f64, mut d1: f64, mut ref2: f64, mut d2: f64) -> f64 {
    if ref1 == ref2 {
        return if d1 == d2 { d1 } else { 0.0 };
    }
    if ref1 > ref2 {
        std::mem::swap(&mut ref1, &mut ref2);
        std::mem::swap(&mut d1, &mut d2);
    }
    if coord <= ref1 {
        return d1;
    }
    if coord >= ref2 {
        return d2;
    }
    d1 + ((coord - ref1) * (d2 - d1) / (ref2 - ref1))
}

fn resolve_bounds(glyphs: &mut [Glyph]) -> Result<(), ConvertError> {
    for gid in 0..glyphs.len() {
        bbox_for(glyphs, gid, &mut Vec::new())?;
    }
    Ok(())
}

fn bbox_for(glyphs: &mut [Glyph], gid: usize, stack: &mut Vec<usize>) -> R<[i64; 4]> {
    if let Some(b) = glyphs[gid].bbox {
        return Ok(b);
    }
    if glyphs[gid].outline.is_empty() || stack.contains(&gid) {
        glyphs[gid].bbox = Some([0; 4]);
        return Ok([0; 4]);
    }
    let bbox = if glyphs[gid].outline.is_composite() {
        let comps: Vec<(usize, Vec<u8>)> = glyphs[gid]
            .outline
            .components
            .iter()
            .map(|c| (usize::from(c.gid), c.transform.clone()))
            .collect();
        let mut corners: Vec<(f64, f64)> = Vec::new();
        stack.push(gid);
        for (i, (cg, transform)) in comps.iter().enumerate() {
            if *cg >= glyphs.len() {
                return Err(bad());
            }
            let sub = bbox_for(glyphs, *cg, stack)?;
            if glyphs[*cg].outline.is_empty() {
                continue;
            }
            let off = glyphs[gid].points[i];
            corners.extend(transform_corners(sub, transform, off));
        }
        stack.pop();
        if corners.is_empty() {
            [0; 4]
        } else {
            let xs = corners.iter().map(|c| c.0);
            let ys = corners.iter().map(|c| c.1);
            let min = |it: &mut dyn Iterator<Item = f64>| it.fold(f64::INFINITY, f64::min);
            let max = |it: &mut dyn Iterator<Item = f64>| it.fold(f64::NEG_INFINITY, f64::max);
            [
                min(&mut xs.clone()).round() as i64,
                min(&mut ys.clone()).round() as i64,
                max(&mut xs.clone()).round() as i64,
                max(&mut ys.clone()).round() as i64,
            ]
        }
    } else {
        let p = &glyphs[gid].points;
        if p.is_empty() {
            [0; 4]
        } else {
            [
                p.iter().map(|q| q.0).min().unwrap_or(0),
                p.iter().map(|q| q.1).min().unwrap_or(0),
                p.iter().map(|q| q.0).max().unwrap_or(0),
                p.iter().map(|q| q.1).max().unwrap_or(0),
            ]
        }
    };
    glyphs[gid].bbox = Some(bbox);
    Ok(bbox)
}

fn transform_corners(bbox: [i64; 4], t: &[u8], off: (i64, i64)) -> Vec<(f64, f64)> {
    let f = |i: usize| f64::from(i16::from_be_bytes([t[i * 2], t[i * 2 + 1]])) / 16384.0;
    let (a, b, c, d) = match t.len() {
        2 => (f(0), 0.0, 0.0, f(0)),
        4 => (f(0), 0.0, 0.0, f(1)),
        8 => (f(0), f(1), f(2), f(3)),
        _ => (1.0, 0.0, 0.0, 1.0),
    };
    let [xmin, ymin, xmax, ymax] = bbox.map(|v| v as f64);
    [(xmin, ymin), (xmin, ymax), (xmax, ymin), (xmax, ymax)]
        .iter()
        .map(|&(x, y)| (a * x + c * y + off.0 as f64, b * x + d * y + off.1 as f64))
        .collect()
}

fn write_glyf_loca(out: &mut Tables, glyphs: &[Glyph]) -> R<()> {
    let mut glyf = Vec::new();
    let mut loca: Vec<u32> = vec![0];
    for g in glyphs {
        let mut data = g.outline.serialize(&g.points, g.bbox.ok_or_else(bad)?);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        glyf.extend_from_slice(&data);
        loca.push(glyf.len() as u32);
    }
    put(out, b"glyf", glyf);
    put(
        out,
        b"loca",
        loca.iter().flat_map(|v| v.to_be_bytes()).collect(),
    );
    let mut head = get(out, b"head").ok_or_else(bad)?.clone();
    set(&mut head, 50, &1i16.to_be_bytes())?;
    put(out, b"head", head);
    Ok(())
}

fn set(buf: &mut [u8], at: usize, v: &[u8]) -> R<()> {
    buf.get_mut(at..at + v.len())
        .ok_or_else(bad)?
        .copy_from_slice(v);
    Ok(())
}

fn write_hmtx(out: &mut Tables, glyphs: &[Glyph], lsb_of: &dyn Fn(usize, &Glyph) -> i64) -> R<()> {
    let mut hmtx = Vec::with_capacity(glyphs.len() * 4);
    for (gid, g) in glyphs.iter().enumerate() {
        hmtx.extend_from_slice(&(g.advance as u16).to_be_bytes());
        hmtx.extend_from_slice(&(lsb_of(gid, g) as i16).to_be_bytes());
    }
    put(out, b"hmtx", hmtx);
    let mut hhea = get(out, b"hhea").ok_or_else(bad)?.clone();
    set(&mut hhea, 34, &(glyphs.len() as u16).to_be_bytes())?;
    put(out, b"hhea", hhea);
    Ok(())
}

fn update_head_hhea(
    out: &mut Tables,
    glyphs: &[Glyph],
    lsb_of: &dyn Fn(usize, &Glyph) -> i64,
) -> R<()> {
    let drawn: Vec<(usize, &Glyph)> = glyphs
        .iter()
        .enumerate()
        .filter(|(_, g)| !g.outline.is_empty())
        .collect();
    if drawn.is_empty() {
        return Ok(());
    }
    let bb = |g: &Glyph| g.bbox.unwrap_or([0; 4]);
    let mut head = get(out, b"head").ok_or_else(bad)?.clone();
    let vals = [
        drawn.iter().map(|(_, g)| bb(g)[0]).min().unwrap_or(0),
        drawn.iter().map(|(_, g)| bb(g)[1]).min().unwrap_or(0),
        drawn.iter().map(|(_, g)| bb(g)[2]).max().unwrap_or(0),
        drawn.iter().map(|(_, g)| bb(g)[3]).max().unwrap_or(0),
    ];
    for (i, v) in vals.iter().enumerate() {
        set(&mut head, 36 + i * 2, &(*v as i16).to_be_bytes())?;
    }
    put(out, b"head", head);
    let mut hhea = get(out, b"hhea").ok_or_else(bad)?.clone();
    let width = |g: &Glyph| bb(g)[2] - bb(g)[0];
    let min_lsb = drawn.iter().map(|(i, g)| lsb_of(*i, g)).min().unwrap_or(0);
    let min_rsb = drawn
        .iter()
        .map(|(i, g)| g.advance - lsb_of(*i, g) - width(g))
        .min()
        .unwrap_or(0);
    let max_ext = drawn
        .iter()
        .map(|(i, g)| lsb_of(*i, g) + width(g))
        .max()
        .unwrap_or(0);
    let max_adv = glyphs.iter().map(|g| g.advance).max().unwrap_or(0);
    set(&mut hhea, 10, &(max_adv as u16).to_be_bytes())?;
    set(&mut hhea, 12, &(min_lsb as i16).to_be_bytes())?;
    set(&mut hhea, 14, &(min_rsb as i16).to_be_bytes())?;
    set(&mut hhea, 16, &(max_ext as i16).to_be_bytes())?;
    put(out, b"hhea", hhea);
    Ok(())
}

fn update_os2(out: &mut Tables, space: &AxisSpace, location: &[f64]) {
    let Some(mut os2) = get(out, b"OS/2").cloned() else {
        return;
    };
    let at = |tag: &[u8; 4]| {
        space
            .axes
            .iter()
            .position(|a| &a.tag == tag)
            .map(|i| location[i])
    };
    if let Some(w) = at(b"wght") {
        let v = w.round().clamp(1.0, 1000.0) as u16;
        let _ = set(&mut os2, 4, &v.to_be_bytes());
    }
    if let Some(w) = at(b"wdth") {
        let mut best = (f64::INFINITY, 0usize);
        for (i, c) in WIDTH_CLASSES.iter().enumerate() {
            let d = (c - w).abs();
            if d < best.0 {
                best = (d, i);
            }
        }
        let _ = set(&mut os2, 6, &((best.1 + 1) as u16).to_be_bytes());
    }
    put(out, b"OS/2", os2);
}

fn update_name(out: &mut Tables, fvar: &[u8], space: &AxisSpace, location: &[f64]) -> R<()> {
    let Some(data) = get(out, b"name").cloned() else {
        return Ok(());
    };
    let mut rw = NameRewriter::new(&data)?;
    let label = instance_label(&rw, fvar, space, location)?;
    rw.rename(&label);
    put(out, b"name", rw.to_binary());
    Ok(())
}

fn instance_label(
    rw: &NameRewriter,
    fvar: &[u8],
    space: &AxisSpace,
    location: &[f64],
) -> R<String> {
    let named = named_instances(fvar)?
        .into_iter()
        .find(|(_, vals)| (0..space.axes.len()).all(|i| (vals[i] - location[i]).abs() < 1e-6))
        .and_then(|(id, _)| rw.lookup(id).map(str::to_string));
    if let Some(n) = named {
        return Ok(n);
    }
    let parts: Vec<String> = space
        .axes
        .iter()
        .zip(location)
        .filter(|(a, v)| **v != a.default)
        .map(|(a, v)| format!("{}{}", capitalize(&tag_str(&a.tag)), format_value(*v)))
        .collect();
    Ok(if parts.is_empty() {
        "Regular".to_string()
    } else {
        parts.join(" ")
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn format_value(v: f64) -> String {
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        rb_float(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(tags: &[&[u8; 4]]) -> Tables {
        tags.iter().map(|t| (**t, vec![0u8; 8])).collect()
    }

    #[test]
    fn dropped_required_table_is_named_before_axis_parsing() {
        // fvar is garbage: Ruby's check_font! reports the missing table first.
        for gone in ["head", "hhea", "hmtx", "maxp", "loca"] {
            let all = [
                b"fvar", b"gvar", b"glyf", b"head", b"hhea", b"hmtx", b"maxp", b"loca",
            ];
            let kept: Vec<&[u8; 4]> = all
                .iter()
                .copied()
                .filter(|t| &t[..gone.len()] != gone.as_bytes())
                .collect();
            let e = instance_tables(&tables(&kept), &[(*b"wght", 500.0)]).expect_err("fails");
            assert_eq!(e.class, "Fontisan::MissingTableError", "{gone}");
            assert_eq!(e.message, format!("Font missing required {gone} table"));
        }
    }

    #[test]
    fn variable_font_without_gvar_keeps_every_table() {
        let all = tables(&[
            b"fvar", b"avar", b"HVAR", b"MVAR", b"STAT", b"DSIG", b"glyf",
        ]);
        assert_eq!(drop_variation_tables(all.clone()), all);
    }
}
