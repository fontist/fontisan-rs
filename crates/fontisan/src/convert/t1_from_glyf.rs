//! TTF → Type 1 (fontisan#192 gap 4, stage 2): Ruby's `Type1::PFBGenerator`
//! direct path - every glyf glyph becomes a Type 1 CharString (quadratic
//! curves become cubics), renamed via the post table, assembled by the
//! canonical PFB writer. The loca/glyf parsing, the UPM scaling, the contour
//! walk, the composite component walk (per-component RMOVETO + RLINETO runs,
//! unscaled, truncating transform, no hsbw and no endchar - Ruby's
//! `convert_composite_glyph`), the quadratic-to-cubic rounding and the hmtx
//! metrics mirror the Ruby converter byte for byte.

use crate::convert::ConvertError;
use crate::convert::t1write::Pfb;

/// Ruby's `TTFToType1Converter#encode_number` - the 108..1131 two-byte form.
fn encode_number_ttf(value: i64) -> Vec<u8> {
    if (-107..=107).contains(&value) {
        vec![(value + 139) as u8]
    } else if (108..=1131).contains(&value) {
        let v = value - 108;
        vec![((v >> 8) + 247) as u8, (v & 0xFF) as u8]
    } else if (-1131..=-108).contains(&value) {
        let v = -value - 108;
        vec![((v >> 8) + 251) as u8, (v & 0xFF) as u8]
    } else if (-32768..=32767).contains(&value) {
        vec![255, (value & 0xFF) as u8, ((value >> 8) & 0xFF) as u8]
    } else {
        let mut out = vec![255u8];
        for i in 0..4 {
            out.push(((value >> (8 * i)) & 0xFF) as u8);
        }
        out
    }
}

/// Ruby's `Rational#round` (half away from zero) over num/den.
fn rational_round(num: i64, den: i64) -> i64 {
    if den <= 0 {
        return num.div_euclid(den.max(1));
    }
    let a = num.abs() * 2 + den;
    let q = a / (den * 2);
    if num >= 0 { q } else { -q }
}

fn tag_data<'a>(tables: &'a [([u8; 4], Vec<u8>)], t: &[u8]) -> Option<&'a [u8]> {
    tables
        .iter()
        .find(|(tag, _)| tag.as_slice() == t)
        .map(|(_, data)| data.as_slice())
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|v| v as i16)
}

fn f2dot14(v: u16) -> f64 {
    f64::from(v as i16) / 16384.0
}

/// One parsed component of a compound glyph (Ruby's Component fields).
struct Component {
    flags: u16,
    glyph_index: u16,
    arg1: i64,
    arg2: i64,
    scale_x: f64,
    scale_y: f64,
    scale_01: f64,
    scale_10: f64,
}

impl Component {
    fn args_are_xy(&self) -> bool {
        self.flags & 0x0002 != 0
    }

    /// Ruby's `transformation_matrix` layout.
    fn matrix(&self) -> [f64; 6] {
        if self.flags & 0x0080 != 0 {
            [
                self.scale_x,
                self.scale_01,
                self.scale_10,
                self.scale_y,
                self.arg1 as f64,
                self.arg2 as f64,
            ]
        } else if self.flags & 0x0040 != 0 {
            [
                self.scale_x,
                0.0,
                0.0,
                self.scale_y,
                self.arg1 as f64,
                self.arg2 as f64,
            ]
        } else if self.flags & 0x0008 != 0 {
            [
                self.scale_x,
                0.0,
                0.0,
                self.scale_x,
                self.arg1 as f64,
                self.arg2 as f64,
            ]
        } else {
            [1.0, 0.0, 0.0, 1.0, self.arg1 as f64, self.arg2 as f64]
        }
    }
}

