//! Port of `Variation::Instancing::GvarReader`.

use super::bytes::*;

pub(crate) struct Tuple {
    pub peak: Vec<f64>,
    pub start: Option<Vec<f64>>,
    pub end: Option<Vec<f64>>,
    pub points: Option<Vec<usize>>,
    pub dx: Vec<i32>,
    pub dy: Vec<i32>,
}

pub(crate) struct Gvar<'a> {
    data: &'a [u8],
    axis_count: usize,
    glyph_count: usize,
    long: bool,
    array_offset: usize,
    shared: Vec<Vec<f64>>,
}

fn f2dot14(d: &[u8], pos: usize, n: usize) -> R<Vec<f64>> {
    (0..n)
        .map(|i| i16_(d, pos + i * 2).map(|v| f64::from(v) / 16384.0))
        .collect()
}

impl<'a> Gvar<'a> {
    pub fn new(data: &'a [u8]) -> R<Self> {
        let axis_count = usize::from(u16_(data, 4)?);
        let shared_count = usize::from(u16_(data, 6)?);
        let shared_offset = u32_(data, 8)? as usize;
        let shared = (0..shared_count)
            .map(|i| f2dot14(data, shared_offset + i * axis_count * 2, axis_count))
            .collect::<R<Vec<_>>>()?;
        Ok(Self {
            data,
            axis_count,
            glyph_count: usize::from(u16_(data, 12)?),
            long: u16_(data, 14)? & 1 != 0,
            array_offset: u32_(data, 16)? as usize,
            shared,
        })
    }

    fn glyph_range(&self, gid: usize) -> R<(usize, usize)> {
        if self.long {
            Ok((
                u32_(self.data, 20 + gid * 4)? as usize,
                u32_(self.data, 24 + gid * 4)? as usize,
            ))
        } else {
            Ok((
                usize::from(u16_(self.data, 20 + gid * 2)?) * 2,
                usize::from(u16_(self.data, 22 + gid * 2)?) * 2,
            ))
        }
    }

    pub fn tuples_for(&self, gid: usize, point_count: usize) -> R<Vec<Tuple>> {
        if gid >= self.glyph_count {
            return Ok(Vec::new());
        }
        let (first, last) = self.glyph_range(gid)?;
        if last <= first {
            return Ok(Vec::new());
        }
        // Ruby slices `@data[start, len]`, which clamps a length that runs past the
        // table. A start beyond the table is nil there (a NoMethodError crash), so
        // that stays a typed error here.
        let start = self.array_offset.checked_add(first).ok_or_else(bad)?;
        let end = start.saturating_add(last - first).min(self.data.len());
        let g = self.data.get(start..end).ok_or_else(bad)?;
        self.parse_glyph(g, point_count)
    }

    fn parse_glyph(&self, g: &[u8], point_count: usize) -> R<Vec<Tuple>> {
        let ac = self.axis_count;
        let count_field = u16_(g, 0)?;
        let data_offset = usize::from(u16_(g, 2)?);
        let count = usize::from(count_field & 0x0FFF);
        struct Header {
            size: usize,
            index: u16,
            peak: Vec<f64>,
            start: Option<Vec<f64>>,
            end: Option<Vec<f64>>,
        }
        let mut headers = Vec::with_capacity(count);
        let mut pos = 4;
        for _ in 0..count {
            let size = usize::from(u16_(g, pos)?);
            let index = u16_(g, pos + 2)?;
            pos += 4;
            let peak = if index & 0x8000 != 0 {
                let p = f2dot14(g, pos, ac)?;
                pos += ac * 2;
                p
            } else {
                self.shared
                    .get(usize::from(index & 0x0FFF))
                    .ok_or_else(bad)?
                    .clone()
            };
            let (mut start, mut end) = (None, None);
            if index & 0x4000 != 0 {
                start = Some(f2dot14(g, pos, ac)?);
                end = Some(f2dot14(g, pos + ac * 2, ac)?);
                pos += ac * 4;
            }
            headers.push(Header {
                size,
                index,
                peak,
                start,
                end,
            });
        }
        let mut dpos = data_offset;
        let mut shared_points = None;
        if count_field & 0x8000 != 0 {
            let (p, n) = read_points(g, dpos)?;
            shared_points = p;
            dpos = n;
        }
        let mut out = Vec::with_capacity(count);
        for h in headers {
            let tuple_end = dpos + h.size;
            let mut points = shared_points.clone();
            if h.index & 0x2000 != 0 {
                let (p, n) = read_points(g, dpos)?;
                points = p;
                dpos = n;
            }
            let n = points.as_ref().map_or(point_count, Vec::len);
            let (dx, p2) = read_deltas(g, dpos, n)?;
            let (dy, _) = read_deltas(g, p2, n)?;
            dpos = tuple_end;
            out.push(Tuple {
                peak: h.peak,
                start: h.start,
                end: h.end,
                points,
                dx,
                dy,
            });
        }
        Ok(out)
    }
}

