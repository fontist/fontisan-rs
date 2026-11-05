//! 1f-core: GSUB/GPOS script and feature enumeration against the `layout` field
//! of the read goldens, every ci sfnt font plus the `_swallow` cases, in all four
//! mode/lazy combinations (metadata mode gates the tables out: null).
mod common;
#[path = "common/goldens.rs"]
mod goldens;

use common::*;
use fontisan::sfnt::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn tag(t: &Tag) -> String {
    String::from_utf8_lossy(t).into_owned()
}

fn layout_value(r: Result<Option<LayoutTable>, Error>) -> Value {
    match r {
        Ok(Some(t)) => {
            let scripts = t.scripts();
            let features: BTreeMap<String, Vec<String>> = scripts
                .iter()
                .map(|s| (tag(s), t.features(s).iter().map(tag).collect()))
                .collect();
            json!({"scripts": scripts.iter().map(tag).collect::<Vec<_>>(), "features": features})
        }
        _ => Value::Null,
    }
}

fn layout(f: &Font) -> Value {
    json!({"GSUB": layout_value(f.gsub()), "GPOS": layout_value(f.gpos())})
}

#[test]
fn layout_matches_read_goldens_on_every_ci_font() {
    let Some(corpus) = corpus() else { return };
    let Some(root) = goldens::goldens_root() else {
        return;
    };
    let (mut fonts, mut checks, mut with_layout) = (0, 0, 0);
    for case in goldens::cases(&corpus, &root) {
        let g = &case.golden;
        let combos: Vec<_> = combos().into_iter().collect();
        for (i, face) in goldens::read_entry(g, "full/lazy")["ok"]
            .as_array()
            .cloned()
            .or_else(|| g["read"]["full/eager"]["ok"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if !matches!(face["format"].as_str(), Some("ttf" | "otf" | "ttc" | "otc")) {
                continue;
            }
            if i == 0 {
                fonts += 1;
            }
            for (name, opts) in &combos {
                let key = format!(
                    "{}/{}",
                    if name.starts_with("full") {
                        "full"
                    } else {
                        "metadata"
                    },
                    if name.ends_with("true") {
                        "lazy"
                    } else {
                        "eager"
                    }
                );
                if case.swallow && key != "full/eager" {
                    continue;
                }
                let want = &goldens::read_entry(g, &key)["ok"][i]["layout"];
                let font = Font::load(&case.bytes, i, *opts).unwrap();
                assert_eq!(&layout(&font), want, "{}[{i}] {key}", case.id);
                checks += 1;
                if key == "full/lazy" && !want["GSUB"].is_null() {
                    with_layout += 1;
                }
            }
        }
    }
    eprintln!("layout parity: {fonts} fonts, {checks} checks, {with_layout} faces with GSUB");
    assert!(
        fonts >= 25 && with_layout >= 10,
        "too few: {fonts}/{with_layout}"
    );
}

/// Ruby `table_data[(off - 10)..]`: an offset below 10 indexes from the END.
#[test]
fn short_offsets_slice_from_the_end_like_ruby() {
    let body = [0, 1, b'D', b'F', b'L', b'T', 0, 8];
    let with = |offset: u8, body: &[u8]| {
        let mut raw = vec![0, 1, 0, 0, 0, offset, 0, 0, 0, 0];
        raw.extend_from_slice(body);
        LayoutTable::from_bytes(&raw).unwrap()
    };
    // offset 2 => body[-8..], the whole 8-byte body
    assert_eq!(with(2, &body).scripts(), vec![*b"DFLT"]);
    // offset 0 is "no script list"; a body shorter than 8 bytes is nil => []
    assert_eq!(with(0, &body).scripts(), Vec::<Tag>::new());
    assert_eq!(with(2, &body[1..]).scripts(), Vec::<Tag>::new());
}

#[test]
fn well_formed_table_lists_scripts_and_features() {
    // header (10) + ScriptList@10 + Script@18 + LangSys@22 + FeatureList@32
    let mut raw = vec![0, 1, 0, 0, 0, 10, 0, 32, 0, 0];
    raw.extend_from_slice(&[0, 1, b'l', b'a', b't', b'n', 0, 8]); // ScriptList @10
    raw.extend_from_slice(&[0, 4, 0, 0]); // Script @18: default LangSys @+4, 0 records
    raw.extend_from_slice(&[0, 0, 0xFF, 0xFF, 0, 2, 0, 1, 0, 0]); // LangSys @22: idx 1, 0
    raw.extend_from_slice(&[
        0, 2, b'l', b'i', b'g', b'a', 0, 0, b'k', b'e', b'r', b'n', 0, 0,
    ]);
    let t = LayoutTable::from_bytes(&raw).unwrap();
    assert_eq!(t.scripts(), vec![*b"latn"]);
    assert_eq!(t.features(b"latn"), vec![*b"kern", *b"liga"]);
    assert_eq!(t.features(b"cyrl"), Vec::<Tag>::new());
    // a feature index past the list is skipped; truncating the FeatureList loses everything
    raw.truncate(raw.len() - 3);
    let t = LayoutTable::from_bytes(&raw).unwrap();
    assert_eq!(t.features(b"latn"), Vec::<Tag>::new());
}
