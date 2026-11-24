//! Apple dfont (fontisan#192 gap 5): Ruby's `Parsers::DfontParser` over the
//! resource fork in the data fork. A dfont is detected by the sfnt sniffer
//! (the resource fork header starts `00 00 01 00`) plus the structural check
//! (`dfont?`: at least the 16-byte header, data offset positive, map after
//! data); the sfnt resources live behind the resource map.
//!
//! Error surfaces, measured against the Ruby tier: a structurally valid
//! header whose map walk runs off the file raises `NoMethodError` inside
//! Ruby (`io.read(2).unpack1` on nil) and surfaces through the facade's
//! malformed-font wrapper; the two explicit `InvalidFontError`s carry the
//! parser's own messages.

use crate::sfnt::{Error, ErrorClass};

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorClass::InvalidFont, msg.into())
}

// Ruby: the map walk hits `nil.unpack1` on any short read, and the facade's
// malformed-font wrapper remaps the NoMethodError.
fn malformed_walk() -> Error {
    invalid("Malformed font data (NoMethodError): undefined method 'unpack1' for nil")
}

/// The structural check (`DfontParser.dfont?`): at least the 16-byte header,
/// data offset positive, map offset after the data offset.
pub fn is_dfont(input: &[u8]) -> bool {
    if input.len() < 16 {
        return false;
    }
    let be32 = |at: usize| {
        input
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    match (be32(0), be32(4)) {
        (Some(data_offset), Some(map_offset)) => {
            data_offset > 0 && map_offset > data_offset
        }
        _ => false,
    }
}

/// One sfnt resource: its id and the absolute offset of its 4-byte length
/// prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    #[allow(dead_code)]
    pub id: u16,
    pub at: usize,
}

/// The sfnt resources in reference order.
pub fn resources(input: &[u8]) -> Result<Vec<Resource>, Error> {
    let be32 = |at: usize| -> Result<u32, Error> {
        input
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(malformed_walk)
    };
    let map_offset = be32(4)? as usize;
    let u16_at = |at: usize| -> Result<u16, Error> {
        input
            .get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or_else(malformed_walk)
    };
    // Resource map: 16-byte header copy, 4+2 reserved, 2 attributes, then
    // the type-list offset (relative to the map).
    let type_list_at = map_offset + usize::from(u16_at(map_offset + 24)?);
    let type_count = usize::from(u16_at(type_list_at)?) + 1;

    let mut sfnt: Option<(usize, usize)> = None; // (resource count, ref list offset)
    let mut at = type_list_at + 2;
    for _ in 0..type_count {
        let tag = input.get(at..at + 4).ok_or_else(malformed_walk)?;
        let count = usize::from(u16_at(at + 4)?) + 1;
        let ref_offset = u16_at(at + 6)? as usize;
        if tag == b"sfnt" {
            sfnt = Some((count, type_list_at + ref_offset));
            break;
        }
        at += 8;
    }
    let Some((count, refs_at)) = sfnt else {
        return Err(invalid("No sfnt resources found in dfont file"));
    };

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = refs_at + i * 12;
        let id = u16_at(at)?;
        // id u16, name offset i16, attributes u8, 24-bit data offset, handle u32.
        let b = input.get(at + 5..at + 8).ok_or_else(malformed_walk)?;
        let data_offset =
            (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]);
        out.push(Resource {
            id,
            at: be32(0)? as usize + data_offset,
        });
    }
    Ok(out)
}

/// Face `index`'s sfnt bytes as a slice of `input` (the 4-byte length prefix
/// skipped), with Ruby's index error.
pub fn member(input: &[u8], index: usize) -> Result<(usize, usize), Error> {
    let list = resources(input)?;
    let Some(r) = list.get(index) else {
        return Err(invalid(format!(
            "Font index {index} out of range (dfont has {} fonts)",
            list.len()
        )));
    };
    let be32 = |at: usize| -> Result<u32, Error> {
        input
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(malformed_walk)
    };
    let len = be32(r.at)? as usize;
    Ok((r.at + 4, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_corpus_dfont_like_ruby() {
        let Ok(bytes) = std::fs::read(
            "/Users/mulgogi/src/fontist/fontisan-rs/.corpus/fontisan/corpus/fonts/TestDFONT.dfont",
        ) else {
            return;
        };
        let rs = resources(&bytes).expect("parses");
        assert_eq!(rs.len(), 1, "Ruby: sfnt_count == 1");
    }
}

