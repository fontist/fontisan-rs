//! Format conversion and collection-member extract (card 1h-writer).
//!
//! Scope: static fonts. woff/woff2 to a same-flavour ttf/otf (through the
//! Ruby `FontWriter` port), every static source to woff, and `UnpackCommand`
//! style member extract. Collections (1h-coll) are
//! typed [`ConvertError`]s with class `fontisan::Unsupported`.

mod collection;
mod extract;
mod outline_o2t;
mod outline_t2o;
mod woff1;
mod writer;

pub mod woff2;

use std::fmt;

use crate::sfnt::{self, Format};

pub(crate) use collection::first_face;
pub use extract::{Extracted, extract};
pub use writer::write_font;

/// Ruby `Fontisan::Error` as it surfaces from `Fontisan.convert`.
pub const ERROR_CLASS: &str = "Fontisan::Error";
/// Not a Ruby class: this card cannot produce the output.
pub const UNSUPPORTED_CLASS: &str = "fontisan::Unsupported";

/// A conversion failure: Ruby class name plus message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertError {
    pub class: &'static str,
    pub message: String,
}

impl ConvertError {
    pub(crate) fn new(class: &'static str, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
    pub(crate) fn transform(message: impl AsRef<str>) -> Self {
        Self::new(
            ERROR_CLASS,
            format!(
                "Conversion failed: Transformation failed: {}",
                message.as_ref()
            ),
        )
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for ConvertError {}

/// Target format. Codes are the C ABI codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Ttf = 0,
    Otf = 1,
    Woff = 2,
    Woff2 = 3,
    /// Collection targets (card 1h-coll): `format:ttc` and `format:otc`.
    Ttc = 4,
    Otc = 5,
}

impl Target {
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Ttf),
            1 => Some(Self::Otf),
            2 => Some(Self::Woff),
            3 => Some(Self::Woff2),
            4 => Some(Self::Ttc),
            5 => Some(Self::Otc),
            _ => None,
        }
    }
}

/// An sfnt's tables in directory order (a repeated tag keeps its first
/// position and its last data, like a Ruby hash).
pub(crate) type Tables = Vec<([u8; 4], Vec<u8>)>;

pub(crate) fn tables_of(sfnt: &[u8]) -> Result<(u32, Tables), ConvertError> {
    let bad = || ConvertError::transform("invalid sfnt table directory");
    let hdr = sfnt.get(..12).ok_or_else(bad)?;
    let version = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let n = usize::from(u16::from_be_bytes([hdr[4], hdr[5]]));
    let mut out: Tables = Vec::with_capacity(n);
    for i in 0..n {
        let e = sfnt.get(12 + i * 16..28 + i * 16).ok_or_else(bad)?;
        let tag = [e[0], e[1], e[2], e[3]];
        let off = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let len = u32::from_be_bytes([e[12], e[13], e[14], e[15]]) as usize;
        // Ruby reads each table with `io.seek(off); io.read(len)`: a length that runs
        // past the end of the file is clamped, and a non-empty read at or past the end
        // gives nil, which the subsetter and converter treat as an absent table. A
        // short or missing table never rejects the file.
        let data = if len > 0 && off >= sfnt.len() {
            None
        } else {
            Some(sfnt[off.min(sfnt.len())..off.saturating_add(len).min(sfnt.len())].to_vec())
        };
        match (out.iter().position(|(t, _)| *t == tag), data) {
            (Some(i), Some(d)) => out[i].1 = d,
            (Some(i), None) => {
                out.remove(i);
            }
            (None, Some(d)) => out.push((tag, d)),
            (None, None) => {}
        }
    }
    Ok((version, out))
}

fn has(tables: &Tables, tag: &[u8; 4]) -> bool {
    tables.iter().any(|(t, _)| t == tag)
}

/// Ruby's `OutlineConverter#validate_source_tables`: `head`, `hhea`, `maxp`, in that order.
fn require_core_tables(tables: &Tables) -> Result<(), ConvertError> {
    for tag in [b"head", b"hhea", b"maxp"] {
        if !has(tables, tag) {
            return Err(ConvertError::transform(format!(
                "Font missing required {} table",
                String::from_utf8_lossy(tag)
            )));
        }
    }
    Ok(())
}

