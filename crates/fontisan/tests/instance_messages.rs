//! F4: the coordinate error text prints Ruby's `Float#to_s` for Infinity and NaN
//! (`Infinity`, not Rust's `inf`). Skips (and says so) without the corpus.
mod common;

use common::{corpus, font_path};
use fontisan::{ErrorKind, instance};

#[test]
fn non_finite_coordinates_use_ruby_float_text() {
    let Some(c) = corpus() else { return };
    let font = std::fs::read(font_path(&c, "SourceSans3VF-Upright.ttf")).unwrap();
    for (v, text) in [
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (f64::NAN, "NaN"),
    ] {
        let e = instance::instance(&font, &[(*b"wght", v)]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidCoordinates);
        assert_eq!(
            e.message,
            format!("Invalid coordinate for axis 'wght': {text}")
        );
    }
}
