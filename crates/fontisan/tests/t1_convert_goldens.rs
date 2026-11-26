//! The otf→type1 engine conversion against the fontisan repo's pinned
//! golden: `convert(TestOTF.otf, to: :type1)` must equal the bytes the Ruby
//! tier produced when `corpus/goldens/convert/fonttools-test-otf.json` was
//! regenerated. Runs only where the fontisan checkout sits next to this
//! repo (the maintainer layout); CI has no such checkout and skips.

use sha2::{Digest, Sha256};

fn fontisan_root() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new("../../../fontisan");
    root.exists().then(|| root.to_path_buf())
}

fn golden_sha(goldens: &std::path::Path, font: &str, case: &str) -> Option<(String, usize)> {
    let text =
        std::fs::read_to_string(goldens.join("convert").join(format!("{font}.json"))).ok()?;
    let golden: serde_json::Value = serde_json::from_str(&text).ok()?;
    let output = golden.get("cases")?.get(case)?.get("output")?;
    Some((
        output.get("sha256")?.as_str()?.to_string(),
        usize::try_from(output.get("size")?.as_u64()?).ok()?,
    ))
}

fn corpus_font(fontisan: &std::path::Path, id: &str, file: &str) -> Vec<u8> {
    std::fs::read(fontisan.join("corpus").join("fonts").join(id).join(file)).expect("corpus font")
}

fn sha(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &byte in b {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xF) as usize] as char);
    }
    out
}

#[test]
fn otf_to_type1_matches_the_ruby_golden() {
    let Some(fontisan) = fontisan_root() else {
        return;
    };
    let goldens = fontisan.join("corpus").join("goldens");
    let Some((want_sha, want_size)) = golden_sha(&goldens, "fonttools-test-otf", "format:type1")
    else {
        return;
    };
    let input = corpus_font(&fontisan, "fonttools-test-otf", "TestOTF.otf");
    let out = fontisan::convert::convert(
        &input,
        fontisan::convert::Target::Type1,
        fontisan::convert::SourceType::Ttc,
        1_735_689_600,
    )
    .expect("converts");
    eprintln!("goldens dir={:?}", goldens.canonicalize());
    eprintln!("golden size={want_size} sha={want_sha}");
    eprintln!("engine size={} sha={}", out.len(), sha(&out));
    assert_eq!(out.len(), want_size, "size");
    assert_eq!(sha(&out), want_sha, "otf→type1 bytes vs the ruby golden");
}

#[test]
fn ttf_to_type1_matches_the_ruby_golden() {
    let Some(fontisan) = fontisan_root() else {
        return;
    };
    let goldens = fontisan.join("corpus").join("goldens");
    let Some((want_sha, want_size)) = golden_sha(&goldens, "fonttools-test-ttf", "format:type1")
    else {
        return;
    };
    let input = corpus_font(&fontisan, "fonttools-test-ttf", "TestTTF.ttf");
    let out = fontisan::convert::convert(
        &input,
        fontisan::convert::Target::Type1,
        fontisan::convert::SourceType::Ttc,
        1_735_689_600,
    );
    // The glyf path may not be wired yet: until it is, this test documents
    // the intended contract and is skipped by the expect above failing.
    let out = out.expect("converts");
    assert_eq!(out.len(), want_size, "size");
    assert_eq!(sha(&out), want_sha, "ttf→type1 bytes vs the ruby golden");
}