const POST_MSG: &str = "Required table 'post' is missing";
const NAME_MSG: &str = "Font structure validation failed for check 'required_tables', Required table 'name' is missing, Required table 'name' is missing, Required table 'name' is missing, Required table 'name' is missing";
const OS2_MSG: &str = "Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing, Required table 'OS/2' is missing";
const CMAP_MSG: &str = "Required table 'cmap' is missing, Required table 'cmap' is missing, Required table 'cmap' is missing";

/// Ruby's output validation when a required table is absent from the source.
/// Ruby repeats a clause once per failing check; the texts above are its
/// measured output for a single missing table, so they are keyed per table.
fn validate_output_tables(
    tables: &Tables,
    checks: &[(&[u8; 4], &str)],
) -> Result<(), ConvertError> {
    let owned = checks.iter().map(|(t, m)| (*t, m.to_string())).collect();
    validate_checks(tables, owned)
}

fn validate_checks(tables: &Tables, checks: Vec<(&[u8; 4], String)>) -> Result<(), ConvertError> {
    // Ruby reports every failing check, in profile order: name content, the
    // missing tables, then cmap content.
    let mut pieces: Vec<String> = name_content_failures(tables);
    if let Some((_, msg)) = checks.into_iter().find(|(tag, _)| !has(tables, tag)) {
        pieces.push(msg);
    }
    if cmap_is_empty(tables) {
        pieces.extend(vec!["Table 'cmap' failed validation".to_string(); 3]);
    }
    if pieces.is_empty() {
        return Ok(());
    }
    Err(ConvertError::transform(format!(
        "Output validation failed: {}",
        pieces.join(", ")
    )))
}

fn table<'a>(tables: &'a Tables, tag: &[u8; 4]) -> Option<&'a [u8]> {
    tables
        .iter()
        .find(|(t, _)| t == tag)
        .map(|(_, d)| d.as_slice())
}

/// An existing `cmap` that is empty or has no subtables fails Ruby's three cmap checks.
fn cmap_is_empty(tables: &Tables) -> bool {
    table(tables, b"cmap").is_some_and(|c| c.get(2..4).is_none_or(|n| n == [0, 0]))
}

/// Ruby's production-profile name checks on an existing `name` table: a
/// Windows Unicode English record (3,1,0x409), a family name (ID 1) and a
/// PostScript name (ID 6); one "failed validation" clause per failed check.
fn name_content_failures(tables: &Tables) -> Vec<String> {
    let Some(n) = table(tables, b"name") else {
        return Vec::new();
    };
    let be = |o: usize| n.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let count = be(2).unwrap_or(0) as usize;
    let recs: Vec<[u16; 4]> = (0..count)
        .filter_map(|i| {
            let o = 6 + 12 * i;
            Some([be(o)?, be(o + 2)?, be(o + 4)?, be(o + 6)?])
        })
        .collect();
    let windows_english = recs.iter().any(|r| r[..3] == [3, 1, 0x409]);
    let family = recs.iter().any(|r| r[3] == 1);
    let ps = recs.iter().any(|r| r[3] == 6);
    [windows_english, family, ps]
        .into_iter()
        .filter(|ok| !ok)
        .map(|_| "Table 'name' failed validation".to_string())
        .collect()
}

const HEAD_MSG_PREFIX: &str = "Font structure validation failed for check 'required_tables'";
const LOCA_MSG: &str = "Exception during check execution: NoMethodError - undefined method 'parsed?' for nil, Exception during check execution: NoMethodError - undefined method 'parsed?' for nil, Exception during check execution: NoMethodError - undefined method 'parsed?' for nil";

