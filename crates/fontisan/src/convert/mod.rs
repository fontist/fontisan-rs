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
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_CLASS, message)
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
        let end = off.checked_add(len).ok_or_else(bad)?;
        let data = sfnt.get(off..end).ok_or_else(bad)?.to_vec();
        match out.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 = data,
            None => out.push((tag, data)),
        }
    }
    Ok((version, out))
}

fn has(tables: &Tables, tag: &[u8; 4]) -> bool {
    tables.iter().any(|(t, _)| t == tag)
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
    let tables = if is_variable {
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
            if has(&tables, b"glyf") && !want_glyf {
                // 1h-t2o: glyf -> CFF outline conversion.
                let converted = outline_t2o::convert_ttf_to_otf(&tables)?;
                let out = write_font(&converted, 0x4F54_544F);
                outline_t2o::validate_output(&converted)?;
                return Ok(out);
            }
            // 1h-o2t: CFF -> glyf conversion.
            if want_glyf && !has(&tables, b"glyf") && has(&tables, b"CFF ") {
                // Ruby's `OutlineConverter#validate_source_tables`.
                for tag in [b"head", b"hhea", b"maxp"] {
                    if !has(&tables, tag) {
                        return Err(ConvertError::transform(format!(
                            "Font missing required {} table",
                            String::from_utf8_lossy(tag)
                        )));
                    }
                }
                let ttf = outline_o2t::convert(&tables)?;
                return Ok(write_font(&ttf, 0x0001_0000));
            }
            // end 1h-o2t
            if has(&tables, b"glyf") != want_glyf {
                return Err(ConvertError::unsupported(
                    "CFF to TrueType outline conversion is card 1h-o2t",
                ));
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
