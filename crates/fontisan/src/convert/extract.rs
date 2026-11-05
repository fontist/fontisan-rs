//! Collection-member extract: `UnpackCommand` with `font_index`, writing the
//! member as `SfntFont#to_file` does (not through `FontWriter`).

use super::{ConvertError, ERROR_CLASS};
use crate::sfnt;

/// The extracted member and the file name Ruby gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub basename: String,
    pub data: Vec<u8>,
}

fn unpack_err(msg: impl AsRef<str>) -> ConvertError {
    ConvertError::new(
        ERROR_CLASS,
        format!("Collection unpacking failed: {}", msg.as_ref()),
    )
}

/// Extract member `index`; the file name carries the `.ttf` extension Ruby gives
/// a `.ttc` input (a `.otc` input would get `.otf`; the bytes carry no path).
pub fn extract(input: &[u8], index: usize) -> Result<Extracted, ConvertError> {
    if input.get(..4) != Some(b"ttcf") {
        return Err(unpack_err("Not a valid TTC/OTC file (invalid signature)"));
    }
    let be32 = |at: usize| -> Option<u32> {
        input
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let count = be32(8).ok_or_else(|| unpack_err("truncated collection header"))? as usize;
    if index >= count {
        return Err(ConvertError::new(
            "ArgumentError",
            format!("Font index {index} out of range (collection has {count} fonts)"),
        ));
    }
    let off =
        be32(12 + 4 * index).ok_or_else(|| unpack_err("truncated collection header"))? as usize;
    let hdr = input
        .get(off..off + 12)
        .ok_or_else(|| unpack_err("truncated font header"))?;
    let n = usize::from(u16::from_be_bytes([hdr[4], hdr[5]]));
    let dir = input
        .get(off + 12..off + 12 + 16 * n)
        .ok_or_else(|| unpack_err("truncated table directory"))?;

    let mut out = Vec::new();
    out.extend_from_slice(hdr);
    let mut head_entry_offset = None;
    for e in dir.chunks_exact(16) {
        out.extend_from_slice(&e[0..8]);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&e[12..16]);
    }
    for (i, e) in dir.chunks_exact(16).enumerate() {
        let pos = out.len();
        let toff = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let tlen = u32::from_be_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let start = toff.min(input.len());
        let end = toff.saturating_add(tlen).min(input.len());
        out.extend_from_slice(&input[start..end]);
        out.resize(out.len().next_multiple_of(4), 0);
        if &e[0..4] == b"head" {
            if end - start >= 12 {
                out[pos + 8..pos + 12].fill(0);
            }
            head_entry_offset = Some(toff);
        }
        out[12 + 16 * i + 8..12 + 16 * i + 12].copy_from_slice(&(pos as u32).to_be_bytes());
    }
    if let Some(toff) = head_entry_offset {
        // Ruby writes the adjustment at the member's ORIGINAL head offset + 8.
        super::writer::set_checksum_adjustment(&mut out, toff + 8);
    }

    let family = sfnt::Font::load(input, index, sfnt::Options::default())
        .ok()
        .and_then(|f| f.name().ok().flatten())
        .and_then(|n| n.english_name(6).or_else(|| n.english_name(1)));
    let base: String = family
        .unwrap_or_else(|| "font".to_string())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ext = ".ttf";
    Ok(Extracted {
        basename: format!("{base}{ext}"),
        data: out,
    })
}
