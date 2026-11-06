//! Card 1e: WOFF / WOFF2 decode against the corpus.
mod common;

use fontisan::woff::{decode_woff, decode_woff2, decode_woff2_collection, is_woff2_collection};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn u32_at(b: &[u8], o: usize) -> usize {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) as usize
}
fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_be_bytes(b[o..o + 2].try_into().unwrap()) as usize
}

/// tag -> (directory checksum, table bytes) for the sfnt starting at `base`.
fn tables(b: &[u8], base: usize) -> BTreeMap<String, (u32, Vec<u8>)> {
    let n = u16_at(b, base + 4);
    (0..n)
        .map(|i| {
            let e = base + 12 + 16 * i;
            let (off, len) = (u32_at(b, e + 8), u32_at(b, e + 12));
            (
                String::from_utf8_lossy(&b[e..e + 4]).into_owned(),
                (u32_at(b, e + 4) as u32, b[off..off + len].to_vec()),
            )
        })
        .collect()
}

fn fonts_of(b: &[u8]) -> Vec<BTreeMap<String, (u32, Vec<u8>)>> {
    if &b[0..4] == b"ttcf" {
        (0..u32_at(b, 8))
            .map(|i| tables(b, u32_at(b, 12 + 4 * i)))
            .collect()
    } else {
        vec![tables(b, 0)]
    }
}

fn read(c: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(common::font_path(c, name)).unwrap()
}

const WOFF1: [(&str, &str); 3] = [
    ("TestWOFF.woff", "TestOTF.otf"),
    ("SourceSans3-Regular.ttf.woff", "SourceSans3-Regular.ttf"),
    ("SourceSans3-Regular.otf.woff", "SourceSans3-Regular.otf"),
];
const WOFF2: [(&str, &str); 3] = [
    ("TestWOFF2.woff2", "TestTTF.ttf"),
    ("SourceSans3-Regular.ttf.woff2", "SourceSans3-Regular.ttf"),
    ("SourceSans3-Regular.otf.woff2", "SourceSans3-Regular.otf"),
];

#[test]
fn woff1_tables_equal_the_source_font() {
    let Some(c) = common::corpus() else { return };
    for (woff, src) in WOFF1 {
        let out = decode_woff(&read(&c, woff)).unwrap_or_else(|e| panic!("{woff}: {e}"));
        let (got, want) = (tables(&out, 0), tables(&read(&c, src), 0));
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            want.keys().collect::<Vec<_>>(),
            "{woff}: tag set"
        );
        for (tag, w) in &want {
            // The fonttools TestWOFF fixture was re-saved: its head (checkSumAdjustment, dates) is its own.
            if tag == "head" && woff == "TestWOFF.woff" {
                continue;
            }
            assert!(&got[tag] == w, "{woff}: table {tag} differs from {src}");
        }
        assert_eq!(&out[0..4], &read(&c, src)[0..4], "{woff}: sfnt version");
    }
}

/// Source-table comparison for the tables a WOFF2 round trip leaves untouched.
#[test]
fn woff2_tables_equal_the_source_font() {
    let Some(c) = common::corpus() else { return };
    for (woff2, src) in WOFF2 {
        let out = decode_woff2(&read(&c, woff2)).unwrap_or_else(|e| panic!("{woff2}: {e}"));
        let (got, want) = (tables(&out, 0), tables(&read(&c, src), 0));
        // The corpus WOFF2 files were encoded from hinting-stripped variants of
        // the corpus fonts (smaller glyf/loca/name, different maxp), so only
        // tables that are independent of that are comparable to the source.
        let mut compared = 0;
        for (tag, g) in &got {
            if matches!(
                tag.as_str(),
                "BASE"
                    | "GDEF"
                    | "GPOS"
                    | "GSUB"
                    | "OS/2"
                    | "cmap"
                    | "hhea"
                    | "hmtx"
                    | "post"
                    | "CFF "
            ) {
                if let Some(w) = want.get(tag) {
                    if g == w {
                        compared += 1;
                    } else if woff2 != "TestWOFF2.woff2" {
                        panic!("{woff2}: table {tag} differs from {src}");
                    }
                }
            }
        }
        assert!(compared >= 3, "{woff2}: only {compared} tables compared");
    }
}

