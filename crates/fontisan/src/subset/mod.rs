//! `Subset::Builder` with the web / pdf / minimal profiles (card 1i-subset).
//!
//! Takes a glyph id list (the facade's unicode/text to gid step stays in the
//! facade) and a profile, and reproduces `Fontisan::Subset::Builder#build` byte
//! for byte: closure over composite glyphs, compacted glyph ids, per-table
//! strategies (glyf/loca/head/hhea/hmtx/maxp/cmap rebuilt, name/OS/2/post and
//! the layout tables passed through) and `FontWriter` assembly. `head.modified`
//! is preserved. Out of this card's scope, typed `fontisan::Unsupported`: a
//! profile table the port does not cover for the given font (CFF / CFF2 /
//! CBDT / CBLC), collections.

mod cff;
mod cmap;
mod glyf;

use std::collections::BTreeSet;
use std::fmt;

use crate::convert::{Tables, tables_of, write_font};
use crate::sfnt::{self, Format};

pub use crate::convert::UNSUPPORTED_CLASS;

/// A subset failure: Ruby class name plus message (as raised by Ruby).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsetError {
    pub class: &'static str,
    pub message: String,
}

impl SubsetError {
    pub(crate) fn new(class: &'static str, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
    pub(crate) fn arg(message: impl Into<String>) -> Self {
        Self::new("ArgumentError", message)
    }
    pub(crate) fn corrupted(message: impl Into<String>) -> Self {
        Self::new("Fontisan::CorruptedTableError", message)
    }
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_CLASS, message)
    }
}

impl fmt::Display for SubsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for SubsetError {}

const PDF: [&str; 9] = [
    "cmap", "head", "hhea", "hmtx", "maxp", "name", "post", "loca", "glyf",
];
const MINIMAL: [&str; 8] = [
    "cmap", "head", "hhea", "hmtx", "maxp", "name", "OS/2", "post",
];
const WEB: [&str; 16] = [
    "cmap", "head", "hhea", "hmtx", "maxp", "name", "OS/2", "post", "loca", "glyf", "CFF ", "CFF2",
    "GSUB", "GPOS", "CBDT", "CBLC",
];

/// `Options#validate!` then `Profile.for_name`.
fn profile_tables(profile: &str) -> Result<&'static [&'static str], SubsetError> {
    if !["pdf", "web", "minimal", "custom"].contains(&profile) {
        return Err(SubsetError::arg(format!(
            "Invalid profile '{profile}'. Must be one of: pdf, web, minimal, custom"
        )));
    }
    match profile {
        "pdf" => Ok(&PDF),
        "web" => Ok(&WEB),
        "minimal" => Ok(&MINIMAL),
        _ => Err(SubsetError::arg(format!(
            "Unknown profile '{profile}'. Valid profiles: full, minimal, pdf, web"
        ))),
    }
}

pub(crate) fn find<'a>(tables: &'a Tables, tag: &str) -> Option<&'a [u8]> {
    tables
        .iter()
        .find(|(t, _)| t == tag.as_bytes())
        .map(|(_, d)| d.as_slice())
}

/// Old glyph id to new glyph id (`GlyphMapping`, compacted).
pub(crate) struct Mapping {
    pub old_ids: Vec<u32>,
}

impl Mapping {
    pub fn new_id(&self, old: u32) -> Option<u32> {
        self.old_ids.binary_search(&old).ok().map(|i| i as u32)
    }
    pub fn size(&self) -> usize {
        self.old_ids.len()
    }
}

/// `Subset::Builder.new(font, glyph_ids, Options.new(profile:)).build` over the
/// font file bytes (face 0; woff and woff2 are decoded first).
pub fn subset(input: &[u8], glyph_ids: &[u32], profile: &str) -> Result<Vec<u8>, SubsetError> {
    let raw = decode(input)?;
    let (_version, tables) = tables_of(&raw).map_err(|e| SubsetError::new(e.class, e.message))?;
    build(&tables, glyph_ids, profile)
}