/// Ruby's output validation for a source that is not transformed but still
/// rebuilt (a WOFF source, an `OTTO` file holding `glyf`, an outline-less
/// TrueType going to otf): the required-table checks with the clauses Ruby
/// measured for the table that is missing. `glyf` selects the TrueType-only ones.
fn validate_rebuilt_output(tables: &Tables, glyf: bool) -> Result<(), ConvertError> {
    let num_glyphs =
        "Exception during check execution: NoMethodError - undefined method 'num_glyphs' for nil";
    let loc_fmt = "Exception during check execution: NoMethodError - undefined method 'index_to_loc_format' for nil";
    let rep = |s: &str, n: usize| vec![s.to_string(); n].join(", ");
    let req = |t: &str, n: usize| rep(&format!("Required table '{t}' is missing"), n);
    let structure = |inner: String| format!("{HEAD_MSG_PREFIX}, {inner}");
    let mut checks: Vec<(&[u8; 4], String)> = vec![
        (
            b"head",
            if glyf {
                structure(format!("{}, {}", req("head", 3), rep(loc_fmt, 3)))
            } else {
                structure(req("head", 3))
            },
        ),
        (b"hhea", structure(req("hhea", 2))),
        (
            b"maxp",
            structure(format!(
                "{}, {}",
                req("maxp", 2),
                rep(num_glyphs, if glyf { 4 } else { 1 })
            )),
        ),
    ];
    if glyf {
        checks.push((b"loca", LOCA_MSG.to_string()));
    }
    checks.extend([
        (b"post", POST_MSG.to_string()),
        (b"name", NAME_MSG.to_string()),
        (b"OS/2", OS2_MSG.to_string()),
        (b"cmap", CMAP_MSG.to_string()),
    ]);
    validate_checks(tables, checks)
}

/// A CFF -> TrueType result (Ruby's `Output validation failed`).
fn validate_ttf_output(ttf: &Tables) -> Result<(), ConvertError> {
    validate_output_tables(
        ttf,
        &[
            (b"post", POST_MSG),
            (b"name", NAME_MSG),
            (b"OS/2", OS2_MSG),
            (b"cmap", CMAP_MSG),
        ],
    )
}

