//! Port of `Variation::Instancing::GlyphOutline`.

use super::bytes::*;

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME: u8 = 0x10;
const Y_SAME: u8 = 0x20;

const ARGS_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY: u16 = 0x0002;
const HAVE_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const HAVE_XY_SCALE: u16 = 0x0040;
const HAVE_TWO_BY_TWO: u16 = 0x0080;

pub(crate) struct Component {
    pub flags: u16,
    pub gid: u16,
    arg1: i32,
    arg2: i32,
    pub transform: Vec<u8>,
}

pub(crate) struct Outline {
    header_contours: i16,
    pub end_pts: Vec<usize>,
    on_curve: Vec<bool>,
    overlap: bool,
    instructions: Vec<u8>,
    pub components: Vec<Component>,
    trailer: Vec<u8>,
    pub points: Vec<(f64, f64)>,
}

impl Outline {
    pub fn parse(bytes: &[u8]) -> R<Self> {
        let mut o = Self {
            header_contours: 0,
            end_pts: Vec::new(),
            on_curve: Vec::new(),
            overlap: false,
            instructions: Vec::new(),
            components: Vec::new(),
            trailer: Vec::new(),
            points: Vec::new(),
        };
        if bytes.is_empty() {
            return Ok(o);
        }
        o.header_contours = i16_(bytes, 0)?;
        if o.header_contours >= 0 {
            o.parse_simple(bytes)?;
        } else {
            o.parse_composite(bytes)?;
        }
        Ok(o)
    }

    pub fn is_empty(&self) -> bool {
        self.header_contours == 0 && self.points.is_empty()
    }

    pub fn is_composite(&self) -> bool {
        self.header_contours < 0
    }

    fn parse_simple(&mut self, b: &[u8]) -> R<()> {
        let n = self.header_contours as usize;
        self.end_pts = (0..n)
            .map(|i| u16_(b, 10 + i * 2).map(usize::from))
            .collect::<R<_>>()?;
        let mut pos = 10 + n * 2;
        let ilen = usize::from(u16_(b, pos)?);
        self.instructions = sl(b, pos + 2, ilen)?.to_vec();
        pos += 2 + ilen;
        let count = if n == 0 { 0 } else { self.end_pts[n - 1] + 1 };
        let mut flags: Vec<u8> = Vec::new();
        while flags.len() < count {
            let f = u8_(b, pos)?;
            pos += 1;
            flags.push(f);
            if f & REPEAT != 0 {
                let rep = u8_(b, pos)?;
                flags.extend(std::iter::repeat_n(f, usize::from(rep)));
                pos += 1;
            }
        }
        let (xs, p) = read_coords(b, pos, &flags, X_SHORT, X_SAME)?;
        let (ys, _) = read_coords(b, p, &flags, Y_SHORT, Y_SAME)?;
        self.on_curve = flags.iter().map(|f| f & ON_CURVE != 0).collect();
        self.overlap = flags.first().is_some_and(|f| f & 0x40 != 0);
        self.points = xs
            .into_iter()
            .zip(ys)
            .map(|(x, y)| (x as f64, y as f64))
            .collect();
        Ok(())
    }

