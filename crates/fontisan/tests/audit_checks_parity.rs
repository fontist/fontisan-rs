//! 1f-checks parity: the Rust audit checks against the Ruby `audit.validate`
//! goldens (`validation_issues`), every sfnt face of every golden plus the
//! `_swallow` hand-built cases. The check set here is a subset of the Ruby
//! registry, so the golden issues are filtered to this set's categories; order
//! within that set follows the Ruby default-profile order.
//!
//! WOFF/WOFF2 goldens need the 1e wire-up and Type 1 / dfont are routed to Ruby:
//! they are skipped (and counted) here. Goldens: `FONTISAN_GOLDENS_DIR/read`,
//! else `<corpus>/corpus/goldens/read`; absent goldens skip with a message and
//! `FONTISAN_SPIKE_REQUIRE_CORPUS=1` makes the absence a failure.
mod common;

use common::*;
use fontisan::audit::checks::*;
use fontisan::audit::{CheckError, CheckFn, Issue};
use fontisan::sfnt::{Font, LoadingMode, Options, face_count};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Ruby default-profile order, restricted to the checks implemented in 1f-checks.
const CHECKS: &[(&str, CheckFn)] = &[
    (table_directory::CODE, table_directory::run),
    (glyph_name::CODE, glyph_name::run),
    (cmap::CODE, cmap::run),
    (ots_compatibility::CODE, ots_compatibility::run),
    (collection_integrity::CODE, collection_integrity::run),
    (variable_font::CODE, variable_font::run),
    (hinting::CODE, hinting::run),
    (woff2_validation::CODE, woff2_validation::run),
    (opentype_conformance::CODE, opentype_conformance::run),
    (naming_consistency::CODE, naming_consistency::run),
    (os2::CODE, os2::run),
    (glyf::CODE, glyf::run),
];

fn goldens_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("FONTISAN_GOLDENS_DIR") {
        Some(d) => PathBuf::from(d),
        None => corpus_dir().join("corpus/goldens"),
    };
    let read = dir.join("read");
    if read.is_dir() {
        return Some(read);
    }
    assert!(
        std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_none(),
        "read goldens missing at {}",
        read.display()
    );
    eprintln!("SKIPPED: read goldens not found at {}", read.display());
    None
}

fn issue_json(i: &Issue) -> Value {
    let mut o =
        json!({"category": i.category, "message": i.message, "severity": i.severity.as_str()});
    if let Some(l) = &i.location {
        o["location"] = json!(l);
    }
    o
}

fn run_checks(font: &Font<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for (code, run) in CHECKS {
        match run(font) {
            Ok(issues) => out.extend(issues.iter().map(issue_json)),
            Err(e) => out.push(issue_json(&e.into_issue(code))),
        }
    }
    out
}

fn golden_font(
    corpus: Option<&PathBuf>,
    dir: &std::path::Path,
    path: &std::path::Path,
    g: &Value,
) -> Option<Vec<u8>> {
    if path.parent().is_some_and(|p| p != dir) {
        // `_swallow/<id>.json` sits next to `<id>.ttf` and has no `file` field.
        return std::fs::read(path.with_extension("ttf")).ok();
    }
    let corpus = corpus?;
    Some(std::fs::read(font_path(corpus, g["file"].as_str()?)).unwrap())
}

#[test]
fn checks_match_ruby_validation_issues() {
    let (Some(corpus), Some(dir)) = (corpus(), goldens_dir()) else {
        return;
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for d in [dir.clone(), dir.join("_swallow")] {
        if let Ok(rd) = std::fs::read_dir(&d) {
            paths.extend(
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "json")),
            );
        }
    }
    paths.sort();
    let codes: Vec<&str> = CHECKS.iter().map(|(c, _)| *c).collect();
    let (mut faces, mut skipped, mut issues_seen) = (0, Vec::new(), 0);
    for path in paths {
        let g: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let Some(oks) = g["audit"]["validate"]["ok"].as_array() else {
            skipped.push(format!("{name}: golden is a Ruby error"));
            continue;
        };
        let bytes = golden_font(Some(&corpus), &dir, &path, &g).expect("font for golden");
        if face_count(&bytes).is_none() {
            skipped.push(format!("{name}: not an sfnt (1e / routed to Ruby)"));
            continue;
        }
        for (idx, face) in oks.iter().enumerate() {
            let font = match Font::load(
                &bytes,
                idx,
                Options {
                    mode: LoadingMode::Full,
                    lazy: false,
                },
            ) {
                Ok(f) => f,
                Err(e) => {
                    skipped.push(format!(
                        "{name}#{idx}: load fails ({}), pending 1e-wire",
                        e.class.ruby_name()
                    ));
                    continue;
                }
            };
            let want: Vec<Value> = face["validation_issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| codes.contains(&i["category"].as_str().unwrap_or("")))
                .collect();
            let got = run_checks(&font);
            assert_eq!(got, want, "{name} face {idx}");
            faces += 1;
            issues_seen += want.len();
        }
    }
    eprintln!(
        "audit checks parity: {faces} faces, {issues_seen} issues matched; skipped: {skipped:#?}"
    );
    assert!(faces >= 25, "too few faces compared: {faces}");
    assert!(issues_seen > 0, "no golden issue was exercised");
}

#[test]
fn a_raising_check_becomes_a_fatal_issue_in_the_ruby_shape() {
    let i = CheckError::new("EOFError", "End of file reached").into_issue("hinting");
    assert_eq!(
        issue_json(&i),
        json!({"category": "hinting", "severity": "fatal",
               "message": "Check hinting failed to execute: EOFError: End of file reached"})
    );
}
