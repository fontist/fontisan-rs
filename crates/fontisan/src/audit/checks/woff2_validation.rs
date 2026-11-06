//! `woff2_validation` (Ruby `Woff2ValidationCheck`).
//!
//! Ruby only validates a `Woff2Font` (a single-font WOFF2); every other source,
//! including WOFF2 collection members, returns no issue. The header fields come
//! from the WOFF2 view of [`Font`].

use crate::audit::types::{CheckError, Issue, Severity};
use crate::sfnt::Font;

pub const CODE: &str = "woff2_validation";

const SIGNATURE: u32 = 0x774F_4632;
const VALID_FLAVORS: [u32; 4] = [0x0001_0000, 0x7472_7565, 0x4F54_544F, 0x7474_6366];

pub fn run(font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    let Some(h) = font.woff2_header() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut add = |sev, loc: &str, msg: String| {
        out.push(Issue::new(CODE, sev, msg).at(format!("woff2_header.{loc}")));
    };
    if h.signature != SIGNATURE {
        add(
            Severity::Error,
            "signature",
            format!(
                "WOFF2 signature is 0x{:x} but must be 0x{SIGNATURE:x} ('wOF2')",
                h.signature
            ),
        );
    }
    if !VALID_FLAVORS.contains(&h.flavor) {
        add(
            Severity::Error,
            "flavor",
            format!(
                "WOFF2 flavor is 0x{:x} but must be a valid SFNT version \
                 (0x00010000, 0x74727565, 0x4F54544F, or 0x74746366)",
                h.flavor
            ),
        );
    }
    if h.reserved != 0 {
        add(
            Severity::Warning,
            "reserved",
            format!(
                "WOFF2 reserved field is {} but must be 0 per the spec",
                h.reserved
            ),
        );
    }
    if h.major_version != 1 {
        add(
            Severity::Warning,
            "major_version",
            format!(
                "WOFF2 major version is {} but the spec recommends 1",
                h.major_version
            ),
        );
    }
    if h.total_compressed_size == 0 {
        add(
            Severity::Error,
            "total_compressed_size",
            "WOFF2 total_compressed_size is 0 but must be positive".into(),
        );
    }
    if h.total_sfnt_size == 0 {
        add(
            Severity::Error,
            "total_sfnt_size",
            "WOFF2 total_sfnt_size is 0 but must be positive".into(),
        );
    }
    if h.meta_offset > 0 {
        if h.meta_length == 0 {
            add(
                Severity::Error,
                "meta_length",
                "WOFF2 metadata offset is set but meta_length is 0 (must be positive)".into(),
            );
        }
        if h.meta_orig_length == 0 {
            add(
                Severity::Error,
                "meta_orig_length",
                "WOFF2 metadata offset is set but meta_orig_length is 0 (must be positive)".into(),
            );
        }
    }
    Ok(out)
}
