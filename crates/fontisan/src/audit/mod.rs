//! Audit checks and report (card 1f). `types` is the shared contract; each check
//! lives in `checks/<name>.rs` as `pub fn run(font) -> Result<Vec<Issue>, CheckError>`.

pub mod checks;
pub mod report;
pub(crate) mod ruby;
pub mod types;

pub use report::{Axis, Report, Summary, audit, build_report};
pub use types::{CheckError, CheckFn, Issue, Severity};

use crate::sfnt::Font;
use checks::*;

/// The `default` profile in the Ruby registry's order (`CheckRegistry::PROFILES[:default]`).
/// Entries marked `// 1f-checks` belong to card 1f-checks and `format_round_trip`
/// to card 1f-b; the integrator uncomments them once those modules exist.
pub const REGISTRY: &[(&str, CheckFn)] = &[
    // 1f-checks ("table_directory", table_directory::run),
    // 1f-checks ("glyph_names", glyph_name::run),
    // 1f-checks ("cmap", cmap::run),
    // 1f-checks ("ots_compatibility", ots_compatibility::run),
    // 1f-checks ("collection_integrity", collection_integrity::run),
    // 1f-checks ("variable_font", variable_font::run),
    // 1f-checks ("hinting", hinting::run),
    // 1f-checks ("woff2_validation", woff2_validation::run),
    // 1f-b ("format_round_trip", format_round_trip::run),
    // 1f-checks ("opentype_conformance", opentype_conformance::run),
    // 1f-checks ("naming_consistency", naming_consistency::run),
    (head::CODE, head::run),
    (hhea::CODE, hhea::run),
    (maxp::CODE, maxp::run),
    // 1f-checks ("ot_os2", os2::run),
    (name_table::CODE, name_table::run),
    (post::CODE, post::run),
    (kern::CODE, kern::run),
    (cff_table::CODE, cff_table::run),
    // 1f-checks ("ot_glyf", glyf::run),
    (layout_table::CODE, layout_table::run),
];

/// Codes owned by this card's registered checks (parity compares only these).
pub fn registered_codes() -> Vec<&'static str> {
    REGISTRY.iter().map(|(c, _)| *c).collect()
}

