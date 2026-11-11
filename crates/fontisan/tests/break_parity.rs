//! Round 2 parity with Ruby on corrupt inputs (p4-break-parity F9, F10). Both inputs are
//! corpus fonts with the byte edits of the saved minimal inputs applied here (the saved
//! files are 0.4-0.6 MB). Skips (and says so) without the corpus.
mod common;

use common::{corpus, font_path};
use fontisan::convert::{self, Target};

fn dir_entry(font: &[u8], tag: &[u8; 4]) -> usize {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    (0..n)
        .map(|i| 12 + 16 * i)
        .find(|&e| &font[e..e + 4] == tag)
        .unwrap()
}

/// F9: `source-sans-3-vf-upright-ttf--swap-OS_2-name.ttf` swaps the OS/2 and name directory
/// offsets (lengths stay), so `name` is OS/2 bytes plus the tables after them. Ruby's
/// NameRewriter slices leniently (`data[off, len]`), so the instance succeeds.
#[test]
fn instance_with_swapped_os2_and_name_offsets_succeeds_like_ruby() {
    let Some(c) = corpus() else { return };
    let mut font = std::fs::read(font_path(&c, "SourceSans3VF-Upright.ttf")).unwrap();
    let (o, n) = (dir_entry(&font, b"OS/2"), dir_entry(&font, b"name"));
    let (a, b) = (font[o + 8..o + 12].to_vec(), font[n + 8..n + 12].to_vec());
    font[o + 8..o + 12].copy_from_slice(&b);
    font[n + 8..n + 12].copy_from_slice(&a);
    let out = fontisan::instance::instance(&font, &[(*b"wght", 400.0)])
        .expect("Ruby instances this font");
    assert!(out.len() > 100_000);
}

/// F10: `noto-sans-hinted-regular--flip-cmap.ttf` makes the cmap map a glyph id at or past
/// numGlyphs. Ruby's output validation rejects it (`cmap_glyph_indices`).
#[test]
fn convert_to_otf_rejects_a_cmap_with_glyph_ids_past_num_glyphs() {
    let Some(c) = corpus() else { return };
    let orig = std::fs::read(font_path(&c, "NotoSans-Regular.ttf")).unwrap();
    // Positive control: the unmodified font converts.
    convert::convert(&orig, Target::Otf, 0).expect("unmodified font converts");
    let mut font = orig;
    assert_eq!(
        (font[100_439], font[104_694]),
        (0o132, 3),
        "corpus font changed"
    );
    font[100_439] = 3;
    font[104_694] = 0o145;
    let e = convert::convert(&font, Target::Otf, 0).unwrap_err();
    assert_eq!(
        e.message,
        "Conversion failed: Transformation failed: Output validation failed: Font structure validation failed for check 'cmap_glyph_indices'"
    );
}