/// Parse the components of a compound glyph (Ruby's `CompoundGlyph#parse_data`):
/// flags u16, glyph index u16, args (8- or 16-bit signed pairs), the transform
/// per flags, and the 2x2 matrix when WE_HAVE_A_TWO_BY_TWO.
fn parse_components(data: &[u8]) -> Option<Vec<Component>> {
    let mut comps = Vec::new();
    let mut i = 10usize; // glyph header
    loop {
        let flags = u16_at(data, i)?;
        let glyph_index = u16_at(data, i + 2)?;
        i += 4;
        let (arg1, arg2) = if flags & 0x0001 == 0 {
            let a = i8::from_be_bytes([data.get(i).copied()?]) as i64;
            let b = i8::from_be_bytes([data.get(i + 1).copied()?]) as i64;
            i += 2;
            (a, b)
        } else {
            let a = i64::from(i16_at(data, i)?);
            let b = i64::from(i16_at(data, i + 2)?);
            i += 4;
            (a, b)
        };
        let mut scale_x = 0.0;
        let mut scale_y = 0.0;
        let mut scale_01 = 0.0;
        let mut scale_10 = 0.0;
        if flags & 0x0080 != 0 {
            scale_x = f2dot14(u16_at(data, i)?);
            scale_01 = f2dot14(u16_at(data, i + 2)?);
            scale_10 = f2dot14(u16_at(data, i + 4)?);
            scale_y = f2dot14(u16_at(data, i + 6)?);
            i += 8;
        } else if flags & 0x0040 != 0 {
            scale_x = f2dot14(u16_at(data, i)?);
            scale_y = f2dot14(u16_at(data, i + 2)?);
            i += 4;
        } else if flags & 0x0008 != 0 {
            scale_x = f2dot14(u16_at(data, i)?);
            scale_y = scale_x;
            i += 2;
        }
        comps.push(Component {
            flags,
            glyph_index,
            arg1,
            arg2,
            scale_x,
            scale_y,
            scale_01,
            scale_10,
        });
        if flags & 0x0020 == 0 {
            break;
        }
    }
    Some(comps)
}

/// Ruby's `SimpleGlyph#points_for_contour` per contour (x, y, on-curve).
fn simple_points(data: &[u8]) -> Option<Vec<Vec<(i64, i64, bool)>>> {
    let num_contours = i16_at(data, 0)?;
    if num_contours < 0 {
        return None;
    }
    let nc = num_contours as usize;
    let mut ends = Vec::with_capacity(nc);
    for c in 0..nc {
        ends.push(usize::from(u16_at(data, 10 + 2 * c)?));
    }
    let num_points = ends.last().copied().unwrap_or(0) + 1;
    let mut start = 0usize;
    let instr_len = usize::from(u16_at(data, 10 + 2 * nc)?);
    let mut i = 10 + 2 * nc + 2 + instr_len;

    // flags (repeat bit 0x08)
    let mut flags = Vec::with_capacity(num_points);
    while flags.len() < num_points {
        let f = data.get(i).copied()?;
        i += 1;
        flags.push(f);
        if f & 0x08 != 0 {
            let repeat = data.get(i).copied()?;
            i += 1;
            for _ in 0..repeat {
                if flags.len() >= num_points {
                    break;
                }
                flags.push(f);
            }
        }
    }
    // x deltas
    let mut xs = Vec::with_capacity(num_points);
    let mut x = 0i64;
    for f in &flags {
        if f & 0x02 != 0 {
            let d = data.get(i).copied().unwrap_or(0) as i64;
            i += 1;
            x += if f & 0x10 != 0 { d } else { -d };
        } else if f & 0x10 == 0 {
            x += i64::from(i16_at(data, i)?);
            i += 2;
        }
        xs.push(x);
    }
    // y deltas
    let mut ys = Vec::with_capacity(num_points);
    let mut y = 0i64;
    for f in &flags {
        if f & 0x04 != 0 {
            let d = data.get(i).copied().unwrap_or(0) as i64;
            i += 1;
            y += if f & 0x20 != 0 { d } else { -d };
        } else if f & 0x20 == 0 {
            y += i64::from(i16_at(data, i)?);
            i += 2;
        }
        ys.push(y);
    }
    let all: Vec<(i64, i64, bool)> = (0..num_points)
        .map(|p| (xs[p], ys[p], flags[p] & 0x01 != 0))
        .collect();
    Some(
        ends.iter()
            .map(|end| {
                let slice = all[start..=*end].to_vec();
                start = end + 1;
                slice
            })
            .collect(),
    )
}

