//! Registry parity: every registered check against the Ruby `audit.validate`
//! goldens (`validation_issues`), every sfnt, WOFF, WOFF2 and WOFF2-collection
//! face of every golden plus the `_swallow` hand-built cases. Only
//! `format_round_trip` (card 1f-b) is dropped from the golden; dfont and Type 1
//! are routed to Ruby and skipped (and counted). Goldens: `FONTISAN_GOLDENS_DIR/read`,
//! else `<corpus>/corpus/goldens/read`; absent goldens skip with a message and
//! `FONTISAN_SPIKE_REQUIRE_CORPUS=1` makes the absence a failure.
mod common;

use common::*;
use fontisan::audit::{CheckError, Issue, REGISTRY};
use fontisan::sfnt::{Font, Format, LoadingMode, Options, detect_format, face_count};
use serde_json::{Value, json};
use std::path::PathBuf;

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
    for (code, run) in REGISTRY {
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
    let (mut faces, mut skipped, mut issues_seen) = (0, Vec::new(), 0);
    for path in paths {
        let g: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let Some(oks) = g["audit"]["validate"]["ok"].as_array() else {
            skipped.push(format!("{name}: golden is a Ruby error"));
            continue;
        };
        let bytes = golden_font(Some(&corpus), &dir, &path, &g).expect("font for golden");
        if matches!(detect_format(&bytes), Some(Format::Dfont)) || face_count(&bytes).is_none() {
            skipped.push(format!("{name}: dfont / Type 1 (routed to Ruby)"));
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
                        "{name}#{idx}: load fails ({})",
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
                .filter(|i| i["category"] != "format_round_trip")
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
