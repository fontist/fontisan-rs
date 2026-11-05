//! glyf glyph ranges located through loca, as Ruby's `Tables::Loca` does it.
use crate::sfnt::error::{Error, ErrorClass};

fn corrupted(msg: String) -> Error {
    Error::new(ErrorClass::CorruptedTable, msg)
}

/// Parse loca for `num_glyphs` and slice `glyf`. A glyph whose range starts at
/// or past the end of `glyf` is empty and one running past it is cut short, like
/// Ruby's `byteslice(...) || ""`.
pub(super) fn glyphs<'a>(
    glyf: &'a [u8],
    loca: &[u8],
    index_to_loc_format: i16,
    num_glyphs: u16,
) -> Result<Vec<&'a [u8]>, Error> {
    if index_to_loc_format != 0 && index_to_loc_format != 1 {
        return Err(Error::new(
            ErrorClass::Argument,
            format!("indexToLocFormat must be 0 (short) or 1 (long), got: {index_to_loc_format}"),
        ));
    }
    if num_glyphs < 1 {
        return Err(Error::new(
            ErrorClass::Argument,
            format!("numGlyphs must be >= 1, got: {num_glyphs}"),
        ));
    }
    let count = usize::from(num_glyphs) + 1;
    let (width, kind, scale) = if index_to_loc_format == 0 {
        (2, "short", 2)
    } else {
        (4, "long", 1)
    };
    let mut offsets = Vec::with_capacity(count);
    for i in 0..count {
        let b = loca.get(i * width..(i + 1) * width).ok_or_else(|| {
            corrupted(format!("Insufficient data for {kind} offset at index {i}"))
        })?;
        let v = if width == 2 {
            u64::from(u16::from_be_bytes([b[0], b[1]]))
        } else {
            u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        offsets.push(v * scale);
    }
    for (i, w) in offsets.windows(2).enumerate() {
        if w[1] < w[0] {
            return Err(corrupted(format!(
                "Offsets are not monotonically increasing: offset[{i}]={}, offset[{}]={}",
                w[0],
                i + 1,
                w[1]
            )));
        }
    }
    Ok(offsets
        .windows(2)
        .map(|w| {
            let start = usize::try_from(w[0]).unwrap_or(usize::MAX);
            let len = usize::try_from(w[1] - w[0]).unwrap_or(usize::MAX);
            match glyf.get(start..) {
                Some(rest) => &rest[..len.min(rest.len())],
                None => &[][..],
            }
        })
        .collect())
}
