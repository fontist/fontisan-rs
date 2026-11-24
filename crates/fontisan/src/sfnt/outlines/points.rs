//! TrueType glyph outline points (card: "glyf point decoding"): simple-glyph
//! decoding and composite resolution, yielding each contour's points in font
//! units (y-up).
//!
//! Composites resolve depth-first with the standard component offsets and
//! transforms; point-matching components refuse typed. Quadratic off-curve
//! points are carried as-is — implied on-curve midpoints are the consumer's
//! choice (SVG, raster and hinting differ in where they want them).

use super::super::cursor::Cursor;
use super::super::error::{Error, ErrorClass};

/// One decoded glyph outline: contours of points in font units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphOutline {
    pub contours: Vec<Contour>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contour {
    pub points: Vec<Point>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
    /// TrueType on-curve flag. An off-curve point implies the standard
    /// quadratic midpoint with its neighbours.
    pub on_curve: bool,
}

const FLAG_ON_CURVE: u8 = 0x01;
const FLAG_X_SHORT: u8 = 0x02;
const FLAG_Y_SHORT: u8 = 0x04;
const FLAG_REPEAT: u8 = 0x08;
const FLAG_X_SAME_OR_POSITIVE: u8 = 0x10;
const FLAG_Y_SAME_OR_POSITIVE: u8 = 0x20;

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;

/// The composite nesting cap. Real fonts stay flat; the cap only turns a
/// cyclic or pathological component graph into a refusal.
const MAX_COMPONENT_DEPTH: u8 = 8;

fn corrupted(msg: String) -> Error {
    Error::new(ErrorClass::CorruptedTable, msg)
}

/// Decodes the glyph `glyphs[glyph_id]`. Components resolve recursively
/// against the same glyph list, capped at [`MAX_COMPONENT_DEPTH`]. Returns
/// `Ok(None)` for an empty glyph (a loca range shorter than the 10-byte
/// header, which loca already cut to its actual bytes).
pub(super) fn decode_glyph(
    glyphs: &[&[u8]],
    glyph_id: usize,
    depth: u8,
) -> Result<Option<GlyphOutline>, Error> {
    let glyph = glyphs.get(glyph_id).ok_or_else(|| {
        corrupted(format!("glyph id {glyph_id} is out of range"))
    })?;
    if glyph.len() < 10 {
        return Ok(None);
    }
    if depth > MAX_COMPONENT_DEPTH {
        return Err(corrupted(format!(
            "component nesting deeper than {MAX_COMPONENT_DEPTH} at glyph {glyph_id}"
        )));
    }
    let mut cursor = Cursor::new(glyph, 0);
    let contours_count = cursor.i16().map_err(|e| read_failed("numberOfContours", e))?;
    if contours_count < 0 {
        return Ok(Some(decode_composite(glyphs, glyph_id, glyph, depth)?));
    }
    cursor.skip(8).map_err(|e| read_failed("bounding box", e))?;
    let contours_count = usize::try_from(contours_count)
        .map_err(|_| corrupted(format!("glyph {glyph_id}: numberOfContours out of range")))?;
    let mut end_points = Vec::with_capacity(contours_count);
    for i in 0..contours_count {
        let end = usize::from(cursor.u16().map_err(|e| read_failed("endPtsOfContours", e))?);
        match end_points.last() {
            Some(&last) if end <= last => {
                return Err(corrupted(format!(
                    "glyph {glyph_id}: endPtsOfContours[{i}]={end} does not increase"
                )));
            }
            _ => {}
        }
        end_points.push(end);
    }
    let point_count = end_points.last().copied().map_or(0, |e| e + 1);
    let instruction_length =
        usize::from(cursor.u16().map_err(|e| read_failed("instructionLength", e))?);
    cursor
        .skip(instruction_length)
        .map_err(|e| read_failed("instructions", e))?;
    let points = read_points(cursor.rest(), point_count)
        .map_err(|msg| corrupted(format!("glyph {glyph_id}: {msg}")))?;
    let mut contours = Vec::with_capacity(contours_count);
    let mut start = 0usize;
    for &end in &end_points {
        contours.push(Contour {
            points: points[start..=end].to_vec(),
        });
        start = end + 1;
    }
    Ok(Some(GlyphOutline { contours }))
}

fn read_failed(field: &str, e: Error) -> Error {
    corrupted(format!("{field}: {}", e.message))
}

