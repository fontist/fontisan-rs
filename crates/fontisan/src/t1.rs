//! Type 1 (fontisan#192 gap 4, read side): Ruby's measured surfaces.
//!
//! The Ruby facade cannot actually read a Type 1 font — `Type1Font` lacks the
//! sfnt accessors the post-load machinery calls — so every Type 1 operation
//! raises a deterministic NoMethodError-shaped error, and the parity contract
//! is to reproduce those surfaces byte for byte:
//!
//! * the loader parses PFB segments (and accepts any detected PFA) into a
//!   Type 1 object;
//! * `read`/`audit`/`outline_hashes` then raise `has_table?`, glyph_ids and
//!   subset raise `table`, instance raises `table_data`, convert raises the
//!   `table_data` text wrapped in the convert transformation prefix;
//! * a segment-walk failure fails the load with the parser's own
//!   `Invalid PFB: ...` message (plain `Fontisan::Error`), before any of the
//!   post-parse surfaces can fire.
//!
//! The encrypted section's contents are never verified at load (measured:
//! corrupt eexec bytes still reach the post-parse surface), so the walk is
//! the only parsing this module does.

use crate::sfnt::{Error, ErrorClass};

/// The sentinel `Font::load` returns for a parsed Type 1 font: the ops map it
/// to their measured post-parse surface (`op_error`).
pub const TYPE1_PARSED: &str = "type1 parsed";

/// The PFB segment markers.
const ASCII: u8 = 0x01;
const BINARY: u8 = 0x02;
const EOF: u8 = 0x03;

fn facade(msg: impl Into<String>) -> Error {
    Error::new(ErrorClass::Facade, msg.into())
}

/// The Type 1 sentinel: a parsed Type 1 font (the loader succeeded).
pub fn parsed() -> Error {
    facade(TYPE1_PARSED)
}

/// Whether `e` is the Type 1 sentinel.
pub fn is_sentinel(e: &Error) -> bool {
    e.class == ErrorClass::Facade && e.message == TYPE1_PARSED
}

/// The read machinery (read/audit/outline_hashes) touches `has_table?`
/// first. The sentinel maps to that surface; everything else passes through.
pub fn read_surface_if_type1(e: Error) -> Error {
    if is_sentinel(&e) {
        op_error("has_table?")
    } else {
        e
    }
}

/// The glyph-id lookups (glyph_ids, subset) touch `table`.
pub fn glyph_surface_if_type1(e: Error) -> Error {
    if is_sentinel(&e) {
        op_error("table")
    } else {
        e
    }
}

/// Parse a PFB for the ops that bypass `Font::load` (instance/convert work
/// on raw bytes): the walk error is the parser's own surface; success means
/// the Type 1 object exists and the caller raises its op surface.
pub fn parse_pfb_ok_or_fail(input: &[u8]) -> Result<(), crate::convert::ConvertError> {
    parse_pfb(input).map_err(|e| crate::convert::ConvertError::new(e.class.ruby_name(), e.message))
}

/// The post-parse surface for the accessor the op touches first
/// (`has_table?` for the read machinery, `table` for the glyph-id lookups,
/// `table_data` for instancing and conversion).
pub fn op_error(accessor: &str) -> Error {
    Error::new(
        ErrorClass::InvalidFont,
        format!(
            "Malformed font data (NoMethodError): undefined method '{accessor}' \
             for an instance of Fontisan::Type1Font"
        ),
    )
}

/// Walk the PFB segments the way Ruby's parser does. `Ok(())` = the loader
/// would build the Type 1 object; the error is the parser's own surface.
pub fn parse_pfb(input: &[u8]) -> Result<(), Error> {
    let mut at = 0usize;
    let mut chunk = 0usize;
    loop {
        let kind = match input.get(at..at + 2) {
            Some([0x80, t @ (ASCII | BINARY | EOF)]) => *t,
            Some([a, b]) => {
                return Err(facade(format!(
                    "Invalid PFB: unknown chunk marker 0x{a:02X}{b:02X} at offset {at}"
                )));
            }
            // Ruby treats running out of data between segments as a clean end.
            Some(_) | None => return Ok(()),
        };
        at += 2;
        if kind == EOF {
            return Ok(());
        }
        let len = match input.get(at..at + 2) {
            Some(b) => u16::from_le_bytes([b[0], b[1]]) as usize,
            None => {
                return Err(facade(format!(
                    "Invalid PFB: chunk {chunk} header truncated"
                )));
            }
        };
        at += 2;
        let kind_name = if kind == ASCII { "ASCII" } else { "binary" };
        let remaining = input.len().saturating_sub(at);
        if len > remaining {
            return Err(facade(format!(
                "Invalid PFB: {kind_name} chunk {chunk} length {len} exceeds remaining data"
            )));
        }
        at += len;
        chunk += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_rejects_a_chunk_past_the_end_like_ruby() {
        let Err(e) = parse_pfb(&[0x80, ASCII, 0xE6, 0x02, 0x00, 0x00]) else {
            panic!("parses");
        };
        assert_eq!(e.class, ErrorClass::Facade);
        assert_eq!(
            e.message,
            "Invalid PFB: ASCII chunk 0 length 742 exceeds remaining data"
        );
    }

    #[test]
    fn walk_accepts_segments_that_fit() {
        let mut input = vec![0x80, BINARY, 4, 0, 1, 2, 3, 4];
        input.extend_from_slice(&[0x80, ASCII, 2, 0, 5, 6]);
        input.extend_from_slice(&[0x80, EOF, 0, 0]);
        assert!(parse_pfb(&input).is_ok());
    }
}
