//! The WOFF2 `glyf`/`loca` transform (W3C WOFF2 section 5.1), ported from Ruby's
//! `Woff2::GlyfCanonicalizer`, `Woff2::GlyfLocaTransform`, `TripletCodec` and `UInt255`.
//!
//! Where Ruby raises (a truncated glyph, a negative point count), this returns an error.

use super::Error;

const FLAG_ON_CURVE: u8 = 0x01;
const FLAG_OVERLAP_SIMPLE: u8 = 0x40;
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
const SIMPLE_HEADER_SIZE: usize = 10;

fn bad<T>(msg: &str) -> Result<T, Error> {
    Err(Error::new(format!("glyf transform: {msg}")))
}

/// Canonical (every glyph padded to 4 bytes) glyf and its loca in `target_format`.
pub(super) struct Canonical {
    pub glyf: Vec<u8>,
    pub loca: Vec<u8>,
}

/// Loca offsets, in bytes. Short format: `u16 * 2` (a trailing odd byte is ignored,
/// as Ruby's `unpack("n*")` does).
pub(super) fn parse_loca(loca: &[u8], long: bool) -> Vec<usize> {
    if long {
        loca.chunks_exact(4)
            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize)
            .collect()
    } else {
        loca.chunks_exact(2)
            .map(|c| usize::from(u16::from_be_bytes([c[0], c[1]])) * 2)
            .collect()
    }
}

