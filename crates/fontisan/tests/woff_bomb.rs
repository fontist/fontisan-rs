//! Decoded-output cap (round 2, item 1): WOFF and WOFF2 refuse to materialise more than
//! 512 MiB, so a small padded file declaring a huge table cannot take gigabytes. The
//! bombs are constructed here; nothing large is allocated before the rejection.

const CAP: u64 = 512 << 20;

fn woff1_declaring(orig_each: u32, tables: u16) -> Vec<u8> {
    // Table data is one byte per table, zlib-less (compLength < origLength is the claim).
    let dir = 44 + 20 * usize::from(tables);
    let mut d = Vec::new();
    d.extend_from_slice(b"wOFF");
    d.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    d.extend_from_slice(&((dir + usize::from(tables)) as u32).to_be_bytes());
    d.extend_from_slice(&tables.to_be_bytes());
    d.resize(44, 0);
    for i in 0..tables {
        d.extend_from_slice(&[b'a' + (i as u8), b'b', b'c', b'd']);
        d.extend_from_slice(&((dir + usize::from(i)) as u32).to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&orig_each.to_be_bytes());
        d.extend_from_slice(&0u32.to_be_bytes());
    }
    d.resize(dir + usize::from(tables), 0);
    d
}

#[test]
fn woff1_total_over_the_cap_is_rejected_before_any_allocation() {
    // Three tables of 200 MiB: 600 MiB > 512 MiB, each ratio-implausible anyway.
    let f = woff1_declaring(200 << 20, 3);
    let e = fontisan::woff::decode_woff(&f).unwrap_err();
    assert!(e.to_string().contains("decoded size too large"), "{e}");
}

#[test]
fn woff1_total_just_under_the_cap_passes_the_size_gate() {
    // 2 x 255 MiB = 510 MiB + header: under the cap, so the ratio gate (not the size gate) fires.
    let f = woff1_declaring(255 << 20, 2);
    let e = fontisan::woff::decode_woff(&f).unwrap_err();
    assert!(
        e.to_string().contains("implausible compression ratio"),
        "{e}"
    );
}

/// WOFF2 with one `cmap` table declaring `orig` bytes, then `payload` bytes of "brotli".
fn woff2_declaring(orig: u32, payload: usize) -> Vec<u8> {
    let mut base128 = Vec::new();
    let mut v = orig;
    let mut groups = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        groups.push(0x80 | (v & 0x7f) as u8);
        v >>= 7;
    }
    groups.reverse();
    base128.extend(groups);
    let mut dir = vec![0u8]; // flags: known tag index 0 (cmap), no transform
    dir.extend(base128);
    // The decoder rounds the end of the compressed block up to 4 bytes: pad the file to match.
    let total = (48 + dir.len() + payload).next_multiple_of(4);
    let mut d = Vec::new();
    d.extend_from_slice(b"wOF2");
    d.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    d.extend_from_slice(&(total as u32).to_be_bytes());
    d.extend_from_slice(&1u16.to_be_bytes());
    d.extend_from_slice(&0u16.to_be_bytes());
    d.extend_from_slice(&orig.to_be_bytes()); // totalSfntSize
    d.extend_from_slice(&(payload as u32).to_be_bytes()); // totalCompressedSize
    d.resize(48, 0);
    d.extend(dir);
    d.resize(total, 0xAA);
    d
}

#[test]
fn woff2_claim_over_the_cap_is_rejected_by_both_decoders() {
    // 600 MiB from a 7 MiB padded file: passes the 100:1 ratio cap, fails the 512 MiB cap.
    let orig = (CAP + (88 << 20)) as u32;
    let f = woff2_declaring(orig, 7 << 20);
    let e = fontisan::woff::decode_woff2(&f).unwrap_err().to_string();
    assert!(e.contains("decoded size too large"), "{e}");
    let e = fontisan::sfnt::woff2_to_sfnt(&f).unwrap_err().to_string();
    assert!(e.contains("decode limit"), "{e}");
}

#[test]
fn woff2_claim_under_the_cap_is_not_stopped_by_the_size_gate() {
    // 100 MiB claimed from a 2 MiB file: gets past both size gates and fails inside brotli.
    let f = woff2_declaring(100 << 20, 2 << 20);
    let e = fontisan::woff::decode_woff2(&f).unwrap_err().to_string();
    assert!(
        !e.contains("too large") && !e.contains("implausible"),
        "{e}"
    );
}
