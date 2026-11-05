//! TrueType (glyf) to OpenType (CFF) outline conversion: port of
//! `Converters::OutlineConverter#convert_ttf_to_otf` and `CffTableBuilder`,
//! plus the two post-write validator rule groups (name, cmap) that fire on
//! the corpus (`Pipeline::TransformationPipeline#validate_output`).
//!
//! The output is byte-literal Ruby, including its defects (the Name INDEX
//! that OTS rejects, the trailing empty local-subr INDEX).

mod cff;
mod glyf;
mod name;

use super::{ConvertError, Tables};

fn table<'a>(tables: &'a Tables, tag: &[u8; 4]) -> Option<&'a Vec<u8>> {
    tables.iter().find(|(t, _)| t == tag).map(|(_, d)| d)
}

fn set(tables: &mut Tables, tag: &[u8; 4], data: Vec<u8>) {
    match tables.iter_mut().find(|(t, _)| t == tag) {
        Some(slot) => slot.1 = data,
        None => tables.push((*tag, data)),
    }
}

/// `convert_ttf_to_otf`: new table set with glyf/loca replaced by CFF.
pub(crate) fn convert_ttf_to_otf(tables: &Tables) -> Result<Tables, ConvertError> {
    let fail = |m: String| ConvertError::transform(m);
    let head = table(tables, b"head").ok_or_else(|| fail("head table not found".into()))?;
    let maxp = table(tables, b"maxp").ok_or_else(|| fail("maxp table not found".into()))?;
    let loca = table(tables, b"loca").ok_or_else(|| fail("loca table not found".into()))?;
    let glyf = table(tables, b"glyf").ok_or_else(|| fail("glyf table not found".into()))?;
    if head.len() < 52 || maxp.len() < 6 {
        return Err(fail("Insufficient data for head/maxp".into()));
    }
    let loc_format = i32::from(i16::from_be_bytes([head[50], head[51]]));
    let num_glyphs = usize::from(u16::from_be_bytes([maxp[4], maxp[5]]));
    let g = glyf::GlyfTable::new(glyf, loca, loc_format, num_glyphs).map_err(fail)?;
    let mut charstrings = Vec::with_capacity(num_glyphs);
    for id in 0..num_glyphs {
        charstrings.push(cff::charstring(&g.outline(id).map_err(fail)?));
    }

    let name_tbl = table(tables, b"name").map(|d| name::Name::parse(d));
    let font_name = match name_tbl.as_ref().and_then(|n| n.english(1)) {
        Some(n) => n,
        // Ruby falls back to a UTF-8 "UnnamedFont", which IndexBuilder rejects.
        None => return Err(fail("item 0 must have BINARY encoding, got: UTF-8".into())),
    };
    let cff_data = cff::cff_table(&charstrings, font_name);

    let mut out: Tables = tables
        .iter()
        .filter(|(t, _)| t != b"glyf" && t != b"loca")
        .cloned()
        .collect();
    set(&mut out, b"CFF ", cff_data);
    let mut new_maxp = 0x0000_5000u32.to_be_bytes().to_vec();
    new_maxp.extend_from_slice(&(num_glyphs as u16).to_be_bytes());
    set(&mut out, b"maxp", new_maxp);
    let mut new_head = head.clone();
    new_head[50..52].fill(0);
    set(&mut out, b"head", new_head);
    Ok(out)
}

/// The Ruby production validator rules that fire on converted output
/// (`Output validation failed: ...`), in Ruby's check order.
pub(crate) fn validate_output(tables: &Tables) -> Result<(), ConvertError> {
    let mut errors: Vec<&str> = Vec::new();
    const NAME: &str = "Table 'name' failed validation";
    const CMAP: &str = "Table 'cmap' failed validation";
    if let Some(d) = table(tables, b"name") {
        let n = name::Name::parse(d);
        // basic: name_version, family_name, postscript_name
        if n.format > 1 {
            errors.push(NAME);
        }
        if n.english(1).is_none_or(|s| s.is_empty()) {
            errors.push(NAME);
        }
        let ps_ok = n.english(6).is_some_and(|s| {
            // Ruby /^[A-Za-z0-9-]+$/ matches any one line.
            s.split(|&b| b == b'\n')
                .any(|l| !l.is_empty() && l.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'-'))
        });
        if !ps_ok {
            errors.push(NAME);
        }
        // font_book: name_windows_encoding (error severity)
        if !n.has_combo(3, 1, 0x0409) {
            errors.push(NAME);
        }
    }
    if let Some(d) = table(tables, b"cmap") {
        // cmap_subtables, cmap_unicode_mapping, cmap_format4: an empty or
        // zero-subtable cmap fails all three.
        let num = if d.len() >= 4 {
            u16::from_be_bytes([d[2], d[3]])
        } else {
            0
        };
        if num == 0 {
            errors.extend([CMAP, CMAP, CMAP]);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConvertError::transform(format!(
            "Output validation failed: {}",
            errors.join(", ")
        )))
    }
}
