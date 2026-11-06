//! 1g-scan: every scan golden row, field by field. Unsupported rows (dfont,
//! Type 1) must be exactly the formats routed to Ruby; every other row must
//! match whole.
mod common;

use common::*;
use fontisan::scan::{self, RowStatus};
use serde_json::Value;

#[test]
fn scan_matches_goldens() {
    let Some(corpus) = corpus() else { return };
    let dir = corpus_dir().join("corpus/goldens/scan");
    if !dir.is_dir() {
        assert!(
            std::env::var_os("FONTISAN_SPIKE_REQUIRE_CORPUS").is_none(),
            "scan goldens missing"
        );
        eprintln!("SKIPPED: scan goldens not found at {}", dir.display());
        return;
    }
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let (mut matched, mut unsupported) = (0, Vec::new());
    for path in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let id = g["id"].as_str().unwrap();
        let name = g["file"].as_str().unwrap();
        let bytes = std::fs::read(font_path(&corpus, name)).unwrap();
        let want = g["rows"].as_array().unwrap();
        let got = scan::scan_bytes(&bytes, name);
        if got[0].status == RowStatus::Unsupported {
            assert_eq!(got.len(), 1, "{id}: one unsupported row");
            unsupported.push((id.to_string(), got[0].format.clone().unwrap()));
            continue;
        }
        assert_eq!(got.len(), want.len(), "{id}: row count");
        for (n, (g, w)) in got.iter().zip(want).enumerate() {
            let mut j: Value = serde_json::from_str(&g.to_json().render()).unwrap();
            let o = j.as_object_mut().unwrap();
            assert_eq!(o.remove("status").unwrap(), "ok");
            let w = w.as_object().unwrap();
            for (k, wv) in w {
                assert_eq!(o.get(k), Some(wv), "{id}[{n}].{k}");
            }
            assert_eq!(o.len(), w.len(), "{id}[{n}]: field set");
            matched += 1;
        }
    }
    eprintln!("scan parity: {matched} rows matched, unsupported: {unsupported:?}");
    // Exactly the formats Ruby keeps: dfont and Type 1 (pfb; the pfa golden's
    // header is unrecognised by Ruby too, so it is an ordinary error row).
    let mut kinds: Vec<_> = unsupported.iter().map(|(_, f)| f.as_str()).collect();
    kinds.sort();
    assert!(
        kinds.iter().all(|k| ["dfont", "pfa", "pfb"].contains(k)),
        "{kinds:?}"
    );
    assert!(matched >= 40, "too few matched: {matched}");
}

#[test]
fn unsupported_rows_carry_no_guessed_fields() {
    let r = &scan::scan_bytes(b"%!PS-AdobeFont-1.0: X 1.0\n", "x.pfa")[0];
    assert_eq!(r.status, RowStatus::Unsupported);
    assert!(r.names.is_none() && r.validation.is_none() && r.errors == Default::default());
}

#[test]
fn unreadable_path_becomes_an_error_row() {
    let rows = scan::scan_path(std::path::Path::new("/nonexistent/none.ttf"));
    assert_eq!(rows.len(), 1);
    let e = rows[0].errors.font_file.as_ref().unwrap();
    assert!(e.message.contains("Errno::ENOENT"), "{}", e.message);
    assert_eq!(rows[0].num_faces, 0);
}