/// Run every registered check; a check that errs becomes a fatal
/// `"Check <code> failed to execute: <Class>: <message>"` issue (`safe_run_check`).
pub fn run_checks(font: &Font<'_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (code, check) in REGISTRY {
        match check(font) {
            Ok(v) => issues.extend(v),
            Err(e) => issues.push(e.into_issue(code)),
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfnt::Font;

    /// A minimal sfnt over the given tables (checksums are not read).
    fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut off = 12 + 16 * tables.len();
        for (tag, d) in tables {
            out.extend(*tag);
            out.extend(0u32.to_be_bytes());
            out.extend((off as u32).to_be_bytes());
            out.extend((d.len() as u32).to_be_bytes());
            off += d.len();
        }
        for (_, d) in tables {
            out.extend(d);
        }
        out
    }

    fn issues(
        tables: &[(&[u8; 4], Vec<u8>)],
        check: CheckFn,
    ) -> Vec<(String, &'static str, String)> {
        let bytes = sfnt(tables);
        let f = Font::from_bytes(&bytes).unwrap();
        check(&f)
            .unwrap()
            .into_iter()
            .map(|i| {
                (
                    i.location.unwrap_or_default(),
                    i.severity.as_str(),
                    i.message,
                )
            })
            .collect()
    }

    fn head(magic: u32, upm: u16, mac: u16, loc: i16) -> Vec<u8> {
        let mut d = vec![0, 1, 0, 0, 0, 0, 0, 0];
        d.extend(0u32.to_be_bytes());
        d.extend(magic.to_be_bytes());
        d.extend([0, 0]);
        d.extend(upm.to_be_bytes());
        d.extend([0u8; 16]); // created, modified: 0
        d.extend([0u8; 8]);
        d.extend(mac.to_be_bytes());
        d.extend([0u8; 4]);
        d.extend(loc.to_be_bytes());
        d.extend([0, 0]);
        d
    }

    #[test]
    fn head_reports_each_violation() {
        let got = issues(&[(b"head", head(0xdeadbeef, 8, 0x0100, 2))], head::run);
        let msgs: Vec<_> = got.iter().map(|g| g.2.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "head.magicNumber is 0xdeadbeef but must be 0x5f0f3cf5",
                "head.unitsPerEm is 8 but must be between 16 and 16384",
                "head.indexToLocFormat is 2 but must be 0 (short) or 1 (long)",
                "head.created is 0 \u{2014} creation date not set",
                "head.modified is 0 \u{2014} modification date not set",
                "head.macStyle uses reserved bits (0x100, bits 8-15 must be 0)",
            ]
        );
        assert!(issues(&[(b"head", head(0x5F0F3CF5, 1000, 0, 0))], head::run).len() == 2);
    }

    #[test]
    fn post_formats_floats_like_ruby() {
        let post = |ver: i32, angle: i32, fixed: u32| {
            let mut d = Vec::new();
            d.extend(ver.to_be_bytes());
            d.extend(angle.to_be_bytes());
            d.extend([0u8; 4]);
            d.extend(fixed.to_be_bytes());
            d.extend([0u8; 16]);
            d
        };
        // version 0x00018000 = 1.5, angle -1/65536 with macStyle plain => info
        let got = issues(
            &[
                (b"head", head(0x5F0F3CF5, 1000, 0, 0)),
                (b"post", post(0x18000, 0x0000_0001, 2)),
            ],
            post::run,
        );
        let msgs: Vec<_> = got.iter().map(|g| g.2.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "post.version is 1.5 but must be one of 1.0, 2.0, 2.5, 3.0, 4.0",
                "post.italicAngle is 1.52587890625e-05 but head.macStyle does not indicate italic (angle should be 0)",
                "post.isFixedPitch is 2 but must be 0 (proportional) or 1 (monospace)",
            ]
        );
    }

    #[test]
    fn hhea_kern_and_cff_checks() {
        let mut hhea = vec![0, 1, 0, 0];
        hhea.extend((-5i16).to_be_bytes()); // ascent
        hhea.extend(7i16.to_be_bytes()); // descent
        hhea.extend((-1i16).to_be_bytes()); // line gap
        hhea.extend([0u8; 2 + 6 + 6 + 8 + 2 + 2]);
        let got = issues(&[(b"hhea", hhea)], hhea::run);
        let sev: Vec<_> = got.iter().map(|g| (g.0.as_str(), g.1)).collect();
        assert_eq!(
            sev,
            [
                ("hhea.ascent", "warning"),
                ("hhea.descent", "warning"),
                ("hhea.line_gap", "info"),
                ("hhea.advance_width_max", "warning"),
            ]
        );
        let got = issues(&[(b"kern", vec![0; 4])], kern::run);
        assert_eq!(
            got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(),
            ["tables.kern", "tables.GPOS"]
        );
        // a CFF whose CharStrings count (0: no Top DICT charstrings) differs from maxp 5
        let mut cff = vec![1, 0, 4, 1];
        cff.extend([0, 1, 1, 1, 2, b'x']); // Name INDEX: one name "x"
        cff.extend([0, 1, 1, 1, 2, 0]); // Top DICT INDEX: one empty-ish DICT (1 byte)
        cff.extend([0, 0]); // String INDEX
        cff.extend([0, 0]); // Global Subr INDEX
        let mut maxp = vec![0, 0, 0x50, 0, 0, 5];
        maxp.truncate(6);
        let got = issues(&[(b"CFF ", cff), (b"maxp", maxp)], cff_table::run);
        assert_eq!(
            got[0].2,
            "CFF  CharStrings count (0) does not match maxp.numGlyphs (5)"
        );
        assert_eq!(got[0].0, "cff .charstrings");
    }

    #[test]
    fn a_failing_check_becomes_a_fatal_issue() {
        // maxp cut to 4 bytes: ot_maxp raises, the runner wraps it.
        let bytes = sfnt(&[(b"maxp", vec![0, 0, 0x50, 0])]);
        let f = Font::from_bytes(&bytes).unwrap();
        let all = run_checks(&f);
        assert!(all.iter().any(|i| i.category == "ot_maxp"
            && i.severity == Severity::Fatal
            && i.message.starts_with("Check ot_maxp failed to execute: ")));
    }
}
