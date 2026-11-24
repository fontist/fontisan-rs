//! SVG font generation (fontisan#192 gap 3): the `SvgGenerator` pipeline —
//! `Svg::FontGenerator`, `FontFaceGenerator`, `GlyphGenerator` over the
//! `OutlineExtractor` contours — byte-identical text.
//!
//! The Ruby generator has latent duck-typing failures that rescue into
//! constants; parity ports the behaviour, not the intent (measured against
//! the Ruby output on the corpus fonts):
//!  * `Tables::Name` has no `font_family`/`postscript_name` → the font id is
//!    always `"Font"` and `font-family` always `"Unknown"`;
//!  * OS/2 has no `weight_class`/`x_height`/`cap_height` → those attributes
//!    never emit;
//!  * `Hmtx` has no `advance_width_for` → glyph elements never carry
//!    `horiz-adv-x`;
//!  * the CFF outline path runs through `charstring_for_glyph`, which is nil
//!    for every glyph when the top DICT has no Private entry (CID fonts) →
//!    no glyph elements for such fonts.

use std::fmt::Write as _;

use super::ConvertError;
use crate::cffread;
use crate::instance::outline::{Component, Outline};
use crate::sfnt::OutlineKind;
use crate::t2interp::{Interp, Seg};
use crate::sfnt::Error;

const ERROR_CLASS: &str = "Fontisan::Error";

/// One contour: rounded (x, y, on-curve) points.
type Contour = Vec<(i64, i64, bool)>;

/// `Ok(None)` is Ruby's nil outline (no glyph element, or a skipped
/// component); `Err(())` is a raise the per-glyph rescue turns into a
/// skipped glyph.
type Extracted = Result<Option<Vec<Contour>>, ()>;

fn surface(e: Error) -> ConvertError {
    ConvertError::new(e.class.ruby_name(), e.message.clone())
}

/// `Ruby Float#to_s` for the implied-midpoint coordinates: integral values
/// keep one decimal (`90.0`), everything else is the shortest round-trip
/// decimal (Rust's `{}`), which matches Ruby's shortest repr.
fn ruby_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e16 {
        format!("{:.1}", v)
    } else {
        format!("{v}")
    }
}

fn round(v: f64) -> i64 {
    v.round() as i64 // half away from zero, like Ruby's Float#round
}

