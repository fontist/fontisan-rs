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
