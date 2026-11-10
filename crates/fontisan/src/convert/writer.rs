//! Port of `Fontisan::FontWriter`.

use super::Tables;
use crate::woff::checksum;

// Ruby spells these lists with %w[...], so `cvt ` and `CFF ` (trailing space)
// split into `cvt` and `CFF`, which never match a tag: both sort among the rest.
const TRUETYPE_ORDER: [&[u8; 4]; 18] = [
    b"head", b"hhea", b"maxp", b"OS/2", b"hmtx", b"LTSH", b"VDMX", b"hdmx", b"cmap", b"fpgm",
    b"prep", b"loca", b"glyf", b"kern", b"name", b"post", b"gasp", b"PCLT",
];
const OPENTYPE_ORDER: [&[u8; 4]; 7] = [
    b"head", b"hhea", b"maxp", b"OS/2", b"name", b"cmap", b"post",
];

/// `FontWriter.write_font(tables, sfnt_version:)`.
pub fn write_font(tables: &Tables, sfnt_version: u32) -> Vec<u8> {
    let has = |t: &[u8; 4]| tables.iter().any(|(k, _)| k == t);
    let is_cff = has(b"CFF ") || has(b"CFF2");
    let mut order: Vec<[u8; 4]> = Vec::new();
    let preferred: Vec<&[u8; 4]> = if is_cff {
        let mut v: Vec<&[u8; 4]> = OPENTYPE_ORDER.to_vec();
        v.push(b"CFF2");
        v
    } else {
        let mut v: Vec<&[u8; 4]> = TRUETYPE_ORDER.to_vec();
        v.push(b"DSIG");
        v
    };
    for t in preferred {
        if has(t) {
            order.push(*t);
        }
    }
    let mut rest: Vec<[u8; 4]> = tables
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| !order.contains(t))
        .collect();
    rest.sort();
    order.extend(rest);

    let n = order.len();
    let mut max_power = 0u32;
    let mut m = n;
    while m > 1 {
        m >>= 1;
        max_power += 1;
    }
    let search_range = (1u32 << max_power) * 16;
    let range_shift = (n as u32 * 16).wrapping_sub(search_range);

    let mut out = Vec::new();
    out.extend_from_slice(&sfnt_version.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&(search_range as u16).to_be_bytes());
    out.extend_from_slice(&(max_power as u16).to_be_bytes());
    out.extend_from_slice(&(range_shift as u16).to_be_bytes());

    let mut offset = 12 + 16 * n as u32;
    let mut head_offset = None;
    for tag in &order {
        let data = &tables
            .iter()
            .find(|(t, _)| t == tag)
            .expect("tag from tables")
            .1;
        let sum = if tag == b"head" && data.len() >= 12 {
            let mut z = data.clone();
            z[8..12].fill(0);
            checksum(&z)
        } else {
            checksum(data)
        };
        if tag == b"head" {
            head_offset = Some(offset as usize);
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() as u32 + 3) & !3;
    }
    for tag in &order {
        let data = &tables
            .iter()
            .find(|(t, _)| t == tag)
            .expect("tag from tables")
            .1;
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    if let Some(h) = head_offset {
        if out.len() < h + 12 {
            out.resize(h + 12, 0); // IO#seek + write grows a file that ends inside head
        }
        out[h + 8..h + 12].fill(0);
        set_checksum_adjustment(&mut out, h + 8);
    }
    out
}

/// Sum the whole file as it is and store `0xB1B0AFBA - sum` at `at` (growing
/// the file when `at` lies past its end, like a Ruby `IO#seek` + `write`).
pub(crate) fn set_checksum_adjustment(out: &mut Vec<u8>, at: usize) {
    let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(out));
    if out.len() < at + 4 {
        out.resize(at + 4, 0);
    }
    out[at..at + 4].copy_from_slice(&adj.to_be_bytes());
}

#[cfg(test)]
mod short_head_tests {
    use super::*;

    #[test]
    fn head_shorter_than_twelve_bytes_is_grown_not_a_panic() {
        // Ruby's FontWriter zeroes checkSumAdjustment with IO#seek + write, which
        // grows a file that ends inside head; the Rust slice index used to panic.
        let tables: Tables = vec![(*b"head", vec![1, 2, 3, 4])];
        let out = write_font(&tables, 0x0001_0000);
        let h = 12 + 16;
        assert_eq!(&out[h..h + 4], &[1, 2, 3, 4]);
        assert!(out.len() >= h + 12);
    }
}
