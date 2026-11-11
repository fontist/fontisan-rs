//! F7: subset writes hhea's four reserved int16s as zeros, like Ruby, even when the
//! source font has junk there (`source-sans-3-regular-ttf--flip-hhea.ttf` in the p4
//! break inputs). Skips (and says so) without the corpus.
mod common;

use common::{corpus, font_path};

fn table(font: &[u8], tag: &[u8; 4]) -> (usize, usize) {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    for i in 0..n {
        let e = &font[12 + 16 * i..28 + 16 * i];
        if &e[0..4] == tag {
            let off = u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(e[12..16].try_into().unwrap()) as usize;
            return (off, len);
        }
    }
    panic!("no {tag:?}");
}

#[test]
fn junk_in_hhea_reserved_is_zeroed_in_the_subset() {
    let Some(c) = corpus() else { return };
    let mut font = std::fs::read(font_path(&c, "SourceSans3-Regular.ttf")).unwrap();
    let (off, _) = table(&font, b"hhea");
    for (i, b) in font[off + 24..off + 32].iter_mut().enumerate() {
        *b = 0xA0 + i as u8;
    }
    let glyphs: Vec<u32> = "Hello Aa1".bytes().map(u32::from).collect();
    let out = fontisan::subset::subset(&font, &glyphs, "pdf").unwrap();
    let (o, l) = table(&out, b"hhea");
    assert_eq!(l, 36);
    assert_eq!(&out[o + 24..o + 32], &[0u8; 8]);
    // Preservation: the other fields still come from the source.
    assert_eq!(&out[o + 4..o + 10], &font[off + 4..off + 10]);
}
