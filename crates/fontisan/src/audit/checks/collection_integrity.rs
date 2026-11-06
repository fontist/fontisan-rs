//! `collection_integrity` (Ruby `CollectionIntegrityCheck`).
//!
//! Ruby's check only inspects a `BaseCollection`. The audit always loads one
//! face as a font (`FontLoader.load(path, font_index:)`), never the collection
//! object, so the check returns no issue for every audited face (measured:
//! zero `collection_integrity` issues in every golden, ttc/otc included).

use crate::audit::types::{CheckError, Issue};
use crate::sfnt::Font;

pub const CODE: &str = "collection_integrity";

pub fn run(_font: &Font<'_>) -> Result<Vec<Issue>, CheckError> {
    Ok(Vec::new())
}
