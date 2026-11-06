//! Ruby-faithful glyf reader and outline model (`Tables::Glyf`, `SimpleGlyph`,
//! `CompoundGlyph`, `CompoundGlyphResolver`, `Models::Outline`). Quirks kept:
//! a simple glyph always reads an instruction length (so a 10-byte glyph
//! fails), component args are always offsets, empty components are skipped.

use std::collections::BTreeSet;

type R<T> = Result<T, String>;

/// Outline drawing command (coordinates are f64 like Ruby Integer/Float mix).
#[derive(Debug, Clone, Copy)]
pub(super) enum Cmd {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
}

struct Simple {
    num_contours: usize,
    ends: Vec<usize>,
    flags: Vec<u8>,
    xs: Vec<i32>,
    ys: Vec<i32>,
}

struct Comp {
    flags: u16,
    index: u16,
    matrix: [f64; 6],
}

enum Glyph {
    Simple(Simple),
    Compound(Vec<Comp>),
}

fn s16(v: u16) -> i32 {
    i32::from(v as i16)
}

fn parse_simple(data: &[u8], id: usize) -> R<Simple> {
    let mut r = Rd { d: data, p: 10 };
    let nc = s16(u16::from_be_bytes([data[0], data[1]])) as usize;
    let mut ends = Vec::new();
    if nc > 0 {
        let b = r.take(nc * 2).ok_or(format!(
            "Insufficient contour end data for simple glyph {id}"
        ))?;
        ends = b
            .chunks(2)
            .map(|c| usize::from(u16::from_be_bytes([c[0], c[1]])))
            .collect();
    }
    let il = r.u16().ok_or(format!(
        "Insufficient instruction length data for simple glyph {id}"
    ))?;
    if il > 0 {
        r.take(usize::from(il)).ok_or(format!(
            "Insufficient instruction data for simple glyph {id}"
        ))?;
    }
    if nc == 0 {
        return Ok(Simple {
            num_contours: 0,
            ends,
            flags: vec![],
            xs: vec![],
            ys: vec![],
        });
    }
    let total = ends[nc - 1] + 1;
    let mut flags: Vec<u8> = Vec::new();
    while flags.len() < total {
        let f = r
            .take(1)
            .ok_or(format!("Insufficient flag data for simple glyph {id}"))?[0];
        flags.push(f);
        if f & 8 != 0 {
            let c = r
                .take(1)
                .ok_or(format!("Missing repeat count for simple glyph {id}"))?[0];
            for _ in 0..c {
                flags.push(f);
            }
        }
    }
    if flags.len() != total {
        return Err(format!(
            "Flag count mismatch for simple glyph {id}: expected {total}, got {}",
            flags.len()
        ));
    }
    let mut coords = |short: u8, same: u8, axis: &str| -> R<Vec<i32>> {
        let mut out = Vec::with_capacity(total);
        let mut cur = 0i32;
        for (i, &f) in flags.iter().enumerate() {
            let miss = || {
                format!("Insufficient {axis} coordinate data for simple glyph {id} at point {i}")
            };
            let delta = if f & short != 0 {
                let v = i32::from(r.take(1).ok_or_else(miss)?[0]);
                if f & same == 0 { -v } else { v }
            } else if f & same != 0 {
                0
            } else {
                let b = r.take(2).ok_or_else(miss)?;
                s16(u16::from_be_bytes([b[0], b[1]]))
            };
            cur = cur.wrapping_add(delta);
            out.push(cur);
        }
        Ok(out)
    };
    let xs = coords(0x02, 0x10, "x")?;
    let ys = coords(0x04, 0x20, "y")?;
    if ends.windows(2).any(|w| w[1] <= w[0]) {
        return Err(format!(
            "Invalid contour end points for simple glyph {id}: not monotonically increasing"
        ));
    }
    Ok(Simple {
        num_contours: nc,
        ends,
        flags,
        xs,
        ys,
    })
}