fn decode(input: &[u8]) -> Result<Vec<u8>, SubsetError> {
    let wrap = |e: String| {
        SubsetError::new(
            "Fontisan::Error",
            format!("Conversion failed: Transformation failed: {e}"),
        )
    };
    match sfnt::detect_format(input) {
        Some(Format::Ttf | Format::Otf) => Ok(input.to_vec()),
        Some(Format::Woff) => crate::woff::decode_woff(input).map_err(|e| wrap(e.to_string())),
        Some(Format::Woff2) => {
            // Ruby's own WOFF2 rebuild (as convert does): it accepts a file whose last
            // block lacks the 4-byte padding, which the strict decoder rejects.
            crate::sfnt::woff2_view::single(input).map_err(|e| wrap(format!("{e:?}")))
        }
        Some(Format::Ttc | Format::Otc | Format::Woff2Collection) => Err(SubsetError::unsupported(
            "collection subsetting is not part of card 1i-subset",
        )),
        _ => Err(SubsetError::new(
            "Fontisan::InvalidFontError",
            "unrecognised font format",
        )),
    }
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn build(tables: &Tables, glyph_ids: &[u32], profile: &str) -> Result<Vec<u8>, SubsetError> {
    let profile_tables = profile_tables(profile)?;
    if glyph_ids.is_empty() {
        return Err(SubsetError::arg("At least one glyph ID must be provided"));
    }
    for tag in ["head", "maxp"] {
        if find(tables, tag).is_none() {
            return Err(SubsetError::new(
                "Fontisan::MissingTableError",
                format!("Required table '{tag}' not found in font"),
            ));
        }
    }
    let maxp = find(tables, "maxp").unwrap_or(&[]);
    let num_glyphs =
        u32::from(be16(maxp, 4).ok_or_else(|| SubsetError::corrupted("maxp table is too short"))?);
    for &g in glyph_ids {
        if g >= num_glyphs {
            return Err(SubsetError::arg(format!(
                "Glyph ID {g} exceeds font's glyph count ({num_glyphs})"
            )));
        }
    }

    // calculate_closure (include_notdef, not retain_gids)
    let closure = glyf::closure(tables, glyph_ids, num_glyphs)?;
    let mapping = Mapping {
        old_ids: closure.into_iter().collect(),
    };

    let mut st = glyf::State::default();
    let mut out: Tables = Vec::new();
    for tag in profile_tables {
        let Some(data) = find(tables, tag) else {
            continue;
        };
        let sub = strategy(tables, &mapping, &mut st, tag, data, num_glyphs).map_err(|e| {
            if e.class == UNSUPPORTED_CLASS {
                e
            } else {
                SubsetError::new(
                    "Fontisan::SubsettingError",
                    format!("Failed to subset table '{tag}': {}", e.message),
                )
            }
        })?;
        out.push((tag_bytes(tag), sub));
    }
    let is_cff = out.iter().any(|(t, _)| t == b"CFF " || t == b"CFF2");
    let version = if is_cff { 0x4F54_544F } else { 0x0001_0000 };
    Ok(write_font(&out, version))
}

fn tag_bytes(tag: &str) -> [u8; 4] {
    let b = tag.as_bytes();
    let mut t = [b' '; 4];
    t[..b.len()].copy_from_slice(b);
    t
}

fn strategy(
    tables: &Tables,
    mapping: &Mapping,
    st: &mut glyf::State,
    tag: &str,
    data: &[u8],
    num_glyphs: u32,
) -> Result<Vec<u8>, SubsetError> {
    match tag {
        "maxp" => {
            let version = data
                .get(..4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            let len = if version == Some(0x0001_0000) { 32 } else { 6 };
            let mut d = data
                .get(..len)
                .ok_or_else(|| SubsetError::corrupted("maxp table is too short"))?
                .to_vec();
            d[4..6].copy_from_slice(&(mapping.size() as u16).to_be_bytes());
            Ok(d)
        }
        "hhea" => glyf::hhea(tables, mapping, data, num_glyphs),
        "hmtx" => glyf::hmtx(mapping, data, tables, num_glyphs),
        "glyf" => {
            st.build(tables, mapping, num_glyphs)?;
            Ok(st.glyf.clone())
        }
        "loca" => {
            st.build(tables, mapping, num_glyphs)?;
            Ok(st.loca(tables))
        }
        "head" => {
            if find(tables, "glyf").is_some() {
                st.build(tables, mapping, num_glyphs)?;
            }
            let mut d = data.to_vec();
            if let Some(b) = st.bbox {
                if d.len() < 44 {
                    return Err(SubsetError::corrupted("head table is too short"));
                }
                for (i, v) in b.iter().enumerate() {
                    d[36 + 2 * i..38 + 2 * i].copy_from_slice(&(*v as u16).to_be_bytes());
                }
            }
            Ok(d)
        }
        "cmap" => cmap::subset(data, mapping),
        "CFF " => cff::subset(tables, mapping, data, num_glyphs),
        "CFF2" | "CBDT" | "CBLC" => Err(SubsetError::unsupported(format!(
            "subsetting the {} table is not part of card 1i-subset",
            tag.trim_end()
        ))),
        _ => Ok(data.to_vec()),
    }
}

/// Glyph ids kept for a set of codepoints (the facade's `gids_for`), or the
/// facade's error. `name` is the file's basename.
pub fn glyph_ids_for(
    input: &[u8],
    codepoints: &[u32],
    name: &str,
) -> Result<Vec<u32>, SubsetError> {
    let font = sfnt::Font::from_bytes(input)
        .map_err(|e| SubsetError::new("Fontisan::Error", e.to_string()))?;
    // Ruby calls `unicode_mappings` on the cmap table: nil when it is absent.
    let map: std::collections::BTreeMap<u32, u32> = match font.cmap() {
        Ok(None) => {
            return Err(SubsetError::new(
                "NoMethodError",
                "undefined method 'unicode_mappings' for nil",
            ));
        }
        Ok(Some(c)) => c.mappings().into_iter().collect(),
        Err(_) => Default::default(),
    };
    let gids: BTreeSet<u32> = codepoints
        .iter()
        .filter_map(|c| map.get(c).copied())
        .collect();
    if gids.is_empty() {
        return Err(SubsetError::new(
            "Fontisan::Error",
            format!("None of the requested characters exist in {name}"),
        ));
    }
    Ok(gids.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> Vec<u8> {
        vec![0; 54]
    }
    fn maxp() -> Vec<u8> {
        let mut m = vec![0, 0, 0x50, 0, 0, 1];
        m.resize(6, 0);
        m
    }
    fn font(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let t: Tables = tables.iter().map(|(k, v)| (**k, v.clone())).collect();
        write_font(&t, 0x0001_0000)
    }
    fn err(r: Result<Vec<u8>, SubsetError>) -> (String, String) {
        let e = r.expect_err("subset should fail");
        (e.class.to_string(), e.message)
    }

    #[test]
    fn missing_hhea_reports_ruby_nil_method_error() {
        let f = font(&[
            (b"head", head()),
            (b"maxp", maxp()),
            (b"hmtx", vec![0; 4]),
            (b"glyf", vec![]),
            (b"loca", vec![0; 4]),
        ]);
        let (class, msg) = err(subset(&f, &[0], "web"));
        assert_eq!(class, "Fontisan::SubsettingError");
        assert_eq!(
            msg,
            "Failed to subset table 'hmtx': undefined method 'number_of_h_metrics' for nil"
        );
    }

    #[test]
    fn missing_loca_reports_required_table() {
        let f = font(&[
            (b"head", head()),
            (b"maxp", maxp()),
            (b"glyf", vec![]),
        ]);
        let (class, msg) = err(subset(&f, &[0], "web"));
        assert_eq!(class, "Fontisan::MissingTableError");
        assert_eq!(msg, "Required table 'loca' not found in font");
    }

    #[test]
    fn missing_cmap_reports_ruby_nil_method_error() {
        let f = font(&[(b"head", head()), (b"maxp", maxp())]);
        let e = glyph_ids_for(&f, &[97], "x.ttf").expect_err("no cmap");
        assert_eq!(e.class, "NoMethodError");
        assert_eq!(e.message, "undefined method 'unicode_mappings' for nil");
    }

    #[test]
    fn woff2_without_trailing_pad_is_accepted() {
        // A table-dropped WOFF2 whose last block is not padded to 4 bytes:
        // Ruby reads it, so subsetting must succeed too.
        let mut hhea = vec![0; 36];
        hhea[35] = 1;
        let mut w2 = None;
        for pad in 0..16usize {
            let sfnt = font(&[
                (b"head", head()),
                (b"maxp", maxp()),
                (b"hhea", hhea.clone()),
                (b"hmtx", vec![0; 4]),
                (b"glyf", vec![0; 12]),
                (b"loca", vec![0, 0, 0, 6]),
                (b"name", vec![1; pad]),
            ]);
            let enc = crate::convert::woff2::encode_woff2_with_quality(&sfnt, Some(0), 1).unwrap();
            let mut cut = enc.clone();
            while cut.len() % 4 == 0 && cut.last() == Some(&0) {
                cut.pop();
            }
            if cut.len() % 4 != 0 && crate::woff::decode_woff2(&cut).is_err() {
                w2 = Some(cut);
                break;
            }
        }
        let w2 = w2.expect("an unpadded woff2");
        assert!(crate::woff::decode_woff2_ruby_padding(&w2).is_err());
        let out = subset(&w2, &[0], "web").expect("subset of an unpadded woff2");
        assert!(tables_of(&out).unwrap().1.iter().any(|(t, _)| t == b"glyf"));
    }
}
