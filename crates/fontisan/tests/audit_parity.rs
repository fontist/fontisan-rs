//! 1f-core: the audit report against `audit.default` / `audit.validate` of the
//! read goldens (ci sfnt fonts incl. the `_swallow` cases), and the
//! `indexability` profile against the scan goldens' `validation` field.
//!
//! Only the categories of the checks registered in `audit::REGISTRY` are compared
//! (the golden's other issues are filtered out and its summary recomputed over
//! the filtered list); the non-issue report fields are compared whole.
//! Pending (not compared, counted and printed): WOFF/WOFF2/dfont/Type 1 sources
//! (need 1e-wire or are routed to Ruby) and every category whose check is not
//! registered yet (1f-checks, 1f-b).
mod common;
#[path = "common/goldens.rs"]
mod goldens;

use common::*;
use fontisan::audit::{self, Report, Summary};
use fontisan::indexability;
use fontisan::sfnt::*;
use serde_json::{Map, Value, json};

fn issue_json(i: &audit::Issue) -> Value {
    let mut o = Map::new();
    o.insert("category".into(), json!(i.category));
    if let Some(l) = &i.location {
        o.insert("location".into(), json!(l));
    }
    o.insert("message".into(), json!(i.message));
    o.insert("severity".into(), json!(i.severity.as_str()));
    Value::Object(o)
}

fn summary_json(s: &Summary) -> Value {
    json!({"error_count": s.error_count, "fatal_count": s.fatal_count,
           "info_count": s.info_count, "passed": s.passed, "total": s.total,
           "warning_count": s.warning_count})
}

/// The canonical audit JSON: nil and empty collections omitted, except the
/// fields that are always present.
fn report_json(r: &Report) -> Value {
    let mut o = Map::new();
    let mut put = |k: &str, v: Value| {
        o.insert(k.into(), v);
    };
    put("source_file", json!(r.source_file));
    put("source_sha256", json!(r.source_sha256));
    put("source_format", json!(r.source_format));
    if let Some(i) = r.font_index {
        put("font_index", json!(i));
    }
    put("num_fonts_in_source", json!(r.num_fonts_in_source));
    for (k, v) in [
        ("family_name", &r.family_name),
        ("subfamily_name", &r.subfamily_name),
        ("full_name", &r.full_name),
        ("postscript_name", &r.postscript_name),
        ("version", &r.version),
        ("panose", &r.panose),
    ] {
        if let Some(v) = v {
            put(k, json!(v));
        }
    }
    put("font_revision_raw", json!(r.font_revision_raw));
    put("is_variable", json!(r.is_variable));
    put("italic", json!(r.italic));
    put("bold", json!(r.bold));
    if let Some(w) = r.weight_class {
        put("weight_class", json!(w));
    }
    if let Some(w) = r.width_class {
        put("width_class", json!(w));
    }
    put(
        "axes",
        json!(
            r.axes
                .iter()
                .map(|a| json!({"tag": a.tag, "min_value_raw": a.min_value_raw,
                    "default_value_raw": a.default_value_raw,
                    "max_value_raw": a.max_value_raw, "name_id": a.name_id}))
                .collect::<Vec<_>>()
        ),
    );
    put("total_codepoints", json!(r.total_codepoints));
    if !r.cmap_subtables.is_empty() {
        put("cmap_subtables", json!(r.cmap_subtables));
    }
    put("total_glyphs", json!(r.total_glyphs));
    if !r.opentype_scripts.is_empty() {
        put("opentype_scripts", json!(r.opentype_scripts));
    }
    if !r.features.is_empty() {
        put("features", json!(r.features));
    }
    if let Some(issues) = &r.validation_issues {
        if !issues.is_empty() {
            put(
                "validation_issues",
                json!(issues.iter().map(issue_json).collect::<Vec<_>>()),
            );
        }
    }
    if let Some(s) = &r.validation_summary {
        put("validation_summary", summary_json(s));
    }
    Value::Object(o)
}

/// Keep only the issues of registered categories and recompute the summary.
fn filter_golden(face: &Value, codes: &[&str]) -> Value {
    let mut face = face.clone();
    let Some(all) = face
        .get("validation_issues")
        .and_then(|v| v.as_array())
        .cloned()
    else {
        if face.get("validation_summary").is_some() {
            face["validation_summary"] = summary_json(&Summary::from_issues(&[]));
        }
        return face;
    };
    let kept: Vec<Value> = all
        .into_iter()
        .filter(|i| codes.contains(&i["category"].as_str().unwrap()))
        .collect();
    let parsed: Vec<audit::Issue> = kept
        .iter()
        .map(|i| {
            let sev = match i["severity"].as_str().unwrap() {
                "info" => audit::Severity::Info,
                "warning" => audit::Severity::Warning,
                "error" => audit::Severity::Error,
                _ => audit::Severity::Fatal,
            };
            audit::Issue::new(i["category"].as_str().unwrap(), sev, "")
        })
        .collect();
    let obj = face.as_object_mut().unwrap();
    obj.remove("validation_issues");
    if !kept.is_empty() {
        obj.insert("validation_issues".into(), Value::Array(kept));
    }
    obj.insert(
        "validation_summary".into(),
        summary_json(&Summary::from_issues(&parsed)),
    );
    face
}

