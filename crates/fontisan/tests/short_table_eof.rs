//! F8: a table that starts inside the file and runs past EOF is held short; Ruby's
//! `font.table(tag)` then raises BinData's EOFError when the body is too small for the
//! table header, on `read` (GSUB/GPOS in the layout summary) and on `subset` (every
//! profile table is parsed). The engine reports the same class string, which the gem
//! passes through unchanged (EOFError is pinned by the malformed goldens).
//! Inputs: p4-parity-inputs/source-sans-3-regular-otf--swap-DSIG-GSUB.otf (GSUB: 8 of
//! 39734 bytes present) and fonttools-test-otf--swap-hmtx-name.otf (name: 32 of 1130).
use fontisan::sfnt::{ErrorClass, LoadingMode, Options};

const GSUB: &[u8] = include_bytes!("break/source-sans-3-regular-otf--swap-DSIG-GSUB.otf");
const NAME: &[u8] = include_bytes!("break/fonttools-test-otf--swap-hmtx-name.otf");

fn subset_err(bytes: &[u8], gids: &[u32]) -> (String, String) {
    let e = fontisan::subset::subset(bytes, gids, "web").expect_err("subset must fail");
    (e.class.to_string(), e.message)
}

#[test]
fn read_full_raises_eof_on_short_gsub() {
    for lazy in [false, true] {
        let opts = Options {
            mode: LoadingMode::Full,
            lazy,
        };
        let e = fontisan::read::read(GSUB, opts).expect_err("read must fail");
        assert_eq!(e.class, ErrorClass::Eof, "lazy={lazy}");
    }
}

#[test]
fn read_metadata_still_succeeds_on_short_gsub() {
    // Ruby's metadata mode never loads GSUB (measured: read OK).
    let opts = Options {
        mode: LoadingMode::Metadata,
        lazy: true,
    };
    fontisan::read::read(GSUB, opts).expect("metadata read");
}

#[test]
fn subset_raises_eof_on_short_gsub_and_short_name() {
    let want = ("EOFError".to_string(), "End of file reached".to_string());
    assert_eq!(subset_err(GSUB, &[1, 2, 9, 28]), want);
    assert_eq!(subset_err(NAME, &[3]), want);
}
