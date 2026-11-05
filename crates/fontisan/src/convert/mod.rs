//! Format conversion and collection-member extract (card 1h-writer).
//!
//! Scope: static fonts. woff/woff2 to a same-flavour ttf/otf (through the
//! Ruby `FontWriter` port), every static source to woff, and `UnpackCommand`
//! style member extract. Outline conversion (1h-outline), WOFF2 encode
//! (1h-woff2enc), collections (1h-coll) and variable instancing (1i) are
//! typed [`ConvertError`]s with class `fontisan::Unsupported`.

mod extract;
mod woff1;
mod writer;

// 1h-woff2enc: pub mod woff2;

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
}

impl Target {
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Ttf),
            1 => Some(Self::Otf),
            2 => Some(Self::Woff),
            3 => Some(Self::Woff2),
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

/// `Fontisan.convert(input, to: target)` for static fonts. `_timestamp` is
/// used by WOFF2 only (1h-woff2enc).
pub fn convert(input: &[u8], target: Target, _timestamp: i64) -> Result<Vec<u8>, ConvertError> {
    let format = sfnt::detect_format(input);
    let sfnt_bytes: Vec<u8>;
    let raw: &[u8] = match format {
        Some(Format::Ttf | Format::Otf) => input,
        Some(Format::Woff) => {
            sfnt_bytes = crate::woff::decode_woff(input)
                .map_err(|e| ConvertError::transform(e.to_string()))?;
            &sfnt_bytes
        }
        Some(Format::Woff2) => {
            sfnt_bytes = crate::woff::decode_woff2_ruby_padding(input)
                .map_err(|e| ConvertError::transform(e.to_string()))?;
            &sfnt_bytes
        }
        Some(Format::Ttc | Format::Otc) => {
            return Err(ConvertError::unsupported(
                "collection conversion is card 1h-coll",
            ));
        }
        _ => {
            return Err(ConvertError::new(
                ERROR_CLASS,
                "Conversion failed: unrecognised font format",
            ));
        }
    };
    let (_version, tables) = tables_of(raw)?;

    if has(&tables, b"fvar") {
        if has(&tables, b"CFF2") {
            return Err(ConvertError::transform(
                "Static instancing needs TrueType outlines with gvar (CFF2 variable fonts are not supported yet)",
            ));
        }
        return Err(ConvertError::unsupported(
            "variable font instancing is card 1i",
        ));
    }

    match target {
        Target::Woff => Ok(woff1::encode(&tables)),
        Target::Ttf | Target::Otf => {
            let want_glyf = target == Target::Ttf;
            if has(&tables, b"glyf") != want_glyf {
                return Err(ConvertError::unsupported(
                    "TrueType/CFF outline conversion is card 1h-outline",
                ));
            }
            let version = if want_glyf { 0x0001_0000 } else { 0x4F54_544F };
            Ok(write_font(&tables, version))
        }
        Target::Woff2 => Err(ConvertError::unsupported(
            "WOFF2 encode is card 1h-woff2enc",
        )),
    }
}