#[test]
fn audit_matches_goldens_on_every_ci_sfnt_font() {
    let Some(corpus) = corpus() else { return };
    let Some(root) = goldens::goldens_root() else {
        return;
    };
    let codes = audit::registered_codes();
    let (mut compared, mut pending) = (0, Vec::new());
    let mut by_category = std::collections::BTreeMap::<String, usize>::new();
    for case in goldens::cases(&corpus, &root) {
        for (profile, validate) in [("default", false), ("validate", true)] {
            let want = &case.golden["audit"][profile];
            let Some(faces) = want["ok"].as_array() else {
                pending.push(format!(
                    "{} {profile}: golden is an error (Type 1; routed to Ruby)",
                    case.id
                ));
                continue;
            };
            let got = match audit::audit(&case.bytes, &case.file, validate) {
                Ok(g) => g,
                Err(e) if e.class == ErrorClass::Unsupported => {
                    pending.push(format!(
                        "{} {profile}: {} needs 1e-wire / unsupported",
                        case.id,
                        e.class.ruby_name()
                    ));
                    continue;
                }
                Err(e) => panic!("{} {profile}: unexpected {e}", case.id),
            };
            assert_eq!(got.len(), faces.len(), "{} {profile}: face count", case.id);
            for (i, (g, w)) in got.iter().zip(faces).enumerate() {
                let w = if validate {
                    filter_golden(w, &codes)
                } else {
                    w.clone()
                };
                let mut g = report_json(g);
                if validate {
                    // Compare against the golden restricted to registered categories.
                    assert!(g.get("validation_summary").is_some());
                }
                if let Value::Object(o) = &mut g {
                    o.entry("validation_summary").or_insert(Value::Null);
                    if !validate {
                        o.remove("validation_summary");
                    }
                }
                assert_eq!(g, w, "{}[{i}] {profile}", case.id);
                for i in w["validation_issues"].as_array().into_iter().flatten() {
                    *by_category
                        .entry(i["category"].as_str().unwrap().into())
                        .or_default() += 1;
                }
                compared += 1;
            }
        }
    }
    for p in &pending {
        eprintln!("PENDING: {p}");
    }
    eprintln!(
        "audit parity: {compared} reports compared, {} pending, registered {codes:?}",
        pending.len()
    );
    eprintln!("issues compared by category: {by_category:?}");
    assert!(compared >= 60, "too few compared: {compared}");
    assert!(
        by_category.contains_key("ot_layout"),
        "ot_layout never exercised"
    );
}

#[test]
fn indexability_matches_scan_goldens() {
    let Some(corpus) = corpus() else { return };
    let Some(root) = goldens::goldens_root() else {
        return;
    };
    let dir = root.join("scan");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let (mut compared, mut invalid, mut pending) = (0, 0, Vec::new());
    for path in files {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let id = g["id"].as_str().unwrap().to_string();
        let bytes = std::fs::read(font_path(&corpus, g["file"].as_str().unwrap())).unwrap();
        for (n, row) in g["rows"].as_array().unwrap().iter().enumerate() {
            let want = &row["validation"];
            if want.is_null() {
                continue;
            }
            let face = row["collection_index"].as_u64().unwrap_or(0) as usize;
            let opts = Options {
                mode: LoadingMode::Metadata,
                lazy: true,
            };
            let font = match Font::load(&bytes, face, opts) {
                Ok(f) => f,
                Err(e) => {
                    pending.push(format!("{id}[{n}]: load {}", e.class.ruby_name()));
                    continue;
                }
            };
            let r = indexability::validate(&font);
            let got = json!({
                "profile": "indexability", "valid": r.valid, "status": r.status,
                "error_summary": r.error_summary,
                "issues": r.issues.iter().map(|i| json!({"severity": i.severity,
                    "category": i.category, "message": i.message, "location": i.location}))
                    .collect::<Vec<_>>(),
            });
            assert_eq!(&got, want, "{id}[{n}]");
            compared += 1;
            invalid += usize::from(!r.valid);
        }
    }
    eprintln!("indexability parity: {compared} compared ({invalid} invalid), pending: {pending:?}");
    assert!(
        compared >= 30 && invalid >= 2,
        "too few: {compared}/{invalid}"
    );
}