fn parse_compound(data: &[u8], id: usize) -> R<Vec<Comp>> {
    let mut r = Rd { d: data, p: 10 };
    let mut comps = Vec::new();
    loop {
        let h = r.take(4).ok_or(format!(
            "Insufficient component header data for compound glyph {id}"
        ))?;
        let flags = u16::from_be_bytes([h[0], h[1]]);
        let index = u16::from_be_bytes([h[2], h[3]]);
        let arg_err = format!("Insufficient argument data for compound glyph {id}");
        let (a1, a2) = if flags & 1 == 0 {
            let b = r.take(2).ok_or(arg_err)?;
            (f64::from(b[0] as i8), f64::from(b[1] as i8))
        } else {
            let b = r.take(4).ok_or(arg_err)?;
            (
                f64::from(s16(u16::from_be_bytes([b[0], b[1]]))),
                f64::from(s16(u16::from_be_bytes([b[2], b[3]]))),
            )
        };
        let f2 =
            |b: &[u8], i: usize| f64::from(s16(u16::from_be_bytes([b[i], b[i + 1]]))) / 16384.0;
        let matrix = if flags & 0x80 != 0 {
            let b = r.take(8).ok_or(format!(
                "Insufficient 2x2 matrix data for compound glyph {id}"
            ))?;
            // Ruby: [xscale, yscale, scale01, scale10] = [v0, v3, v1, v2]
            [f2(b, 0), f2(b, 2), f2(b, 4), f2(b, 6), a1, a2]
        } else if flags & 0x40 != 0 {
            let b = r.take(4).ok_or(format!(
                "Insufficient x,y scale data for compound glyph {id}"
            ))?;
            [f2(b, 0), 0.0, 0.0, f2(b, 2), a1, a2]
        } else if flags & 0x08 != 0 {
            let b = r
                .take(2)
                .ok_or(format!("Insufficient scale data for compound glyph {id}"))?;
            [f2(b, 0), 0.0, 0.0, f2(b, 0), a1, a2]
        } else {
            [1.0, 0.0, 0.0, 1.0, a1, a2]
        };
        comps.push(Comp {
            flags,
            index,
            matrix,
        });
        if flags & 0x20 == 0 {
            break;
        }
    }
    if comps.iter().any(|c| c.flags & 0x100 != 0) {
        let il = r.u16().ok_or(format!(
            "Insufficient instruction length data for compound glyph {id}"
        ))?;
        if il > 0 {
            r.take(usize::from(il)).ok_or(format!(
                "Insufficient instruction data for compound glyph {id}"
            ))?;
        }
    }
    if comps.iter().filter(|c| c.flags & 0x200 != 0).count() > 1 {
        return Err(format!(
            "Compound glyph {id} has multiple components with USE_MY_METRICS flag"
        ));
    }
    if comps.iter().any(|c| usize::from(c.index) == id) {
        return Err(format!(
            "Circular reference: compound glyph {id} references itself"
        ));
    }
    Ok(comps)
}

/// glyf + parsed loca (Ruby `Loca#parse_with_context`).
pub(super) struct GlyfTable<'a> {
    glyf: &'a [u8],
    offsets: Vec<usize>,
}