/// Ruby's `TTFToType1Converter#convert_contours` over the scaled points.
fn convert_contours(points: &[(i64, i64, bool)]) -> Vec<Vec<i64>> {
    let mut commands = Vec::new();
    if points.is_empty() {
        return commands;
    }
    let (mut cx, mut cy) = (points[0].0, points[0].1);
    for i in 1..points.len() {
        let (x, y, on) = points[i];
        if on {
            if !points[i - 1].2 {
                let (px, py, _) = points[i - 1];
                let c1x = cx + rational_round(2 * (px - cx), 3);
                let c1y = cy + rational_round(2 * (py - cy), 3);
                let c2x = x + rational_round(2 * (px - x), 3);
                let c2y = y + rational_round(2 * (py - y), 3);
                commands.push(vec![
                    8,
                    c1x - cx,
                    c1y - cy,
                    c2x - c1x,
                    c2y - c1y,
                    x - c2x,
                    y - c2y,
                ]);
            } else {
                commands.push(vec![5, x - cx, y - cy]);
            }
            cx = x;
            cy = y;
        } else if i + 1 < points.len() && !points[i + 1].2 {
            let (nx, ny, _) = points[i + 1];
            let ix = ((x + nx) as f64 / 2.0).round() as i64;
            let iy = ((y + ny) as f64 / 2.0).round() as i64;
            let c1x = cx + rational_round(2 * (x - cx), 3);
            let c1y = cy + rational_round(2 * (y - cy), 3);
            let c2x = ix + rational_round(2 * (x - ix), 3);
            let c2y = iy + rational_round(2 * (y - iy), 3);
            commands.push(vec![
                8,
                c1x - cx,
                c1y - cy,
                c2x - c1x,
                c2y - c1y,
                ix - c2x,
                iy - c2y,
            ]);
            cx = ix;
            cy = iy;
        }
    }
    commands
}

