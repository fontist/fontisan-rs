//! WOFF 1.0 encode, as `Converters::WoffWriter` (zlib level 6, tables under
//! 100 bytes or that do not shrink stay stored). The table directory is in
//! ascending tag order, as WOFF 1.0 section 5 requires.

use super::Tables;
use crate::woff::checksum;

pub(crate) fn encode(tables: &Tables) -> Vec<u8> {
    struct T<'a> {
        tag: [u8; 4],
        orig: &'a [u8],
        stored: Vec<u8>,
    }
    let mut ts: Vec<T> = tables
        .iter()
        .map(|(tag, data)| {
            let mut stored = data.clone();
            if data.len() >= 100 {
                let z = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
                if z.len() < data.len() {
                    stored = z;
                }
            }
            T {
                tag: *tag,
                orig: data,
                stored,
            }
        })
        .collect();
    ts.sort_by(|a, b| a.tag.cmp(&b.tag));
    let n = ts.len();
    let mut cursor = 44 + 20 * n;
    let mut dir = Vec::with_capacity(20 * n);
    let mut body = Vec::new();
    let mut sfnt_size = 12 + 16 * n;
    for t in &ts {
        dir.extend_from_slice(&t.tag);
        dir.extend_from_slice(&(cursor as u32).to_be_bytes());
        dir.extend_from_slice(&(t.stored.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(t.orig.len() as u32).to_be_bytes());
        dir.extend_from_slice(&checksum(t.orig).to_be_bytes());
        body.extend_from_slice(&t.stored);
        body.resize(body.len().next_multiple_of(4), 0);
        cursor = 44 + 20 * n + body.len();
        sfnt_size += t.orig.len().next_multiple_of(4);
    }
    let is_cff = ts.iter().any(|t| &t.tag == b"CFF " || &t.tag == b"CFF2");
    let flavor: u32 = if is_cff { 0x4F54_544F } else { 0x0001_0000 };
    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(cursor as u32).to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 20]); // meta offset/length/orig, private offset/length
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn directory_is_sorted_by_tag_whatever_the_input_order() {
        let tables = vec![
            (*b"loca", vec![0u8; 4]),
            (*b"OS/2", vec![0u8; 4]),
            (*b"head", vec![0u8; 54]),
        ];
        let out = super::encode(&tables);
        let tags: Vec<&[u8]> = (0..3).map(|i| &out[44 + 20 * i..48 + 20 * i]).collect();
        assert_eq!(tags, [&b"OS/2"[..], b"head", b"loca"]);
    }
}