pub(super) fn canonicalize(
    glyf: &[u8],
    loca: &[u8],
    num_glyphs: usize,
    source_long: bool,
    target_long: bool,
) -> Result<Canonical, Error> {
    let src = parse_loca(loca, source_long);
    if src.len() != num_glyphs + 1 {
        return bad(&format!(
            "loca has {} entries, expected {}",
            src.len(),
            num_glyphs + 1
        ));
    }
    let mut out = Vec::with_capacity(glyf.len() + 3 * num_glyphs);
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    offsets.push(0usize);
    for i in 0..num_glyphs {
        let (start, end) = (src[i], src[i + 1]);
        if start < end {
            let s = start.min(glyf.len());
            let e = end.min(glyf.len());
            out.extend_from_slice(&glyf[s..e]);
        }
        while out.len() % 4 != 0 {
            out.push(0);
        }
        offsets.push(out.len());
    }
    let mut loca_out = Vec::with_capacity(offsets.len() * 4);
    for o in offsets {
        if target_long {
            loca_out.extend_from_slice(&(o as u32).to_be_bytes());
        } else {
            loca_out.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    }
    Ok(Canonical {
        glyf: out,
        loca: loca_out,
    })
}

fn uint255(out: &mut Vec<u8>, v: usize) -> Result<(), Error> {
    match v {
        0..=252 => out.push(v as u8),
        253..=505 => out.extend_from_slice(&[255, (v - 253) as u8]),
        506..=761 => out.extend_from_slice(&[254, (v - 506) as u8]),
        762..=65535 => {
            out.push(253);
            out.extend_from_slice(&(v as u16).to_be_bytes());
        }
        _ => return bad("255UInt16 value out of range"),
    }
    Ok(())
}

/// One point's `(flag, payload)` per the WOFF2 triplet encoding.
fn triplet(dx: i32, dy: i32, on_curve: bool, glyph: &mut Vec<u8>, flags: &mut Vec<u8>) {
    let on_bit: i32 = if on_curve { 0 } else { 0x80 };
    let (ax, ay) = (dx.abs(), dy.abs());
    let xs = i32::from(dx >= 0);
    let ys = i32::from(dy >= 0);
    let xy = xs + 2 * ys;
    let flag;
    if dx == 0 && ay < 1280 {
        flag = on_bit + ((ay & 0xF00) >> 7) + ys;
        glyph.push((ay & 0xFF) as u8);
    } else if dy == 0 && ax < 1280 {
        flag = on_bit + 10 + ((ax & 0xF00) >> 7) + xs;
        glyph.push((ax & 0xFF) as u8);
    } else if ax < 65 && ay < 65 {
        flag = on_bit + 20 + ((ax - 1) & 0x30) + (((ay - 1) & 0x30) >> 2) + xy;
        glyph.push(((((ax - 1) & 0x0F) << 4) | ((ay - 1) & 0x0F)) as u8);
    } else if ax < 769 && ay < 769 {
        flag = on_bit + 84 + 12 * (((ax - 1) & 0x300) >> 8) + (((ay - 1) & 0x300) >> 6) + xy;
        glyph.push(((ax - 1) & 0xFF) as u8);
        glyph.push(((ay - 1) & 0xFF) as u8);
    } else if ax < 4096 && ay < 4096 {
        flag = on_bit + 120 + xy;
        glyph.push(((ax >> 4) & 0xFF) as u8);
        glyph.push((((ax & 0x0F) << 4) | ((ay >> 8) & 0x0F)) as u8);
        glyph.push((ay & 0xFF) as u8);
    } else {
        flag = on_bit + 124 + xy;
        glyph.push(((ax >> 8) & 0xFF) as u8);
        glyph.push((ax & 0xFF) as u8);
        glyph.push(((ay >> 8) & 0xFF) as u8);
        glyph.push((ay & 0xFF) as u8);
    }
    flags.push(flag as u8);
}

/// Cursor over one glyph's bytes; reads past the end are errors (Ruby: `nil` crash).
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.pos + n > self.b.len() {
            return bad("truncated glyph");
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> Result<i16, Error> {
        Ok(self.u16()? as i16)
    }
}

fn i16_at(b: &[u8], off: usize) -> Result<i16, Error> {
    match b.get(off..off + 2) {
        Some(s) => Ok(i16::from_be_bytes([s[0], s[1]])),
        None => bad("truncated glyph header"),
    }
}

#[derive(Default)]
struct Streams {
    n_contour: Vec<u8>,
    n_points: Vec<u8>,
    flags: Vec<u8>,
    glyph: Vec<u8>,
    composite: Vec<u8>,
    bbox_data: Vec<u8>,
    instructions: Vec<u8>,
    bbox_bitmap: Vec<u8>,
    overlap_bitmap: Vec<u8>,
}

impl Streams {
    fn set_bbox_bit(&mut self, id: usize) {
        self.bbox_bitmap[id >> 3] |= 0x80 >> (id & 7);
    }
}

/// The transformed `glyf` table for a canonical `glyf`/`loca` pair.
pub(super) fn transform(
    glyf: &[u8],
    loca: &[u8],
    num_glyphs: usize,
    index_long: bool,
) -> Result<Vec<u8>, Error> {
    let offsets = parse_loca(loca, index_long);
    if offsets.len() < num_glyphs + 1 {
        return bad("loca shorter than numGlyphs + 1");
    }
    let bitmap_len = ((num_glyphs + 31) >> 5) << 2;
    let mut s = Streams {
        bbox_bitmap: vec![0; bitmap_len],
        overlap_bitmap: vec![0; (num_glyphs + 7) >> 3],
        ..Streams::default()
    };
    for id in 0..num_glyphs {
        let (start, end) = (offsets[id], offsets[id + 1]);
        let bytes: &[u8] = if start < end && start < glyf.len() {
            &glyf[start..end.min(glyf.len())]
        } else {
            &[]
        };
        if bytes.is_empty() {
            s.n_contour.extend_from_slice(&0i16.to_be_bytes());
            continue;
        }
        let n = i16_at(bytes, 0)?;
        let bbox = [
            i16_at(bytes, 2)?,
            i16_at(bytes, 4)?,
            i16_at(bytes, 6)?,
            i16_at(bytes, 8)?,
        ];
        match n {
            0 => s.n_contour.extend_from_slice(&0i16.to_be_bytes()),
            -1 => composite(&mut s, bytes, bbox, id)?,
            n if n > 0 => simple(&mut s, bytes, n as usize, bbox, id)?,
            _ => return bad("negative contour count other than -1"),
        }
    }
    let has_overlap = s.overlap_bitmap.iter().any(|&b| b != 0);
    let mut bbox_wire = s.bbox_bitmap.clone();
    bbox_wire.extend_from_slice(&s.bbox_data);

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&u16::from(has_overlap).to_be_bytes()); // optionFlags
    out.extend_from_slice(&(num_glyphs as u16).to_be_bytes());
    out.extend_from_slice(&u16::from(index_long).to_be_bytes());
    for len in [
        s.n_contour.len(),
        s.n_points.len(),
        s.flags.len(),
        s.glyph.len(),
        s.composite.len(),
        bbox_wire.len(),
        s.instructions.len(),
    ] {
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(&s.n_contour);
    out.extend_from_slice(&s.n_points);
    out.extend_from_slice(&s.flags);
    out.extend_from_slice(&s.glyph);
    out.extend_from_slice(&s.composite);
    out.extend_from_slice(&bbox_wire);
    out.extend_from_slice(&s.instructions);
    if has_overlap {
        out.extend_from_slice(&s.overlap_bitmap);
    }
    Ok(out)
}

fn simple(
    s: &mut Streams,
    bytes: &[u8],
    n_contours: usize,
    bbox: [i16; 4],
    id: usize,
) -> Result<(), Error> {
    s.n_contour
        .extend_from_slice(&(n_contours as i16).to_be_bytes());
    let mut c = Cur {
        b: bytes,
        pos: SIMPLE_HEADER_SIZE,
    };
    let mut end_pts = Vec::with_capacity(n_contours);
    for _ in 0..n_contours {
        end_pts.push(usize::from(c.u16()?));
    }
    let total = end_pts[n_contours - 1] + 1;
    let mut prev: i64 = -1;
    for &ep in &end_pts {
        let d = ep as i64 - prev;
        if d < 0 {
            return bad("end points not ascending");
        }
        uint255(&mut s.n_points, d as usize)?;
        prev = ep as i64;
    }
    let inst_len = usize::from(c.u16()?);
    // Ruby's `io.read(n)` returns what is left when the glyph is short.
    let avail = bytes.len().saturating_sub(c.pos).min(inst_len);
    let instructions = c.take(avail)?;

    let mut tt_flags: Vec<u8> = Vec::with_capacity(total);
    while tt_flags.len() < total {
        let f = c.u8()?;
        tt_flags.push(f);
        if f & 0x08 != 0 {
            let rep = c.u8()?;
            for _ in 0..rep {
                tt_flags.push(f);
            }
        }
    }
    let xs = coords(&mut c, &tt_flags, 0x02, 0x10)?;
    let ys = coords(&mut c, &tt_flags, 0x04, 0x20)?;

    let (mut px, mut py) = (0i32, 0i32);
    for i in 0..total {
        let (dx, dy) = (xs[i] - px, ys[i] - py);
        px = xs[i];
        py = ys[i];
        triplet(
            dx,
            dy,
            tt_flags[i] & FLAG_ON_CURVE != 0,
            &mut s.glyph,
            &mut s.flags,
        );
    }
    if tt_flags[0] & FLAG_OVERLAP_SIMPLE != 0 {
        s.overlap_bitmap[id >> 3] |= 0x80 >> (id & 7);
    }
    uint255(&mut s.glyph, instructions.len())?;
    s.instructions.extend_from_slice(instructions);

    let calc = [
        *xs.iter().min().unwrap_or(&0),
        *ys.iter().min().unwrap_or(&0),
        *xs.iter().max().unwrap_or(&0),
        *ys.iter().max().unwrap_or(&0),
    ];
    let same = calc
        .iter()
        .zip(bbox.iter())
        .all(|(a, b)| *a == i32::from(*b));
    if !same {
        s.set_bbox_bit(id);
        for v in bbox {
            s.bbox_data.extend_from_slice(&v.to_be_bytes());
        }
    }
    Ok(())
}

fn coords(c: &mut Cur, flags: &[u8], short: u8, same: u8) -> Result<Vec<i32>, Error> {
    let mut v = 0i32;
    let mut out = Vec::with_capacity(flags.len());
    for &f in flags {
        let delta = if f & short != 0 {
            let d = i32::from(c.u8()?);
            if f & same == 0 { -d } else { d }
        } else if f & same != 0 {
            0
        } else {
            i32::from(c.i16()?)
        };
        v += delta;
        out.push(v);
    }
    Ok(out)
}

fn composite(s: &mut Streams, bytes: &[u8], bbox: [i16; 4], id: usize) -> Result<(), Error> {
    s.n_contour.extend_from_slice(&(-1i16).to_be_bytes());
    let mut c = Cur {
        b: bytes,
        pos: SIMPLE_HEADER_SIZE,
    };
    let mut have_instructions;
    loop {
        let flags = c.u16()?;
        c.take(2)?; // glyphIndex
        c.take(if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            4
        } else {
            2
        })?;
        c.take(if flags & WE_HAVE_A_SCALE != 0 {
            2
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            4
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            8
        } else {
            0
        })?;
        have_instructions = flags & WE_HAVE_INSTRUCTIONS != 0;
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    s.composite
        .extend_from_slice(&bytes[SIMPLE_HEADER_SIZE..c.pos]);
    if have_instructions {
        let len = usize::from(c.u16()?);
        let avail = bytes.len().saturating_sub(c.pos).min(len);
        let inst = c.take(avail)?;
        uint255(&mut s.glyph, inst.len())?;
        s.instructions.extend_from_slice(inst);
    }
    s.set_bbox_bit(id);
    for v in bbox {
        s.bbox_data.extend_from_slice(&v.to_be_bytes());
    }
    Ok(())
}
