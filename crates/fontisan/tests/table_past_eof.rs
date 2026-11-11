//! F6: a CFF-flavoured (OTTO) font read lazily keeps a table whose range starts past EOF as
//! an empty table, like Ruby's page-based loader (`read(PAGE_SIZE) || ""`): scan reports
//! `Table 'name' failed validation` (Ruby), not `Required table 'name' is missing`.
//! Input: p4-parity-inputs/fonttools-test-otf--trunc-at-head.otf (a 172-byte truncation
//! inside `head`, every table past EOF). The TrueType loader (`io.read` -> nil) is unchanged,
//! covered by the truncation goldens and the unit test in sfnt/font.rs.
use fontisan::scan::scan_bytes;

#[test]
fn otto_table_past_eof_is_kept_empty_not_missing() {
    let bytes = include_bytes!("break/fonttools-test-otf--trunc-at-head.otf");
    let rows = scan_bytes(bytes, "trunc-at-head.otf");
    assert_eq!(rows.len(), 1);
    let json = rows[0].to_json().render();
    assert!(json.contains("Table 'name' failed validation"), "{json}");
    assert!(json.contains("Table 'head' failed validation"), "{json}");
    assert!(json.contains("Table 'maxp' failed validation"), "{json}");
    assert!(!json.contains("Required table 'name' is missing"), "{json}");
}