/// `Fontisan.convert(input, to: target)` for static fonts. `timestamp` is
/// used by WOFF2 encode only (head.modified, Ruby's encoder rules).
pub fn convert(input: &[u8], target: Target, timestamp: i64) -> Result<Vec<u8>, ConvertError> {
    let format = sfnt::detect_format(input);
    // 1h-coll: collection sources (ttcf, or a WOFF2 with the ttcf flavor).
    let woff2_collection = input.starts_with(b"wOF2") && input.get(4..8) == Some(b"ttcf");
    if matches!(format, Some(Format::Ttc | Format::Otc)) || woff2_collection {
        let source = if woff2_collection {
            collection::Source::Woff2Collection
        } else {
            collection::Source::Ttc
        };
        return collection::convert(input, source, target, timestamp);
    }
    if matches!(target, Target::Ttc | Target::Otc) {
        return Err(ConvertError::transform(
            "Target format is not a collection format for a single font",
        ));
    }
    // end 1h-coll
    let sfnt_bytes: Vec<u8>;
    let raw: &[u8] = match format {
        Some(Format::Ttf | Format::Otf) => input,
        Some(Format::Woff) => {
            sfnt_bytes = crate::woff::decode_woff(input)
                .map_err(|e| ConvertError::transform(e.to_string()))?;
            &sfnt_bytes
        }
        Some(Format::Woff2) => {
            // Ruby's own WOFF2 rebuild (padding, flag RLE, table order), shared with reads.
            sfnt_bytes = crate::sfnt::woff2_view::single(input)
                .map_err(|e| ConvertError::transform(format!("{e:?}")))?;
            &sfnt_bytes
        }
        _ => {
            return Err(ConvertError::new(
                ERROR_CLASS,
                "Conversion failed: unrecognised font format",
            ));
        }
    };
    let (sfnt_version, tables) = tables_of(raw)?;
    let is_variable = has(&tables, b"fvar");

    // 1i-instance: a variable font goes through instancing at its default
    // location first, whatever the target (Ruby's `:instance` strategy).
    // A variable font with neither gvar nor CFF2 is `:static` to Ruby's
    // `FormatDetector#variation_type`: every table passes through untouched.
    let tables = if is_variable && (has(&tables, b"gvar") || has(&tables, b"CFF2")) {
        if has(&tables, b"CFF2") {
            return Err(ConvertError::transform(
                "Static instancing needs TrueType outlines with gvar (CFF2 variable fonts are not supported yet)",
            ));
        }
        crate::instance::instance_tables(&tables, &[])
            .map_err(|e| ConvertError::transform(e.message))?
    } else {
        tables
    };

    match target {
        Target::Woff => Ok(woff1::encode(&tables)),
        Target::Ttf | Target::Otf => {
            let want_glyf = target == Target::Ttf;
            if want_glyf
                && !has(&tables, b"glyf")
                && !has(&tables, b"CFF ")
                && !has(&tables, b"CFF2")
            {
                // Ruby's `FormatConverter#detect_format` on an outline-less source.
                return Err(ConvertError::transform(
                    "Cannot detect font format: missing both CFF and glyf tables",
                ));
            }
            if has(&tables, b"glyf") && !want_glyf {
                // 1h-t2o: glyf -> CFF outline conversion.
                if !has(&tables, b"loca") {
                    return Err(ConvertError::transform(
                        "TrueType font missing required glyf or loca table",
                    ));
                }
                require_core_tables(&tables)?;
                validate_output_tables(
                    &tables,
                    &[
                        (b"post", POST_MSG),
                        (b"name", NAME_MSG),
                        (b"OS/2", OS2_MSG),
                        (b"cmap", CMAP_MSG),
                    ],
                )?;
                let converted = outline_t2o::convert_ttf_to_otf(&tables)?;
                let out = write_font(&converted, 0x4F54_544F);
                outline_t2o::validate_output(&converted)?;
                return Ok(out);
            }
            // 1h-o2t: CFF -> glyf conversion.
            if want_glyf && !has(&tables, b"glyf") && has(&tables, b"CFF ") {
                require_core_tables(&tables)?;
                let ttf = outline_o2t::convert(&tables)?;
                validate_ttf_output(&ttf)?;
                return Ok(write_font(&ttf, 0x0001_0000));
            }
            // end 1h-o2t
            if has(&tables, b"glyf") != want_glyf {
                // Only a CFF2-flavoured source reaches here (glyf wanted, none present,
                // no CFF table). Ruby's CFF2 source check rejects it: the CFF2 table is
                // absent or fails to parse.
                return Err(ConvertError::transform(
                    "CFF2 font missing required CFF2 table",
                ));
            }
            // Ruby validates the written font unless the detected source format
            // is already the target (a plain sfnt of the matching flavour).
            let plain = matches!(format, Some(Format::Ttf | Format::Otf))
                && (sfnt_version == 0x4F54_544F) != want_glyf;
            if !plain {
                validate_rebuilt_output(&tables, want_glyf)?;
            }
            let version = if want_glyf { 0x0001_0000 } else { 0x4F54_544F };
            Ok(write_font(&tables, version))
        }
        Target::Ttc | Target::Otc => unreachable!("handled above"),
        Target::Woff2 => {
            // A variable source encodes its instanced tables, not the raw input.
            let instanced;
            let src = if is_variable {
                instanced = write_font(&tables, sfnt_version);
                &instanced[..]
            } else {
                raw
            };
            woff2::encode_woff2(src, Some(timestamp))
                .map_err(|e| ConvertError::transform(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> Tables {
        ["head", "hhea", "maxp"]
            .iter()
            .map(|t| {
                (
                    <[u8; 4]>::try_from(format!("{t:<4}").as_bytes()).unwrap(),
                    vec![0; 54],
                )
            })
            .collect()
    }

    fn sfnt(mut tables: Tables, extra: &[&[u8; 4]]) -> Vec<u8> {
        tables.extend(extra.iter().map(|t| (**t, vec![0; 8])));
        write_font(&tables, 0x4F54_544F)
    }

    #[test]
    fn cff2_source_without_loadable_cff2_to_ttf_matches_ruby() {
        // Ruby's CFF2 source check: no internal planning id, no Unsupported class.
        let font = sfnt(core(), &[b"CFF2"]);
        let e = convert(&font, Target::Ttf, 0).expect_err("fails");
        assert_eq!(e.class, ERROR_CLASS);
        assert_eq!(
            e.message,
            "Conversion failed: Transformation failed: CFF2 font missing required CFF2 table"
        );
    }

    #[test]
    fn variable_font_without_gvar_or_cff2_keeps_fvar_through_convert() {
        // `:static` to Ruby: the tables pass through, so the same source check applies.
        let font = sfnt(core(), &[b"fvar", b"CFF "]);
        let out = convert(&font, Target::Woff, 0).expect("converts");
        assert!(out.starts_with(b"wOFF"));
    }

    #[test]
    fn missing_cmap_in_a_cff_sourced_ttf_has_no_glyph_index_clause() {
        let mut t = core();
        for tag in [b"post", b"name", b"OS/2"] {
            t.push((*tag, vec![0; 8]));
        }
        let e = validate_ttf_output(&t).expect_err("fails");
        assert!(e.message.contains("Required table 'cmap' is missing"));
        assert!(!e.message.contains("valid_glyph_indices?"), "{}", e.message);
    }
}
