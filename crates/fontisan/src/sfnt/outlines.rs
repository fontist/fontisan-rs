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
mod points;

use super::error::{Error, ErrorClass};
use super::font::Font;
pub use points::{Contour, GlyphOutline, Point};
use sha2::{Digest, Sha256};

use crate::convert::ConvertError;
use crate::json::Json;

/// The facade's `outline_hashes` for face `index` of `input` (fontisan#192
/// gap 2): `{"kind": "glyf"|"cff"|"cff2"|null, "hashes": [hex, ...]}`, or the
/// facade's load error (the read ops' Ruby class + message surface).
pub fn outline_hashes(input: &[u8], index: usize) -> Result<Json, ConvertError> {
    let surface = |e: Error| ConvertError::new(e.class.ruby_name(), e.message.clone());
    let font = Font::load(input, index, super::Options::default()).map_err(surface)?;
    let Some(outlines) = font.outlines().map_err(surface)? else {
        return Ok(Json::Obj(vec![("kind", Json::Null), ("hashes", Json::Arr(Vec::new()))]));
    };
    // The facade never serves CFF2 here: Ruby's `raw_table` cannot fetch the
    // CFF2 table, so `outline_data` falls through to nil (the read docs keep
    // the spec-based CFF2 walk for the goldens, TRIAGE: CFF2-PARSER — this
    // op matches the facade instead).
    if outlines.kind == OutlineKind::Cff2 {
        return Ok(Json::Obj(vec![("kind", Json::Null), ("hashes", Json::Arr(Vec::new()))]));
    }
    let hashes = outlines.hashes().iter().map(|h| Json::Str(hex(h))).collect();
    Ok(Json::Obj(vec![
        ("kind", Json::Str(outlines.kind.as_str().to_string())),
        ("hashes", Json::Arr(hashes)),
    ]))
}

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

    /// Decodes one glyph's outline points (TrueType glyf only: contours of
    /// on/off-curve points in font units, composites resolved). CFF and CFF2
    /// outlines refuse typed until charstring decoding exists; a glyph id out
    /// of range and a malformed glyph are `CorruptedTableError`s.
    pub fn decode_points(&self, glyph_id: usize) -> Result<Option<GlyphOutline>, Error> {
        match self.kind {
            OutlineKind::Glyf => points::decode_glyph(&self.glyphs, glyph_id, 0),
            kind => Err(Error::new(
                ErrorClass::Unsupported,
                format!(
                    "{} outline points are not decoded (byte ranges only)",
                    kind.as_str()
                ),
            )),
        }
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
        self.digest_with(&mut GlyphDigests::default())
    }

    /// [`Self::digest`] through `cache`, so glyph data shared between faces of a
    /// collection (same location in the file) is hashed once.
    pub fn digest_with(&self, cache: &mut GlyphDigests) -> [u8; 32] {
        let mut h = Sha256::new();
        for g in &self.glyphs {
            h.update(cache.hash(g));
        }
        h.finalize().into()
    }
}

/// Per-glyph SHA-256 memoised by the glyph's location in the input buffer.
#[derive(Default)]
pub struct GlyphDigests(std::collections::HashMap<(usize, usize), [u8; 32]>);

impl GlyphDigests {
    fn hash(&mut self, glyph: &[u8]) -> [u8; 32] {
        // Every glyph slice borrows from the one input buffer, so address+length
        // identifies its bytes. Empty glyphs all hash the empty string.
        if glyph.is_empty() {
            return Sha256::digest(glyph).into();
        }
        *self
            .0
            .entry((glyph.as_ptr() as usize, glyph.len()))
            .or_insert_with(|| Sha256::digest(glyph).into())
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
        self.outline_summary_with(&mut GlyphDigests::default())
    }

    /// [`Self::outline_summary`] sharing glyph digests across faces of one read.
    pub fn outline_summary_with(&self, cache: &mut GlyphDigests) -> OutlineSummary {
        match self.outlines() {
            Ok(Some(o)) => OutlineSummary {
                kind: Some(o.kind),
                count: o.count(),
                digest: Some(hex(&o.digest_with(cache))),
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

impl Font<'_> {
    /// The audit's `ot_cff` view of `font.table("CFF ")`: the CharStrings INDEX
    /// count, or `Ok(None)` when the table is absent/unreadable. A malformed
    /// table is the `Err` Ruby's `Tables::Cff.read` raises; a CharStrings INDEX
    /// that fails to parse counts as 0 (Ruby rescues it in `glyph_count_from_cff`).
    pub fn cff_charstring_count(&self) -> Result<Option<usize>, Error> {
        let Some(Some(data)) = self.available("CFF ")? else {
            return Ok(None);
        };
        match cff::glyphs(data) {
            Ok(g) => Ok(Some(g.len())),
            Err(e)
                if e.class == ErrorClass::CorruptedTable
                    && e.message.starts_with("Failed to parse CharStrings INDEX") =>
            {
                Ok(Some(0))
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_glyphs_hash_once_and_the_digest_is_unchanged() {
        let buf: Vec<u8> = (0..64u8).collect();
        let glyphs = |idx: &[(usize, usize)]| Outlines {
            kind: OutlineKind::Cff,
            glyphs: idx.iter().map(|&(a, b)| &buf[a..b]).collect(),
        };
        // Two "faces" sharing glyph locations 0..10 and 10..30, one distinct each.
        let a = glyphs(&[(0, 10), (10, 30), (30, 40), (5, 5)]);
        let b = glyphs(&[(0, 10), (10, 30), (40, 64), (5, 5)]);
        let mut cache = GlyphDigests::default();
        let (da, db) = (a.digest_with(&mut cache), b.digest_with(&mut cache));
        assert_eq!(da, a.digest());
        assert_eq!(db, b.digest());
        assert_ne!(da, db);
        // 5 distinct non-empty locations; the empty glyph is never cached.
        assert_eq!(cache.0.len(), 4);
    }
}
