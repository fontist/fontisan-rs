use fontisan::convert::{SourceType, Target, convert};

#[test]
fn woff_encode_matches_the_ruby_tier() {
    let Some(dir) = std::path::Path::new("../../fontisan-rs/.corpus/fontisan/corpus/fonts")
        .exists()
        .then(|| std::path::PathBuf::from("../../fontisan-rs/.corpus/fontisan/corpus/fonts"))
    else {
        return;
    };
    let input = std::fs::read(dir.join("TestTTF.ttf")).expect("corpus ttf");
    let out = convert(&input, Target::Woff, SourceType::Ttc, 1_735_689_600).expect("converts");
    println!(
        "engine woff: {} bytes sha {:02x?}",
        out.len(),
        &sha(&out)[..8]
    );
    assert_eq!(
        sha(&out),
        [
            0x8d, 0xf8, 0xce, 0xea, 0x2b, 0xa3, 0xea, 0xc1, 0x91, 0xa0, 0x61, 0x16, 0x22, 0xc5,
            0x6c, 0x0a, 0xed, 0xe7, 0x53, 0xe1, 0x8a, 0x3d, 0x2a, 0x1d, 0x85, 0x0b, 0x6d, 0x56,
            0x27, 0xe4, 0x93, 0x34,
        ],
        "engine woff bytes equal the ruby tier's"
    );
}

fn sha(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}