impl<'a> GlyfTable<'a> {
    pub(super) fn new(glyf: &'a [u8], loca: &[u8], loc_format: i32, num_glyphs: usize) -> R<Self> {
        if loc_format != 0 && loc_format != 1 {
            return Err(format!(
                "indexToLocFormat must be 0 (short) or 1 (long), got: {loc_format}"
            ));
        }
        if num_glyphs < 1 {
            return Err("numGlyphs must be >= 1, got: 0".into());
        }
        let mut offsets = Vec::with_capacity(num_glyphs + 1);
        for i in 0..=num_glyphs {
            if loc_format == 0 {
                let b = loca
                    .get(i * 2..i * 2 + 2)
                    .ok_or(format!("Insufficient data for short offset at index {i}"))?;
                offsets.push(usize::from(u16::from_be_bytes([b[0], b[1]])) * 2);
            } else {
                let b = loca
                    .get(i * 4..i * 4 + 4)
                    .ok_or(format!("Insufficient data for long offset at index {i}"))?;
                offsets.push(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            }
        }
        for (i, w) in offsets.windows(2).enumerate() {
            if w[1] < w[0] {
                return Err(format!(
                    "Offsets are not monotonically increasing: offset[{i}]={}, offset[{}]={}",
                    w[0],
                    i + 1,
                    w[1]
                ));
            }
        }
        Ok(Self { glyf, offsets })
    }

    fn num_glyphs(&self) -> usize {
        self.offsets.len() - 1
    }

    /// `Glyf#glyph_for`: None for a zero-size glyph.
    fn glyph(&self, id: usize) -> R<Option<Glyph>> {
        if id >= self.num_glyphs() {
            return Err(format!(
                "glyph_id {id} exceeds number of glyphs ({})",
                self.num_glyphs()
            ));
        }
        let (off, size) = (self.offsets[id], self.offsets[id + 1] - self.offsets[id]);
        if size == 0 {
            return Ok(None);
        }
        if off + size > self.glyf.len() {
            return Err(format!(
                "Glyph {id} extends beyond glyf table: offset={off}, size={size}, table_size={}",
                self.glyf.len()
            ));
        }
        let data = &self.glyf[off..off + size];
        if data.len() < 10 {
            return Err(format!(
                "Insufficient glyph data for glyph {id}: need at least 10 bytes, got {}",
                data.len()
            ));
        }
        let nc = s16(u16::from_be_bytes([data[0], data[1]]));
        if nc >= 0 {
            Ok(Some(Glyph::Simple(parse_simple(data, id)?)))
        } else if nc == -1 {
            Ok(Some(Glyph::Compound(parse_compound(data, id)?)))
        } else {
            Err(format!(
                "Invalid numberOfContours for glyph {id}: {nc}. Must be >= 0 for simple glyphs or -1 for compound glyphs."
            ))
        }
    }

    /// `OutlineExtraction#extract_ttf_outlines` for one glyph.
    pub(super) fn outline(&self, id: usize) -> R<Vec<Cmd>> {
        match self.glyph(id)? {
            None => Ok(vec![]),
            Some(Glyph::Simple(s)) if s.num_contours == 0 => Ok(vec![]),
            Some(Glyph::Simple(s)) => Ok(simple_commands(&s)),
            Some(Glyph::Compound(c)) => self.resolve(id, &c, &BTreeSet::new(), 0),
        }
    }

    fn resolve(
        &self,
        id: usize,
        comps: &[Comp],
        visited: &BTreeSet<usize>,
        depth: usize,
    ) -> R<Vec<Cmd>> {
        if depth > 32 {
            return Err(format!(
                "Maximum recursion depth (32) exceeded resolving compound glyph {id}"
            ));
        }
        if visited.contains(&id) {
            return Err(format!(
                "Circular reference detected in compound glyph {id}"
            ));
        }
        let mut visited = visited.clone();
        visited.insert(id);
        let mut out = Vec::new();
        for c in comps {
            let sub = match self.glyph(usize::from(c.index))? {
                None => continue,
                Some(Glyph::Simple(s)) if s.num_contours == 0 => continue,
                Some(Glyph::Simple(s)) => simple_commands(&s),
                Some(Glyph::Compound(cc)) => {
                    self.resolve(usize::from(c.index), &cc, &visited, depth + 1)?
                }
            };
            let [a, b, cc, d, e, f] = c.matrix;
            let tx = |x: f64, y: f64| a * x + cc * y + e;
            let ty = |x: f64, y: f64| b * x + d * y + f;
            for cmd in sub {
                out.push(match cmd {
                    Cmd::Move(x, y) => Cmd::Move(tx(x, y), ty(x, y)),
                    Cmd::Line(x, y) => Cmd::Line(tx(x, y), ty(x, y)),
                    Cmd::Quad(cx, cy, x, y) => {
                        Cmd::Quad(tx(cx, cy), ty(cx, cy), tx(x, y), ty(x, y))
                    }
                });
            }
        }
        Ok(out)
    }
}

/// `Outline.from_truetype` (close_path commands are dropped: CFF ignores them).
fn simple_commands(s: &Simple) -> Vec<Cmd> {
    let mut out = Vec::new();
    for ci in 0..s.num_contours {
        let start = if ci == 0 { 0 } else { s.ends[ci - 1] + 1 };
        let end = s.ends[ci];
        let pts: Vec<(f64, f64, bool)> = (start..=end)
            .map(|i| (f64::from(s.xs[i]), f64::from(s.ys[i]), s.flags[i] & 1 != 0))
            .collect();
        let first = pts[0];
        out.push(Cmd::Move(first.0, first.1));
        let mut i = 1;
        while i < pts.len() {
            let p = pts[i];
            if p.2 {
                out.push(Cmd::Line(p.0, p.1));
                i += 1;
            } else {
                i += 1;
                if i < pts.len() && !pts[i].2 {
                    let n = pts[i];
                    out.push(Cmd::Quad(p.0, p.1, (p.0 + n.0) / 2.0, (p.1 + n.1) / 2.0));
                } else if i < pts.len() {
                    let e = pts[i];
                    out.push(Cmd::Quad(p.0, p.1, e.0, e.1));
                    i += 1;
                } else {
                    out.push(Cmd::Quad(p.0, p.1, first.0, first.1));
                }
            }
        }
    }
    out
}
