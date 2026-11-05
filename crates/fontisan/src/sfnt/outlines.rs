//! Outlines (card 1d): the glyf/loca, CFF and CFF2 glyph byte ranges and the
//! per-glyph hashes the Ruby facade's `outline_hashes` / `outlines` report.
//!
//! A glyph's "outline" here is its raw byte range, not a drawn path: the glyf
//! slice located through loca, or the CharStrings INDEX entry. The selection
//! order, the edge cases and the error classes follow `Fontisan::Reference`
//! (`outline_data`), including the spec-based CFF2 walk the goldens use in place
//! of Ruby's broken `Tables::Cff2` (TRIAGE: CFF2-PARSER).
mod cff;
mod cff2;
mod glyf;

use super::error::{Error, ErrorClass};
use super::font::Font;
use sha2::{Digest, Sha256};

/// Which outline table family a font's glyphs come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Glyf,
    Cff,
    Cff2,
}

impl OutlineKind {
    /// The facade's `kind` string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Glyf => "glyf",
            Self::Cff => "cff",
            Self::Cff2 => "cff2",
        }
    }
}

/// Every glyph's raw outline bytes, in glyph order, borrowed from the font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outlines<'a> {
    pub kind: OutlineKind,
    pub glyphs: Vec<&'a [u8]>,
}

impl Outlines<'_> {
    pub fn count(&self) -> usize {
        self.glyphs.len()
    }

    /// SHA-256 of each glyph's bytes (the empty glyph hashes the empty string).
    pub fn hashes(&self) -> Vec<[u8; 32]> {
        self.glyphs
            .iter()
            .map(|g| Sha256::digest(g).into())
            .collect()
    }

    /// SHA-256 over the concatenated 32-byte per-glyph hashes.
    pub fn digest(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        for g in &self.glyphs {
            h.update(Sha256::digest(g));
        }
        h.finalize().into()
    }
}

/// The facade's `outlines` entry: `kind`, `count`, `digest`, and the error class
/// name when the outline read raised (`kind` is then nil and `count` 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineSummary {
    pub kind: Option<OutlineKind>,
    pub count: usize,
    /// Lowercase hex, or `None` when there is no outline data.
    pub digest: Option<String>,
    pub error: Option<ErrorClass>,
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

impl Font<'_> {
    /// The font's glyph outlines, or `Ok(None)` when it has none the loading
    /// mode lets us read. glyf+loca win over CFF, which wins over CFF2 (the
    /// facade's order). A malformed table is an `Err` shaped like Ruby's.
    pub fn outlines(&self) -> Result<Option<Outlines<'_>>, Error> {
        let glyf = self.raw_table_data("glyf");
        let loca = self.raw_table_data("loca");
        if let (Some(glyf), Some(loca)) = (glyf, loca) {
            let head = self
                .head()?
                .ok_or_else(|| Error::new(ErrorClass::NoMethod, "undefined method for nil"))?;
            let maxp = self
                .maxp()?
                .ok_or_else(|| Error::new(ErrorClass::NoMethod, "undefined method for nil"))?;
            let glyphs = glyf::glyphs(glyf, loca, head.index_to_loc_format, maxp.num_glyphs)?;
            return Ok(Some(Outlines {
                kind: OutlineKind::Glyf,
                glyphs,
            }));
        }
        if let Some(data) = self.raw_table_data("CFF ") {
            return Ok(Some(Outlines {
                kind: OutlineKind::Cff,
                glyphs: cff::glyphs(data)?,
            }));
        }
        if let Some(data) = self.raw_table_data("CFF2") {
            return Ok(Some(Outlines {
                kind: OutlineKind::Cff2,
                glyphs: cff2::glyphs(data)?,
            }));
        }
        Ok(None)
    }

    /// `outline_summary`: never fails, a read error becomes `error`.
    pub fn outline_summary(&self) -> OutlineSummary {
        match self.outlines() {
            Ok(Some(o)) => OutlineSummary {
                kind: Some(o.kind),
                count: o.count(),
                digest: Some(hex(&o.digest())),
                error: None,
            },
            Ok(None) => OutlineSummary {
                kind: None,
                count: 0,
                digest: None,
                error: None,
            },
            Err(e) => OutlineSummary {
                kind: None,
                count: 0,
                digest: None,
                error: Some(e.class),
            },
        }
    }
}