/// Reads the flag stream and the packed x/y deltas, accumulating absolute
/// coordinates. `bytes` is the glyph tail after the instructions.
fn read_points(bytes: &[u8], count: usize) -> Result<Vec<Point>, String> {
    let mut flags = Vec::with_capacity(count);
    let mut at = 0usize;
    while flags.len() < count {
        let flag = *bytes
            .get(at)
            .ok_or_else(|| "the flag stream runs past the glyph".to_string())?;
        at += 1;
        flags.push(flag);
        if flag & FLAG_REPEAT != 0 {
            let repeats = usize::from(
                *bytes
                    .get(at)
                    .ok_or_else(|| "a repeat count runs past the glyph".to_string())?,
            );
            at += 1;
            for _ in 0..repeats.min(count.saturating_sub(flags.len())) {
                flags.push(flag);
            }
        }
    }
    let mut x = 0i32;
    let mut xs = Vec::with_capacity(count);
    for &flag in &flags {
        if flag & FLAG_X_SHORT != 0 {
            let byte = i32::from(*bytes.get(at).ok_or_else(|| {
                "an x delta runs past the glyph".to_string()
            })?);
            at += 1;
            x += if flag & FLAG_X_SAME_OR_POSITIVE != 0 { byte } else { -byte };
        } else if flag & FLAG_X_SAME_OR_POSITIVE == 0 {
            let pair = bytes.get(at..at + 2).ok_or_else(|| {
                "an x delta runs past the glyph".to_string()
            })?;
            at += 2;
            x += i32::from(i16::from_be_bytes([pair[0], pair[1]]));
        }
        xs.push(x);
    }
    let mut y = 0i32;
    let mut points = Vec::with_capacity(count);
    for (i, &flag) in flags.iter().enumerate() {
        if flag & FLAG_Y_SHORT != 0 {
            let byte = i32::from(*bytes.get(at).ok_or_else(|| {
                "a y delta runs past the glyph".to_string()
            })?);
            at += 1;
            y += if flag & FLAG_Y_SAME_OR_POSITIVE != 0 { byte } else { -byte };
        } else if flag & FLAG_Y_SAME_OR_POSITIVE == 0 {
            let pair = bytes.get(at..at + 2).ok_or_else(|| {
                "a y delta runs past the glyph".to_string()
            })?;
            at += 2;
            y += i32::from(i16::from_be_bytes([pair[0], pair[1]]));
        }
        points.push(Point {
            x: xs[i],
            y,
            on_curve: flag & FLAG_ON_CURVE != 0,
        });
    }
    Ok(points)
}