    fn parse_composite(&mut self, b: &[u8]) -> R<()> {
        let mut pos = 10;
        loop {
            let flags = u16_(b, pos)?;
            let gid = u16_(b, pos + 2)?;
            pos += 4;
            let (a1, a2);
            if flags & ARGS_ARE_WORDS != 0 {
                if flags & ARGS_ARE_XY != 0 {
                    a1 = i32::from(i16_(b, pos)?);
                    a2 = i32::from(i16_(b, pos + 2)?);
                } else {
                    a1 = i32::from(u16_(b, pos)?);
                    a2 = i32::from(u16_(b, pos + 2)?);
                }
                pos += 4;
            } else {
                let (x, y) = (u8_(b, pos)?, u8_(b, pos + 1)?);
                if flags & ARGS_ARE_XY != 0 {
                    a1 = i32::from(x as i8);
                    a2 = i32::from(y as i8);
                } else {
                    a1 = i32::from(x);
                    a2 = i32::from(y);
                }
                pos += 2;
            }
            let tlen = if flags & HAVE_SCALE != 0 {
                2
            } else if flags & HAVE_XY_SCALE != 0 {
                4
            } else if flags & HAVE_TWO_BY_TWO != 0 {
                8
            } else {
                0
            };
            let transform = sl(b, pos, tlen)?.to_vec();
            self.components.push(Component {
                flags,
                gid,
                arg1: a1,
                arg2: a2,
                transform,
            });
            pos += tlen;
            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        self.trailer = b.get(pos..).unwrap_or_default().to_vec();
        self.points = self
            .components
            .iter()
            .map(|c| {
                if c.flags & ARGS_ARE_XY != 0 {
                    (f64::from(c.arg1), f64::from(c.arg2))
                } else {
                    (0.0, 0.0)
                }
            })
            .collect();
        Ok(())
    }

    /// `serialize(new_points, bbox)`; points are already rounded.
    pub fn serialize(&self, pts: &[(i64, i64)], bbox: [i64; 4]) -> Vec<u8> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.header_contours.to_be_bytes());
        for v in bbox {
            out.extend_from_slice(&(v as i16).to_be_bytes());
        }
        if self.is_composite() {
            self.serialize_composite(&mut out, pts);
        } else {
            self.serialize_simple(&mut out, pts);
        }
        out
    }

    fn serialize_simple(&self, out: &mut Vec<u8>, pts: &[(i64, i64)]) {
        for e in &self.end_pts {
            out.extend_from_slice(&(*e as u16).to_be_bytes());
        }
        out.extend_from_slice(&(self.instructions.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.instructions);
        let (mut flags, mut xs, mut ys) = (Vec::new(), Vec::new(), Vec::new());
        let (mut px, mut py) = (0i64, 0i64);
        for (i, &(x, y)) in pts.iter().enumerate() {
            let mut f = if self.on_curve.get(i).copied().unwrap_or(false) {
                ON_CURVE
            } else {
                0
            };
            if i == 0 && self.overlap {
                f |= 0x40;
            }
            let (dx, dy) = (x - px, y - py);
            if dx == 0 {
                f |= X_SAME;
            } else {
                xs.extend_from_slice(&(dx as i16).to_be_bytes());
            }
            if dy == 0 {
                f |= Y_SAME;
            } else {
                ys.extend_from_slice(&(dy as i16).to_be_bytes());
            }
            flags.push(f);
            px = x;
            py = y;
        }
        out.extend_from_slice(&flags);
        out.extend_from_slice(&xs);
        out.extend_from_slice(&ys);
    }

    fn serialize_composite(&self, out: &mut Vec<u8>, pts: &[(i64, i64)]) {
        for (i, c) in self.components.iter().enumerate() {
            let mut flags = c.flags;
            if flags & ARGS_ARE_XY != 0 {
                let (a1, a2) = pts[i];
                flags |= ARGS_ARE_WORDS;
                out.extend_from_slice(&flags.to_be_bytes());
                out.extend_from_slice(&c.gid.to_be_bytes());
                out.extend_from_slice(&(a1 as i16).to_be_bytes());
                out.extend_from_slice(&(a2 as i16).to_be_bytes());
            } else if flags & ARGS_ARE_WORDS != 0 {
                out.extend_from_slice(&flags.to_be_bytes());
                out.extend_from_slice(&c.gid.to_be_bytes());
                out.extend_from_slice(&(c.arg1 as u16).to_be_bytes());
                out.extend_from_slice(&(c.arg2 as u16).to_be_bytes());
            } else {
                out.extend_from_slice(&flags.to_be_bytes());
                out.extend_from_slice(&c.gid.to_be_bytes());
                out.push(c.arg1 as u8);
                out.push(c.arg2 as u8);
            }
            out.extend_from_slice(&c.transform);
        }
        out.extend_from_slice(&self.trailer);
    }
}

fn read_coords(
    b: &[u8],
    mut pos: usize,
    flags: &[u8],
    short: u8,
    same: u8,
) -> R<(Vec<i64>, usize)> {
    let mut val = 0i64;
    let mut out = Vec::with_capacity(flags.len());
    for &f in flags {
        if f & short != 0 {
            let d = i64::from(u8_(b, pos)?);
            pos += 1;
            val += if f & same != 0 { d } else { -d };
        } else if f & same == 0 {
            val += i64::from(i16_(b, pos)?);
            pos += 2;
        }
        out.push(val);
    }
    Ok((out, pos))
}
