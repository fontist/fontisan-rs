//! 1c malformed parity: the 0e malformed goldens (load / name / table_head /
//! table_cmap / read ops, four mode/lazy combos, failure point) against the
//! sfnt model. Messages and numbers are normalised as the goldens are (digit
//! runs masked).
//!
//! Goldens come from the checkout xtask fetches at the pinned fontisan SHA.
mod common;

use common::*;
use fontisan::sfnt::{Error, Font, LoadingMode};
use serde_json::{Value, json};

fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push_str("<N>");
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

fn err_json(e: &Error) -> Value {
    let causes: Vec<Value> = e
        .cause
        .iter()
        .map(|(c, m)| json!({"class": c.ruby_name(), "message": norm(m)}))
        .collect();
    json!({"outcome": "error", "error_class": e.class.ruby_name(), "message": norm(&e.message), "causes": causes})
}

fn golden_dir() -> std::path::PathBuf {
    corpus_dir().join("corpus/goldens/malformed")
}

/// Ruby's `nil.<method>` on an absent table: the reference reads tables with
/// `font.table("x").method`, so a missing table is a NoMethodError, not a nil.
fn nil_table(method: &str) -> Value {
    json!({"outcome": "error", "error_class": "NoMethodError",
           "message": format!("undefined method '{method}' for nil"), "causes": []})
}

/// What Reference.read raises for a face: the first of name, head, hhea, maxp,
/// OS/2, cmap that fails to parse (`read_face` order).
fn read_error(font: &Font) -> Option<Error> {
    let r = read_face(&[], font);
    r.err()
}

fn ok_or(e: Result<Value, Error>) -> Value {
    e.unwrap_or_else(|e| err_json(&e))
}

#[test]
fn malformed_goldens_match() {
    let Some(corpus) = corpus() else { return };
    let mut kinds = 0;
    let mut files: Vec<_> = std::fs::read_dir(golden_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
    {
        let g: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let id = g["id"].as_str().unwrap().to_string();
        let bytes = std::fs::read(font_path(&corpus, g["file"].as_str().unwrap())).unwrap();
        for (combo, opts) in combos() {
            let want = &g["combos"][combo];
            let loaded = load(&bytes, 0, opts);
            let mut ops = serde_json::Map::new();
            let mut failure: Option<&str> = None;
            let mut note = |op: &'static str, v: Value| {
                if v["outcome"] == "error" && failure.is_none() {
                    failure = Some(op);
                }
                ops.insert(op.to_string(), v);
            };
            match &loaded {
                Err(e) => {
                    note("load", err_json(e));
                    for op in ["name", "table_head"] {
                        ops.insert(
                            op.into(),
                            json!({"outcome": "skipped", "reason": "load failed"}),
                        );
                    }
                    if opts.mode == LoadingMode::Full {
                        ops.insert(
                            "table_cmap".into(),
                            json!({"outcome": "skipped", "reason": "load failed"}),
                        );
                    }
                    ops.insert("read".into(), err_json(e));
                }
                Ok(font) => {
                    note(
                        "load",
                        json!({"outcome": "ok", "format": font.format().ruby_class()}),
                    );
                    note(
                        "name",
                        ok_or(font.name().map(|n| match n {
                            Some(n) => json!({"outcome": "ok", "family": n.english_name(1)}),
                            None => nil_table("english_name"),
                        })),
                    );
                    note(
                        "table_head",
                        ok_or(font.head().map(|h| match h {
                            Some(h) => json!({"outcome": "ok", "units_per_em": h.units_per_em}),
                            None => nil_table("units_per_em"),
                        })),
                    );
                    if opts.mode == LoadingMode::Full {
                        note(
                            "table_cmap",
                            ok_or(font.cmap().map(|c| match c {
                                Some(c) => {
                                    json!({"outcome": "ok", "mappings": c.codepoints().len()})
                                }
                                None => nil_table("unicode_mappings"),
                            })),
                        );
                    }
                    // `read` carries a digest that needs outlines (1d): compare outcome only.
                    let read = match read_error(font) {
                        Some(e) => err_json(&e),
                        None => json!({"outcome": "ok"}),
                    };
                    ops.insert("read".into(), read);
                }
            }
            for (op, got) in &ops {
                let w = &want["ops"][op];
                if w["outcome"] == "ok" {
                    // golden `read` ok also has faces/digest: compare the keys we produce
                    for (k, v) in got.as_object().unwrap() {
                        assert_eq!(&w[k], v, "{id} {combo} {op}.{k}");
                    }
                } else {
                    assert_eq!(&w["outcome"], &got["outcome"], "{id} {combo} {op}");
                    for k in ["error_class", "message", "causes", "reason"] {
                        assert_eq!(&w[k], &got[k], "{id} {combo} {op}.{k}");
                    }
                }
            }
            assert_eq!(
                want["failure_point"],
                failure.map_or(Value::Null, Value::from),
                "{id} {combo}: failure point"
            );
        }
        kinds += 1;
    }
    assert_eq!(kinds, 17, "malformed kinds");
}
