//! One module per Ruby audit check (lib/fontisan/audit/checks/*.rb).
// 1f-core
pub mod cff_table;
pub mod head;
pub mod hhea;
pub mod kern;
pub mod layout_table;
pub mod maxp;
pub mod name_table;
pub mod post;
// 1f-checks: the integrator merges their `pub mod` lines here.

pub mod cmap;
pub mod collection_integrity;
pub mod format_round_trip;
pub mod glyf;
pub mod glyph_name;
pub mod hinting;
pub mod naming_consistency;
pub mod opentype_conformance;
pub mod os2;
pub mod ots_compatibility;
pub mod table_directory;
pub mod variable_font;
pub mod woff2_validation;
