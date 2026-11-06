//! #158 break test F2: audit parsed the layout lists once per script (12-15 s
//! on a 137 KB GSUB). It must use the once-parsed, budgeted path like `read`.
use std::time::{Duration, Instant};

fn seed(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/seeds")
        .join(name);
    std::fs::read(path).unwrap()
}

fn tag(i: usize) -> [u8; 4] {
    [
        b'A' + (i / 17576 % 26) as u8,
        b'a' + (i / 676 % 26) as u8,
        b'a' + (i / 26 % 26) as u8,
        b'a' + (i % 26) as u8,
    ]
}

/// An sfnt holding only a GSUB with `scripts` records that share one Script
/// table whose DefaultLangSys lists `features` feature indices (the shape of
/// the 137 KB break-test font; nothing binary is committed).
fn gsub_font(scripts: usize, features: usize) -> Vec<u8> {
    let be16 = |v: usize| u16::try_from(v).unwrap().to_be_bytes();
    let script_list_len = 2 + 6 * scripts;
    let script_table = script_list_len;
    let mut sl = Vec::new();
    sl.extend(be16(scripts));
    for i in 0..scripts {
        sl.extend(tag(i));
        sl.extend(be16(script_table));
    }
    sl.extend(be16(4)); // defaultLangSys offset (inside the Script table)
    sl.extend(be16(0)); // langSysCount
    sl.extend(be16(0)); // lookupOrder
    sl.extend([0xff, 0xff]); // requiredFeatureIndex
    sl.extend(be16(features));
    for i in 0..features {
        sl.extend(be16(i));
    }
    let mut fl = Vec::new();
    fl.extend(be16(features));
    for i in 0..features {
        fl.extend(tag(i));
        fl.extend(be16(2 + 6 * features));
    }
    fl.extend([0, 0, 0, 0]); // one shared Feature table: no params, no lookups
    // FeatureList first so every u16 offset fits; no LookupList.
    let mut gsub = vec![0, 1, 0, 0];
    gsub.extend(be16(10 + fl.len()));
    gsub.extend(be16(10));
    gsub.extend(be16(0));
    gsub.extend(&fl);
    gsub.extend(&sl);
    let mut f = vec![0, 1, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0];
    f.extend(b"GSUB");
    f.extend([0, 0, 0, 0]);
    f.extend(28u32.to_be_bytes());
    f.extend(u32::try_from(gsub.len()).unwrap().to_be_bytes());
    f.extend(gsub);
    f
}

/// Without the once-parsed, budgeted path this was 12-15 s.
#[test]
fn audit_of_a_generated_137k_gsub_is_fast() {
    let data = gsub_font(10_900, 9_000);
    assert!(data.len() > 130_000, "{}", data.len());
    for validate in [false, true] {
        let start = Instant::now();
        let r = fontisan::audit::audit_staged(&data, "x.otf", validate);
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(10),
            "validate={validate}: audit took {took:?}"
        );
        eprintln!("validate={validate}: {took:?} ok={}", r.is_ok());
    }
}

/// Audit used `features(script)` per script, re-parsing both lists each time
/// (12-15 s on a 137 KB GSUB while `read` took 0.1 s).
#[test]
fn audit_of_a_layout_heavy_font_is_fast() {
    for name in [
        "timeout-font-layout-quadratic.otf",
        "timeout-font-layout-scripts.ttf",
    ] {
        let data = seed(name);
        for validate in [false, true] {
            let start = Instant::now();
            let _ = fontisan::audit::audit_staged(&data, "x.otf", validate);
            let took = start.elapsed();
            assert!(
                took < Duration::from_secs(10),
                "{name} validate={validate}: audit took {took:?}"
            );
        }
    }
}