#[test]
fn woff2_collection_members_carry_the_ttc_tables() {
    let Some(c) = common::corpus() else { return };
    let w = read(&c, "woff2-collection-4members.woff2");
    assert!(is_woff2_collection(&w));
    let out = decode_woff2_collection(&w).unwrap();
    let mut want = fonts_of(&read(&c, "ttc-4members-shared.ttc"));
    want.iter_mut().for_each(|f| {
        f.remove("DSIG"); // the WOFF2 encoder drops DSIG
    });
    let got = fonts_of(&out);
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(
            g.keys().collect::<Vec<_>>(),
            w.keys().collect::<Vec<_>>(),
            "member {i}"
        );
        for (tag, t) in w {
            if !matches!(tag.as_str(), "head" | "glyf" | "loca" | "DSIG") {
                assert!(&g[tag] == t, "member {i}: table {tag}");
            }
        }
    }
    assert!(
        decode_woff2(&w).is_err(),
        "collection must go through decode_woff2_collection"
    );
}

/// Byte comparison with the pinned Google `woff2_decompress`; the reference
/// outputs come from scripts/woff2-reference.sh.
#[test]
fn woff2_output_is_byte_equal_to_woff2_decompress() {
    if common::corpus().is_none() {
        return;
    }
    let Some(dir) = std::env::var_os("FONTISAN_WOFF2_REF_DIR").map(PathBuf::from) else {
        eprintln!("SKIPPED: FONTISAN_WOFF2_REF_DIR not set (see scripts/woff2-reference.sh)");
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "woff2"))
        .collect();
    files.sort();
    // The corpus files must be among them (the dir may add derived variants).
    let names: Vec<String> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for want in WOFF2
        .iter()
        .map(|p| p.0)
        .chain(["woff2-collection-4members.woff2"])
    {
        assert!(
            names.iter().any(|n| n == want),
            "reference dir lacks {want}"
        );
    }
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let input = std::fs::read(&path).unwrap();
        let ours = if is_woff2_collection(&input) {
            decode_woff2_collection(&input)
        } else {
            decode_woff2(&input)
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let theirs = std::fs::read(dir.join(format!("{name}.ref"))).unwrap();
        assert_eq!(ours.len(), theirs.len(), "{name}: length");
        assert!(ours == theirs, "{name}: bytes differ from woff2_decompress");
        assert_eq!(fonts_of(&ours), fonts_of(&theirs), "{name}: tables");
        eprintln!("byte-equal: {name} ({} bytes)", ours.len());
    }
}

/// Truncated and bit-flipped inputs must be errors (or, rarely, a valid
/// decode), never a panic. The header length field is patched to the new
/// length so the parser gets past the cheap length check.
#[test]
fn malformed_inputs_never_panic() {
    let Some(c) = common::corpus() else { return };
    let mut inputs: Vec<(&str, Vec<u8>)> = Vec::new();
    for name in WOFF1
        .iter()
        .chain(&WOFF2)
        .map(|p| p.0)
        .chain(["woff2-collection-4members.woff2"])
    {
        inputs.push((name, read(&c, name)));
    }
    for (name, full) in &inputs {
        let is2 = &full[0..4] == b"wOF2";
        let run = |data: &[u8]| {
            let r = std::panic::catch_unwind(|| {
                if is2 {
                    let _ = decode_woff2(data);
                    let _ = decode_woff2_collection(data);
                } else {
                    let _ = decode_woff(data);
                }
            });
            assert!(r.is_ok(), "{name}: panic on {} bytes", data.len());
        };
        // Every short prefix, plain and with the length patched.
        let step = (full.len() / 300).max(1);
        for n in (0..full.len()).filter(|n| *n < 600 || n % step == 0) {
            let mut cut = full[..n].to_vec();
            run(&cut);
            if n >= 12 {
                cut[8..12].copy_from_slice(&(n as u32).to_be_bytes());
                run(&cut);
            }
        }
        // Single-byte corruption across the first 400 bytes and a stride after (kept small: debug-build brotli is slow).
        let step = (full.len() / 150).max(1);
        for at in (0..full.len()).filter(|a| *a < 400 || a % step == 0) {
            for v in [0xff, full[at] ^ 0x55] {
                let mut bad = full.clone();
                bad[at] = v;
                run(&bad);
            }
        }
        // Cleanly rejected, specifically: any strict prefix of a valid file is an error.
        for n in [0, 1, 3, 11, 43, 47, full.len() / 2, full.len() - 1] {
            let cut = &full[..n];
            let res = if is2 {
                decode_woff2(cut)
                    .map(|_| ())
                    .or_else(|_| decode_woff2_collection(cut).map(|_| ()))
            } else {
                decode_woff(cut).map(|_| ())
            };
            assert!(res.is_err(), "{name}: {n}-byte prefix decoded");
        }
    }
    assert!(decode_woff(b"").is_err());
    assert!(decode_woff2(b"wOF2").is_err());
    assert!(decode_woff(b"wOF2\0\0\0\0").is_err());
}