/// Ruby's `encode_charstring`: every integer in every command (operators
/// included) is encoded as a number.
fn encode_charstring(commands: &[Vec<i64>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for cmd in commands {
        for value in cmd {
            bytes.extend_from_slice(&encode_number_ttf(*value));
        }
    }
    bytes
}

/// The ttf → type1 conversion: the PFB bytes for a TrueType source, mirroring
/// Ruby's `Type1::PFBGenerator.generate` (the direct glyf path).
pub fn ttf_to_type1(tables: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>, ConvertError> {
    let glyf =
        tag_data(tables, b"glyf").ok_or_else(|| ConvertError::transform("glyf table not found"))?;
    let loca =
        tag_data(tables, b"loca").ok_or_else(|| ConvertError::transform("loca table not found"))?;
    let maxp =
        tag_data(tables, b"maxp").ok_or_else(|| ConvertError::transform("maxp table not found"))?;
    let hmtx = tag_data(tables, b"hmtx");
    let name = tag_data(tables, b"name").and_then(|d| crate::sfnt::tables::Name::parse(d).ok());
    let head = tag_data(tables, b"head").and_then(|d| crate::sfnt::tables::Head::parse(d).ok());
    let post = tag_data(tables, b"post").and_then(|d| crate::sfnt::tables::Post::parse(d).ok());
    let os2 = tag_data(tables, b"OS/2").and_then(|d| crate::sfnt::tables::Os2::parse(d).ok());
    let hhea = tag_data(tables, b"hhea").and_then(|d| crate::sfnt::tables::Hhea::parse(d).ok());

    let psname = name
        .as_ref()
        .and_then(|n| n.english_name(6))
        .unwrap_or_else(|| "Untitled".to_string());
    let version = name
        .as_ref()
        .and_then(|n| n.english_name(5))
        .unwrap_or_else(|| "1.0".to_string());

    let upm = i64::from(head.as_ref().map_or(0, |h| h.units_per_em));
    let scale = if upm > 0 { (1000, upm) } else { (1, 1) };
    let sc = |v: i64| {
        if v == 0 {
            0
        } else {
            rational_round(v * scale.0, scale.1)
        }
    };

    // Ruby's PFBGenerator#fontinfo (the TTF path: underline always rendered
    // when the post table parsed; Copyright when the name table has one).
    let mut fontinfo = vec![format!("/version ({version}) def")];
    fontinfo.push(format!(
        "/FullName ({}) def",
        name.as_ref()
            .and_then(|n| n.english_name(4))
            .unwrap_or_else(|| psname.clone())
    ));
    fontinfo.push(format!(
        "/FamilyName ({}) def",
        name.as_ref()
            .and_then(|n| n.english_name(1))
            .unwrap_or_else(|| psname.clone())
    ));
    // Ruby's weight guard checks respond_to?(:weight_class) on the raw
    // BinData OS/2 record, which has only us_weight_class - the guard never
    // passes, so the PFB always names Regular.
    fontinfo.push("/Weight (Regular) def".to_string());
    let italic_angle = post
        .as_ref()
        .map_or(0.0, |p| f64::from(p.italic_angle_raw) / 65536.0);
    fontinfo.push(format!("/ItalicAngle {italic_angle:.6} def"));
    let fixed_pitch = post.as_ref().is_some_and(|p| p.is_fixed_pitch != 0);
    fontinfo.push(format!("/isFixedPitch {fixed_pitch} def"));
    if let Some(copyright) = name.as_ref().and_then(|n| n.english_name(0)) {
        fontinfo.push(format!("/Copyright ({copyright}) def"));
    }
    if let Some(p) = post.as_ref() {
        fontinfo.push(format!(
            "/UnderlinePosition {:.6} def",
            sc(i64::from(p.underline_position)) as f64
        ));
        fontinfo.push(format!(
            "/UnderlineThickness {:.6} def",
            sc(i64::from(p.underline_thickness)) as f64
        ));
    }

    let bbox = head.as_ref().map_or([0; 4], |h| {
        [
            sc(i64::from(h.x_min)) as i32,
            sc(i64::from(h.y_min)) as i32,
            sc(i64::from(h.x_max)) as i32,
            sc(i64::from(h.y_max)) as i32,
        ]
    });

    // private_entries: BlueValues/password/MinFeature, then the OS/2 stem
    // guesses and ForceBold.
    let us_weight_class = os2.as_ref().map_or(0, |o| o.us_weight_class);
    let mut private_entries: Vec<(&str, String)> = vec![
        ("BlueValues", "[0 0]".to_string()),
        ("password", "5839".to_string()),
        ("MinFeature", "{16 16}".to_string()),
    ];
    if us_weight_class != 0 {
        // Ruby: [100, 80, 90][weight_class / 100] || 80 - an index past the
        // end (weight classes 300-900) falls back to 80.
        let idx = (us_weight_class / 100) as usize;
        let stem = sc(if idx <= 2 { [100, 80, 90][idx] } else { 80 });
        private_entries.push(("StdHW", stem.to_string()));
        private_entries.push(("StdVW", stem.to_string()));
        private_entries.push(("ForceBold", (us_weight_class >= 700).to_string()));
    }

    // hmtx metrics: numberOfHMetrics × (advance, lsb), then the last advance
    // with indexed LSBs (Ruby's `Hmtx#metric_for`).
    let num_metrics = usize::from(hhea.as_ref().map_or(0, |h| h.number_of_h_metrics));
    let num_glyphs = usize::from(u16_at(maxp, 4).unwrap_or(0));
    let metric = |gid: usize| -> (i64, i64) {
        let Some(data) = hmtx else {
            return (0, 500);
        };
        if gid < num_metrics {
            let at = 4 * gid;
            let Some(b) = data.get(at..at + 4) else {
                return (0, 500);
            };
            (
                i64::from(u16::from_be_bytes([b[0], b[1]])),
                i64::from(i16::from_be_bytes([b[2], b[3]])),
            )
        } else {
            let at = 4 * num_metrics.saturating_sub(1);
            let Some(b) = data.get(at..at + 2) else {
                return (0, 500);
            };
            let advance = i64::from(u16::from_be_bytes([b[0], b[1]]));
            let idx = 4 * num_metrics + 2 * (gid - num_metrics);
            let lsb = data
                .get(idx..idx + 2)
                .map(|b| i64::from(i16::from_be_bytes([b[0], b[1]])))
                .unwrap_or(0);
            (advance, lsb)
        }
    };

    // loca offsets (format 0 doubles the u16s).
    let long_loca = head.as_ref().is_some_and(|h| h.index_to_loc_format != 0);
    let loca_at = |gid: usize| -> Option<usize> {
        if long_loca {
            let at = 4 * gid;
            let b = loca.get(at..at + 4)?;
            Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        } else {
            Some(2 * usize::from(u16_at(loca, 2 * gid)?))
        }
    };

    let post_names = post.as_ref().map(|p| p.glyph_names());
    let mut charstrings: Vec<(String, Vec<u8>)> = Vec::with_capacity(num_glyphs);
    for gid in 0..num_glyphs {
        let name = post_names
            .as_ref()
            .and_then(|names| names.get(gid).cloned())
            .unwrap_or_else(|| format!("glyph{gid}"));
        let empty = vec![0u8, 244, 14]; // [0, 500, 14].pack("C*") - 500 truncates
        let (Some(start), Some(end)) = (loca_at(gid), loca_at(gid + 1)) else {
            charstrings.push((name, empty));
            continue;
        };
        let Some(slice) = (end > start).then(|| glyf.get(start..end)).flatten() else {
            charstrings.push((name, empty));
            continue;
        };
        let Some(num_contours) = i16_at(slice, 0) else {
            charstrings.push((name, empty));
            continue;
        };

        let plain = if num_contours < 0 {
            // Ruby's convert_composite_glyph dereferences @loca_table /
            // @head_table, which TTFToType1Converter#initialize never
            // assigns - so Glyf#glyph_for survives only on its cache, which
            // holds every gid the convert loop has already processed. A
            // component whose gid is still uncached raises into the method's
            // rescue and the whole glyph converts to the empty charstring.
            let mut commands: Vec<Vec<i64>> = Vec::new();
            let mut ok = true;
            match parse_components(slice) {
                Some(components) => {
                    for component in &components {
                        if !component.args_are_xy() {
                            continue;
                        }
                        let cgid = usize::from(component.glyph_index);
                        if cgid >= gid {
                            // Uncached: Ruby's glyph_for raises here.
                            ok = false;
                            break;
                        }
                        let (Some(cs), Some(ce)) = (loca_at(cgid), loca_at(cgid + 1)) else {
                            ok = false;
                            break;
                        };
                        let Some(comp_slice) = (ce > cs).then(|| glyf.get(cs..ce)).flatten() else {
                            // Empty component: glyph_for cached nil - skipped.
                            continue;
                        };
                        let Some(ncc) = i16_at(comp_slice, 0) else {
                            ok = false;
                            break;
                        };
                        if ncc < 0 {
                            continue; // not simple - Ruby skips it
                        }
                        let m = component.matrix();
                        let tx = |x: f64, y: f64| -> i64 { (m[0] * x + m[2] * y + m[4]) as i64 };
                        let ty = |x: f64, y: f64| -> i64 { (m[1] * x + m[3] * y + m[5]) as i64 };
                        let Some(contours) = simple_points(comp_slice) else {
                            ok = false;
                            break;
                        };
                        // Ruby's transform_simple_glyph_commands: per
                        // contour, RMOVETO the transformed first point then
                        // RLINETO transformed deltas - every point, on- or
                        // off-curve, no closing line.
                        for contour in &contours {
                            if contour.is_empty() {
                                continue;
                            }
                            let (x0, y0, _) = contour[0];
                            let (fx, fy) = (tx(x0 as f64, y0 as f64), ty(x0 as f64, y0 as f64));
                            commands.push(vec![21, fx, fy]);
                            let (mut px, mut py) = (x0 as f64, y0 as f64);
                            for (x, y, _) in &contour[1..] {
                                let (x, y) = (*x as f64, *y as f64);
                                let dx = tx(x, y) - tx(px, py);
                                let dy = ty(x, y) - ty(px, py);
                                commands.push(vec![5, dx, dy]);
                                px = x;
                                py = y;
                            }
                        }
                    }
                }
                None => ok = false,
            }
            if !ok || commands.is_empty() {
                empty
            } else {
                encode_charstring(&commands)
            }
        } else {
            // simple glyph: hsbw(scale(lsb), scale(width)) + the contour walk
            // + endchar, scaled, through encode_charstring.
            let Some(contours) = simple_points(slice) else {
                charstrings.push((name, empty));
                continue;
            };
            let mut points: Vec<(i64, i64, bool)> = contours.iter().flatten().copied().collect();
            for p in &mut points {
                *p = (sc(p.0), sc(p.1), p.2);
            }
            if points.is_empty() {
                charstrings.push((name, empty));
                continue;
            }
            let (advance, lsb) = metric(gid);
            let mut commands: Vec<Vec<i64>> = vec![vec![13, sc(lsb), sc(advance)]];
            commands.extend(convert_contours(&points));
            commands.push(vec![14]);
            encode_charstring(&commands)
        };
        charstrings.push((name, plain));
    }

    let pfb = Pfb {
        psname: &psname,
        version: &version,
        fontinfo,
        bbox,
        private_entries,
        charstrings: charstrings
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.clone()))
            .collect(),
    };
    pfb.write()
}
