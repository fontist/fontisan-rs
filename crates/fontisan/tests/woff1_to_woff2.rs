//! E1: WOFF1 -> WOFF2 convert encoded an empty sfnt ("sfnt too short"); Ruby succeeds.
mod common;
use common::{corpus, font_path};
use fontisan::convert::{Target, convert};

#[test]
fn woff1_converts_to_woff2_and_round_trips() {
    let Some(c) = corpus() else { return };
    let woff = std::fs::read(font_path(&c, "TestWOFF.woff")).unwrap();
    let out = convert(&woff, Target::Woff2, 0).expect("WOFF1 -> WOFF2");
    assert_eq!(&out[..4], b"wOF2");
    let back = fontisan::woff::decode_woff2(&out).expect("decodes");
    let orig = fontisan::woff::decode_woff(&woff).unwrap();
    let names = |d: &[u8]| {
        let f = common::load(d, 0, fontisan::sfnt::Options::default()).unwrap();
        let mut n: Vec<_> = f.table_names().into_iter().map(|t| t.to_string()).collect();
        n.retain(|t| t != "DSIG"); // the WOFF2 encoder drops DSIG
        n.sort();
        n
    };
    assert_eq!(names(&back), names(&orig));
}