/// The SVG target (`Fontisan.convert(input, to: :svg)`), for a single sfnt
/// font. Collection inputs never reach here (the Ruby convert gate routes
/// them to `CollectionConverter`).
pub fn generate(input: &[u8]) -> Result<Vec<u8>, ConvertError> {
    // The loader's error surface first: Ruby's ConvertCommand loads the font
    // through FontLoader before the generator validates anything.
    let font = crate::sfnt::Font::load(input, 0, crate::sfnt::Options::default())
        .map_err(surface)?;

    // SvgGenerator#validate, in its order.
    for tag in ["head", "hhea", "maxp", "cmap"] {
        if font.table_data(tag).is_none() {
            return Err(ConvertError::new(
                ERROR_CLASS,
                format!("Font is missing required table: {tag}"),
            ));
        }
    }
    let has_glyf = font.table_data("glyf").is_some();
    let has_cff = font.table_data("CFF ").is_some();
    let has_cff2 = font.table_data("CFF2").is_some();
    if !has_glyf && !has_cff && !has_cff2 {
        return Err(ConvertError::new(
            ERROR_CLASS,
            "Font must have either glyf or CFF/CFF2 table",
        ));
    }

    let head = font.head().map_err(surface)?.unwrap_or_default();
    let hhea = font.hhea().map_err(surface)?.unwrap_or_default();
    let maxp = font.maxp().map_err(surface)?.unwrap_or_default();
    let os2 = font.os2().map_err(surface)?;
    let post = font.post().map_err(surface)?;
    let num_glyphs = usize::from(maxp.num_glyphs);
    let ascent = i64::from(hhea.ascent);
    let flip = |y: i64| ascent - y;

    // FontFaceGenerator: constant family (the name-table calls always raise
    // and are rescued), OS/2 fs_selection style with the unreachable
    // name-table fallback, underline attrs per their nil / positive checks.
    let style = match os2 {
        Some(o) if o.fs_selection & 0x01 != 0 => "italic",
        _ => "normal",
    };
    let mut face = format!(
        "      <font-face font-family=\"Unknown\" units-per-em=\"{}\" \
         ascent=\"{}\" descent=\"{}\" font-style=\"{style}\" bbox=\"{} {} {} {}\"",
        head.units_per_em,
        hhea.ascent,
        hhea.descent,
        head.x_min,
        head.y_min,
        head.x_max,
        head.y_max,
    );
    if let Some(p) = &post {
        let _ = write!(face, " underline-position=\"{}\"", p.underline_position);
        if p.underline_thickness > 0 {
            let _ = write!(face, " underline-thickness=\"{}\"", p.underline_thickness);
        }
    }
    face.push_str("/>");

    let missing = "      <missing-glyph horiz-adv-x=\"500\"/>".to_string();

    // Codepoint reverse map: gid => [cp, ...] in cmap order.
    let mut gid_to_cps: Vec<Vec<u32>> = vec![Vec::new(); num_glyphs];
    if let Some(cmap) = font.cmap().map_err(surface)? {
        for (cp, gid) in cmap.mappings() {
            let gid = gid as usize;
            if gid < num_glyphs {
                gid_to_cps[gid].push(cp);
            }
        }
    }
    let glyph_names = post.map(|p| p.glyph_names()).unwrap_or_default();

    // Outline sources. The extractor checks `table("CFF ")` FIRST, so a
    // glyf+CFF hybrid goes through the CFF path. The glyf slices come from
    // the outlines walk (glyf+loca only): glyf without loca leaves them
    // empty, which is Ruby's per-glyph MissingTableError → every glyph
    // skipped.
    let glyf_slices = if has_glyf {
        match font.outlines() {
            Ok(Some(o)) if o.kind == OutlineKind::Glyf => Some(o.glyphs),
            _ => None,
        }
    } else {
        None
    };
    let cff = if has_cff {
        font.table_data("CFF ")
            .and_then(|data| cffread::parse(data).ok())
    } else {
        None
    };

    let mut glyphs: Vec<String> = Vec::new();
    for (gid, cps) in gid_to_cps.iter_mut().enumerate() {
        let contours = if has_cff {
            cff_contours(gid, &cff)
        } else {
            glyf_glyph(gid, glyf_slices.as_deref(), 0)
        };
        // Ruby: the outline object nil → no element; contours empty → an
        // element without a `d`; a raise → the whole glyph skipped.
        let Ok(Some(contours)) = contours else { continue };

        let path = path_data(&contours, &flip, ascent as f64);
        let mut attrs = String::new();
        if !cps.is_empty() {
            let _ = write!(attrs, " unicode=\"{}\"", format_codepoints(cps));
        }
        let name = match glyph_names.get(gid) {
            Some(n) => n.clone(),
            None => format!("gid{gid}"),
        };
        let _ = write!(attrs, " glyph-name=\"{}\"", escape_xml(&name));
        if let Some(path) = path {
            let _ = write!(attrs, " d=\"{path}\"");
        }
        glyphs.push(format!("      <glyph{attrs}/>"));
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\">\n");
    out.push_str("  <defs>\n");
    out.push_str("    <font id=\"Font\" horiz-adv-x=\"500\">\n");
    out.push_str(&face);
    out.push('\n');
    out.push_str(&missing);
    // Ruby pushes the glyph block as a (possibly empty) string, so an empty
    // glyph set still contributes the blank line before `</font>`.
    out.push('\n');
    out.push_str(&glyphs.join("\n"));
    out.push_str("\n    </font>\n");
    out.push_str("  </defs>\n");
    out.push_str("</svg>");
    Ok(out.into_bytes())
}