/// Resolves a composite: each component's outline is fetched from `glyphs`,
/// transformed by its offsets and scale (offsets are applied unscaled, per
/// spec), and appended in component order.
fn decode_composite(
    glyphs: &[&[u8]],
    glyph_id: usize,
    glyph: &[u8],
    depth: u8,
) -> Result<GlyphOutline, Error> {
    let mut cursor = Cursor::new(glyph, 10);
    let mut contours: Vec<Contour> = Vec::new();
    loop {
        let flags = cursor.u16().map_err(|e| read_failed("component flags", e))?;
        let component_id = cursor.u16().map_err(|e| read_failed("component glyph index", e))?;
        let args_are_words = flags & ARG_1_AND_2_ARE_WORDS != 0;
        let args_are_xy = flags & ARGS_ARE_XY_VALUES != 0;
        if !args_are_xy {
            // Point-matching composition needs the component's and the
            // composite's contour point lists, which is a different algorithm
            // from offsets; refuse it rather than guess.
            return Err(Error::new(
                ErrorClass::Unsupported,
                format!(
                    "glyph {glyph_id}: point-matching composite components are not decoded"
                ),
            ));
        }
        let dx = if args_are_words {
            i32::from(cursor.i16().map_err(|e| read_failed("component dx", e))?)
        } else {
            i32::from(cursor.u8().map_err(|e| read_failed("component dx", e))? as i8)
        };
        let dy = if args_are_words {
            i32::from(cursor.i16().map_err(|e| read_failed("component dy", e))?)
        } else {
            i32::from(cursor.u8().map_err(|e| read_failed("component dy", e))? as i8)
        };
        let mut a = 1.0f64;
        let mut b = 0.0f64;
        let mut c = 0.0f64;
        let mut d = 1.0f64;
        let f2dot14 = |raw: i16| f64::from(raw) / f64::from(1 << 14);
        if flags & WE_HAVE_A_SCALE != 0 {
            let scale = f2dot14(cursor.i16().map_err(|e| read_failed("component scale", e))?);
            a = scale;
            d = scale;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            a = f2dot14(cursor.i16().map_err(|e| read_failed("component xscale", e))?);
            d = f2dot14(cursor.i16().map_err(|e| read_failed("component yscale", e))?);
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            a = f2dot14(cursor.i16().map_err(|e| read_failed("component transform a", e))?);
            b = f2dot14(cursor.i16().map_err(|e| read_failed("component transform b", e))?);
            c = f2dot14(cursor.i16().map_err(|e| read_failed("component transform c", e))?);
            d = f2dot14(cursor.i16().map_err(|e| read_failed("component transform d", e))?);
        }
        if let Some(outline) = decode_glyph(glyphs, usize::from(component_id), depth + 1)? {
            for contour in outline.contours {
                let transformed = contour
                    .points
                    .iter()
                    .map(|p| {
                        let (px, py) = (f64::from(p.x), f64::from(p.y));
                        Point {
                            x: (a * px + c * py + f64::from(dx)).round() as i32,
                            y: (b * px + d * py + f64::from(dy)).round() as i32,
                            on_curve: p.on_curve,
                        }
                    })
                    .collect();
                contours.push(Contour { points: transformed });
            }
        }
        if flags & MORE_COMPONENTS == 0 {
            return Ok(GlyphOutline { contours });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> Vec<u8> {
        // The same glyph logifos-layout's test builder writes: nc=1,
        // bbox 0/-200/500/700, endPts [2], no instructions, three on-curve
        // points with i16 deltas.
        let mut g = Vec::new();
        g.extend_from_slice(&1i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&(-200i16).to_be_bytes());
        g.extend_from_slice(&500i16.to_be_bytes());
        g.extend_from_slice(&700i16.to_be_bytes());
        g.extend_from_slice(&2u16.to_be_bytes());
        g.extend_from_slice(&0u16.to_be_bytes());
        g.extend_from_slice(&[0x01, 0x01, 0x01]);
        for v in [0i16, 500i16, -500i16, -200i16, 900i16, -900i16] {
            g.extend_from_slice(&v.to_be_bytes());
        }
        g
    }

    #[test]
    fn a_simple_triangle_decodes_to_three_on_curve_points() {
        let t = triangle();
        let glyphs: Vec<&[u8]> = vec![&t];
        let outline = decode_glyph(&glyphs, 0, 0).unwrap().unwrap();
        assert_eq!(outline.contours.len(), 1);
        let points = &outline.contours[0].points;
        assert_eq!(
            points,
            &[
                Point { x: 0, y: -200, on_curve: true },
                Point { x: 500, y: 700, on_curve: true },
                Point { x: 0, y: -200, on_curve: true },
            ]
        );
    }

    #[test]
    fn an_empty_glyph_is_none() {
        let t = triangle();
        let glyphs: Vec<&[u8]> = vec![&[], &t];
        assert_eq!(decode_glyph(&glyphs, 0, 0).unwrap(), None);
        assert_eq!(decode_glyph(&glyphs, 1, 0).unwrap().unwrap().contours.len(), 1);
    }

    #[test]
    fn short_deltas_and_repeat_flags_decode() {
        // nc=1, endPts [2] (inclusive: three points), no instructions; flags:
        // on-curve + x-short + positive x, repeated 2x; y deltas all zero
        // (y-same bit).
        let mut g = Vec::new();
        g.extend_from_slice(&1i16.to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // bbox all zero
        g.extend_from_slice(&2u16.to_be_bytes());
        g.extend_from_slice(&0u16.to_be_bytes());
        g.extend_from_slice(&[0x01 | 0x02 | 0x08 | 0x10 | 0x20, 0x02]);
        g.extend_from_slice(&[10u8, 20, 30]);
        let glyphs: Vec<&[u8]> = vec![&g];
        let outline = decode_glyph(&glyphs, 0, 0).unwrap().unwrap();
        let points = &outline.contours[0].points;
        assert_eq!(
            points.iter().map(|p| (p.x, p.y, p.on_curve)).collect::<Vec<_>>(),
            vec![(10, 0, true), (30, 0, true), (60, 0, true)]
        );
    }

    #[test]
    fn an_off_curve_run_is_carried_with_its_flag() {
        let mut g = Vec::new();
        g.extend_from_slice(&1i16.to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        g.extend_from_slice(&2u16.to_be_bytes());
        g.extend_from_slice(&0u16.to_be_bytes());
        // The middle point is off-curve (flag 0x00).
        g.extend_from_slice(&[0x01, 0x00, 0x01]);
        // Three x deltas then three y deltas.
        for v in [0i16, 100i16, 0i16, 0i16, 100i16, 100i16] {
            g.extend_from_slice(&v.to_be_bytes());
        }
        let glyphs: Vec<&[u8]> = vec![&g];
        let outline = decode_glyph(&glyphs, 0, 0).unwrap().unwrap();
        let points = &outline.contours[0].points;
        assert_eq!(points[0], Point { x: 0, y: 0, on_curve: true });
        assert_eq!(points[1], Point { x: 100, y: 100, on_curve: false });
        assert_eq!(points[2], Point { x: 100, y: 200, on_curve: true });
    }

    #[test]
    fn a_composite_shifts_and_scales_its_component() {
        // Glyph 1: the triangle. Glyph 0: a composite using it with offsets
        // (100, 50), xy-values as bytes, no transform.
        let mut g = Vec::new();
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        g.extend_from_slice(&(ARGS_ARE_XY_VALUES | MORE_COMPONENTS).to_be_bytes());
        g.extend_from_slice(&1u16.to_be_bytes());
        g.extend_from_slice(&[100i8 as u8, 50i8 as u8]);
        g.extend_from_slice(&ARGS_ARE_XY_VALUES.to_be_bytes()); // no MORE_COMPONENTS
        g.extend_from_slice(&1u16.to_be_bytes());
        g.extend_from_slice(&[40i8 as u8, 50]);
        let t = triangle();
        let glyphs: Vec<&[u8]> = vec![&g, &t];
        let outline = decode_glyph(&glyphs, 0, 0).unwrap().unwrap();
        let points = &outline.contours[0].points;
        assert_eq!(
            points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            vec![(100, -150), (600, 750), (100, -150)]
        );
        let second = &outline.contours[1].points;
        assert_eq!(second[0], Point { x: 40, y: -150, on_curve: true });
    }

    #[test]
    fn a_half_scale_composite_scales_but_not_the_offsets() {
        // Component with WE_HAVE_A_SCALE = 0.5 and offsets (10, 0): the
        // triangle's points halve, the offsets do not.
        let mut g = Vec::new();
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        g.extend_from_slice(&(ARGS_ARE_XY_VALUES | WE_HAVE_A_SCALE).to_be_bytes());
        g.extend_from_slice(&1u16.to_be_bytes());
        g.extend_from_slice(&[10i8 as u8, 0]);
        g.extend_from_slice(&(8192i16).to_be_bytes()); // 0.5 in F2Dot14
        let t = triangle();
        let glyphs: Vec<&[u8]> = vec![&g, &t];
        let outline = decode_glyph(&glyphs, 0, 0).unwrap().unwrap();
        let points = &outline.contours[0].points;
        assert_eq!(
            points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            vec![(10, -100), (260, 350), (10, -100)]
        );
    }

    #[test]
    fn a_component_cycle_refuses_at_the_depth_cap() {
        // Glyph 0 and glyph 1 reference each other.
        let mut zero = Vec::new();
        zero.extend_from_slice(&(-1i16).to_be_bytes());
        zero.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        zero.extend_from_slice(&(ARGS_ARE_XY_VALUES).to_be_bytes());
        zero.extend_from_slice(&1u16.to_be_bytes());
        zero.extend_from_slice(&[0, 0]);
        let mut one = zero.clone();
        one[12..14].copy_from_slice(&0u16.to_be_bytes());
        let glyphs: Vec<&[u8]> = vec![&zero, &one];
        let err = decode_glyph(&glyphs, 0, 0).unwrap_err();
        assert_eq!(err.class, ErrorClass::CorruptedTable);
    }

    #[test]
    fn a_non_increasing_end_point_refuses() {
        let mut g = Vec::new();
        g.extend_from_slice(&2i16.to_be_bytes());
        g.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        g.extend_from_slice(&1u16.to_be_bytes());
        g.extend_from_slice(&0u16.to_be_bytes());
        let glyphs: Vec<&[u8]> = vec![&g];
        let err = decode_glyph(&glyphs, 0, 0).unwrap_err();
        assert_eq!(err.class, ErrorClass::CorruptedTable);
    }
}
