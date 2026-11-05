//! `woff2_validation` (Ruby `Woff2ValidationCheck`).
//!
//! Ruby only validates a `Woff2Font`; every other font returns no issue. The
//! sfnt `Font` here is always the decoded sfnt, so the header checks belong to
//! the WOFF2 path (card 1e-wire) and are pending until a WOFF2 face can be
//! audited: [`run`] returns no issue, exactly Ruby's answer for non-WOFF2 fonts.

use crate::audit::types::{CheckError, Issue};
use crate::sfnt::Font;

pub const CODE: &str = "woff2_validation";

pub fn run(_font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    Ok(Vec::new())
}