fn read_points(d: &[u8], mut pos: usize) -> R<(Option<Vec<usize>>, usize)> {
    let mut count = usize::from(u8_(d, pos)?);
    pos += 1;
    if count & 0x80 != 0 {
        count = ((count & 0x7F) << 8) | usize::from(u8_(d, pos)?);
        pos += 1;
    }
    if count == 0 {
        return Ok((None, pos));
    }
    let mut points = Vec::new();
    let mut last = 0usize;
    while points.len() < count {
        let ctl = u8_(d, pos)?;
        pos += 1;
        let run = usize::from(ctl & 0x7F) + 1;
        for i in 0..run {
            let v = if ctl & 0x80 != 0 {
                usize::from(u16_(d, pos + i * 2)?)
            } else {
                usize::from(u8_(d, pos + i)?)
            };
            last += v;
            points.push(last);
        }
        pos += if ctl & 0x80 != 0 { run * 2 } else { run };
    }
    Ok((Some(points), pos))
}

fn read_deltas(d: &[u8], mut pos: usize, count: usize) -> R<(Vec<i32>, usize)> {
    let mut out: Vec<i32> = Vec::new();
    while out.len() < count {
        let ctl = u8_(d, pos)?;
        pos += 1;
        let run = usize::from(ctl & 0x3F) + 1;
        if ctl & 0x80 != 0 {
            out.extend(std::iter::repeat_n(0, run));
        } else if ctl & 0x40 != 0 {
            for i in 0..run {
                out.push(i32::from(i16_(d, pos + i * 2)?));
            }
            pos += run * 2;
        } else {
            for i in 0..run {
                out.push(i32::from(u8_(d, pos + i)? as i8));
            }
            pos += run;
        }
    }
    out.truncate(count);
    Ok((out, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_range_past_the_table_is_clamped_like_ruby_slicing() {
        // 1 axis, no shared tuples, 1 glyph, short offsets, data array at 24. The
        // glyph claims 64 bytes but the table ends after 4 (tuple count 0).
        let mut d = vec![0u8; 20];
        d[5] = 1;
        d[13] = 1;
        d[19] = 24;
        d.extend([0, 0, 0, 32]);
        d.extend([0, 0, 0, 4]);
        let g = Gvar::new(&d).unwrap();
        assert!(g.tuples_for(0, 4).unwrap().is_empty());
    }

    #[test]
    fn glyph_range_starting_past_the_table_is_an_error() {
        // Ruby's nil slice is a NoMethodError crash; here it stays a typed error.
        let mut d = vec![0u8; 20];
        d[5] = 1;
        d[13] = 1;
        d[19] = 100;
        d.extend([0, 0, 0, 32]);
        let g = Gvar::new(&d).unwrap();
        assert!(g.tuples_for(0, 4).is_err());
    }
}