/// One glyph's contours from the loca-located glyf slices.
fn glyf_glyph(gid: usize, slices: Option<&[&[u8]]>, depth: u8) -> Extracted {
    if depth > 64 {
        // Ruby: mutual component cycles blow the stack (SystemStackError,
        // not a StandardError) — the closest surface is a skipped glyph.
        return Err(());
    }
    let Some(bytes) = slices.and_then(|s| s.get(gid)).copied() else {
        return Err(()); // Ruby: MissingTableError / ArgumentError per glyph
    };
    if bytes.is_empty() {
        return Ok(None); // empty glyph (space): outline nil
    }
    let o = Outline::parse(bytes).map_err(|_| ())?;
    if !o.is_composite() {
        let mut out: Vec<Contour> = Vec::new();
        let mut start = 0usize;
        for end in &o.end_pts {
            let end = *end + 1;
            if end > o.points.len() {
                return Err(()); // corrupt: Ruby raises, the glyph is skipped
            }
            let contour: Contour = (start..end)
                .map(|i| (o.points[i].0 as i64, o.points[i].1 as i64, o.on_curve[i]))
                .collect();
            if !contour.is_empty() {
                out.push(contour);
            }
            start = end;
        }
        return Ok(Some(out));
    }
    // Composite: each component's outline is extracted recursively (a nil
    // sub-outline skips the component; a raise skips the whole glyph), then
    // `transformation_matrix` applies with rounding.
    let mut all: Vec<Contour> = Vec::new();
    for c in &o.components {
        let Some(bytes) = slices.and_then(|s| s.get(usize::from(c.gid))).copied() else {
            return Err(());
        };
        let sub = if bytes.is_empty() {
            Ok(None)
        } else {
            glyf_glyph(usize::from(c.gid), slices, depth + 1)
        };
        let Ok(Some(contours)) = sub else { continue };
        let (a, b, cc, d, e, f) = component_matrix(c);
        for contour in &contours {
            let mapped: Contour = contour
                .iter()
                .map(|&(x, y, on)| {
                    let (x, y) = (x as f64, y as f64);
                    (round(a * x + cc * y + e), round(b * x + d * y + f), on)
                })
                .collect();
            if !mapped.is_empty() {
                all.push(mapped);
            }
        }
    }
    Ok(Some(all)) // possibly empty: Ruby still emits the element, without `d`
}

/// Ruby's `Component#transformation_matrix`: `[a, b, c, d, e, f]` with the
/// F2Dot14 scales and the raw (possibly point-index) arguments as offsets.
fn component_matrix(c: &Component) -> (f64, f64, f64, f64, f64, f64) {
    const HAVE_SCALE: u16 = 0x0008;
    const HAVE_XY_SCALE: u16 = 0x0040;
    const HAVE_TWO_BY_TWO: u16 = 0x0080;
    let f2 = |raw: &[u8]| -> f64 {
        let v = u16::from_be_bytes([raw[0], raw[1]]) as i16;
        f64::from(v) / 16_384.0
    };
    let (arg1, arg2) = (c.arg1 as f64, c.arg2 as f64);
    if c.flags & HAVE_TWO_BY_TWO != 0 {
        // parse order: [xscale, scale01, scale10, yscale]
        (
            f2(&c.transform[0..2]),
            f2(&c.transform[2..4]),
            f2(&c.transform[4..6]),
            f2(&c.transform[6..8]),
            arg1,
            arg2,
        )
    } else if c.flags & HAVE_XY_SCALE != 0 {
        (f2(&c.transform[0..2]), 0.0, 0.0, f2(&c.transform[2..4]), arg1, arg2)
    } else if c.flags & HAVE_SCALE != 0 {
        let s = f2(&c.transform[0..2]);
        (s, 0.0, 0.0, s, arg1, arg2)
    } else {
        (1.0, 0.0, 0.0, 1.0, arg1, arg2)
    }
}

