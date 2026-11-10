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
        // Ruby seeks to the offset and reads `tlen` bytes: at or past the end of the
        // file that read returns nil, and the member's writer raises on the table.
        if tlen > 0 && toff >= input.len() {
            return Err(ConvertError::new(
                ERROR_CLASS,
                format!(
                    "Unexpected error during unpacking: Missing table data for tag '{}'",
                    String::from_utf8_lossy(&e[0..4])
                ),
            ));
        }
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
    // An empty `head` at an offset past the end has no table behind it; Ruby would seek
    // there and grow the file to that offset, so the adjustment is skipped instead.
    if let Some(toff) = head_entry_offset.filter(|t| t.saturating_add(12) <= input.len()) {
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

#[cfg(test)]
mod tests {
    use super::extract;

    fn one_table_ttc(tag: &[u8; 4], offset: u32, length: u32) -> Vec<u8> {
        let mut ttc = b"ttcf".to_vec();
        ttc.extend([0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 16]);
        ttc.extend([0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]); // sfnt header, one table
        ttc.extend(tag);
        ttc.extend([0; 4]);
        ttc.extend(offset.to_be_bytes());
        ttc.extend(length.to_be_bytes());
        ttc
    }

    /// Found by the 1j-abi mutation run: a `head` directory offset near u32::MAX made
    /// `set_checksum_adjustment` resize the output to ~4 GiB. Ruby raises for the
    /// table (no data behind the offset); so does the engine, and writes nothing.
    #[test]
    fn head_offset_past_the_collection_is_missing_table_data() {
        let ttc = one_table_ttc(b"head", 0xFFFF_FFF0, 54);
        let err = extract(&ttc, 0).expect_err("table behind the offset is missing");
        assert_eq!(err.class, "Fontisan::Error");
        assert_eq!(
            err.message,
            "Unexpected error during unpacking: Missing table data for tag 'head'"
        );
    }

    /// #159 break-test F2: a table whose offset sits at or past the end of the file
    /// is missing data in Ruby, for any tag, including at exactly the end.
    #[test]
    fn table_offset_at_or_past_the_end_is_missing_table_data() {
        for tag in [b"glyf", b"hmtx"] {
            let probe = one_table_ttc(tag, 0, 4);
            let ttc = one_table_ttc(tag, probe.len() as u32, 4);
            let err = extract(&ttc, 0).expect_err("offset at the end of the file");
            assert!(err.message.ends_with(&format!(
                "Missing table data for tag '{}'",
                std::str::from_utf8(tag).unwrap()
            )));
        }
    }

    /// A zero-length table at the end reads as an empty string in Ruby, and a table that
    /// starts inside the file but runs past its end is written truncated.
    #[test]
    fn empty_and_truncated_tables_are_still_written() {
        let probe = one_table_ttc(b"cvt ", 0, 0);
        assert!(extract(&one_table_ttc(b"cvt ", probe.len() as u32, 0), 0).is_ok());
        assert!(extract(&one_table_ttc(b"cvt ", 12, 4096), 0).is_ok());
    }
}