/// CFF contours through the `charstring_for_glyph` semantics: no top-DICT
/// Private entry (`has_private == false`) yields nil for every glyph;
/// per-glyph interpretation failures are rescued to nil.
fn cff_contours(gid: usize, cff: &Option<cffread::Cff>) -> Extracted {
    let Some(cff) = cff else { return Err(()) }; // the table parsed in Ruby; a parse failure is per-glyph
    if !cff.has_private {
        return Ok(None);
    }
    let Some(cs) = cff.charstrings.get(gid) else {
        return Ok(None); // Ruby: `charstring_at` nil for a missing INDEX entry
    };
    if cs.is_empty() {
        return Ok(None); // empty CharString → empty path → outline nil
    }
    let mut it = Interp::new(&cff.global_subrs, cff.local_subrs.as_deref());
    if it.run_main(cs).is_err() {
        return Err(()); // the interpreter raise → glyph skipped
    }
    if it.path.is_empty() {
        return Ok(None);
    }
    let mut contours: Vec<Contour> = Vec::new();
    let mut current: Contour = Vec::new();
    for seg in &it.path {
        match *seg {
            Seg::Move(x, y) => {
                if !current.is_empty() {
                    contours.push(std::mem::take(&mut current));
                }
                current.push((round(x), round(y), true));
            }
            Seg::Line(x, y) => current.push((round(x), round(y), true)),
            Seg::Curve(c) => {
                // The extractor keeps both cubic controls as off-curve points
                // and the end on-curve; GlyphGenerator's quadratic walk then
                // renders them as two Q segments.
                current.push((round(c[0]), round(c[1]), false));
                current.push((round(c[2]), round(c[3]), false));
                current.push((round(c[4]), round(c[5]), true));
            }
        }
    }
    if !current.is_empty() {
        contours.push(current);
    }
    if contours.is_empty() {
        return Ok(None);
    }
    Ok(Some(contours))
}

/// GlyphGenerator's path builder: `M`/`L` on-curve points, `Q` segments with
/// the implied midpoint between consecutive off-curve points, `Z` per
/// contour, svg_y = ascent − y. `None` when no contours (no `d` attribute).
fn path_data(contours: &[Contour], flip: &dyn Fn(i64) -> i64, ascent_f: f64) -> Option<String> {
    if contours.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for contour in contours {
        if contour.is_empty() {
            continue;
        }
        let first = contour[0];
        parts.push(format!("M {} {}", first.0, flip(first.1)));
        let mut i = 1usize;
        while i < contour.len() {
            let point = contour[i];
            if point.2 {
                parts.push(format!("L {} {}", point.0, flip(point.1)));
                i += 1;
                continue;
            }
            let (cx, cy) = (point.0, flip(point.1));
            i += 1;
            if i < contour.len() && !contour[i].2 {
                // Two consecutive off-curve points: the implied on-curve
                // midpoint is a Ruby float (x printed directly, y flipped
                // without rounding).
                let next = contour[i];
                let mx = (point.0 as f64 + next.0 as f64) / 2.0;
                let my = (point.1 as f64 + next.1 as f64) / 2.0;
                parts.push(format!(
                    "Q {} {} {} {}",
                    cx,
                    cy,
                    ruby_float(mx),
                    ruby_float(ascent_f - my)
                ));
            } else if i < contour.len() {
                let end = contour[i];
                parts.push(format!("Q {} {} {} {}", cx, cy, end.0, flip(end.1)));
                i += 1;
            } else {
                parts.push(format!("Q {} {} {} {}", cx, cy, first.0, flip(first.1)));
            }
        }
        parts.push("Z".to_string());
    }
    Some(parts.join(" "))
}

/// The `unicode=` attribute value: printable ASCII characters (XML-escaped),
/// everything else as `&#xHEX;` entities, concatenated in order.
fn format_codepoints(cps: &[u32]) -> String {
    let mut out = String::new();
    for &cp in cps {
        if (0x20..=0x7e).contains(&cp) {
            out.push_str(&escape_xml(&(cp as u8 as char).to_string()));
        } else {
            let _ = write!(out, "&#x{:X};", cp);
        }
    }
    out
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
