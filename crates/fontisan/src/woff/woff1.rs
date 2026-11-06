//! WOFF 1.0: zlib per table, tables stored in sfnt directory order.

use super::{MAX_OUTPUT, Reader, Result, err, offset_table, round4};

const SIGNATURE: u32 = 0x774F_4646; // 'wOFF'

struct Entry {
    tag: u32,
    offset: u32,
    comp_length: u32,
    orig_length: u32,
    checksum: u32,
}

/// Decode a WOFF 1.0 file to sfnt bytes. Table payloads (and checksums) are
/// the original ones; the sfnt is laid out with 4-byte aligned tables in
/// directory order.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    if r.u32()? != SIGNATURE {
        return err("not a WOFF file (bad signature)");
    }
    let flavor = r.u32()?;
    if u64::from(r.u32()?) != data.len() as u64 {
        return err("header length does not match file length");
    }
    let num_tables = r.u16()?;
    if num_tables == 0 {
        return err("no tables");
    }
    r.skip(2 + 4 + 4)?; // reserved, totalSfntSize, major/minor version
    let meta_offset = r.u32()?;
    let meta_length = r.u32()?;
    r.skip(4)?; // metaOrigLength
    let priv_offset = r.u32()?;
    let priv_length = r.u32()?;
    for (off, len) in [(meta_offset, meta_length), (priv_offset, priv_length)] {
        if off != 0 && u64::from(off) + u64::from(len) > data.len() as u64 {
            return err("metadata or private block out of range");
        }
    }

    let mut entries = Vec::with_capacity(usize::from(num_tables));
    let mut total: u64 = 12 + 16 * u64::from(num_tables);
    for _ in 0..num_tables {
        let e = Entry {
            tag: r.u32()?,
            offset: r.u32()?,
            comp_length: r.u32()?,
            orig_length: r.u32()?,
            checksum: r.u32()?,
        };
        if e.comp_length > e.orig_length {
            return err("table compLength exceeds origLength");
        }
        if u64::from(e.offset) + u64::from(e.comp_length) > data.len() as u64 {
            return err("table data out of range");
        }
        total += round4(u64::from(e.orig_length));
        if total > MAX_OUTPUT {
            return err("decoded size too large");
        }
        entries.push(e);
    }
    if entries.windows(2).any(|w| w[0].tag >= w[1].tag) {
        return err("table directory not sorted by unique tag");
    }

    let mut out = Vec::with_capacity(total.min(1 << 24) as usize);
    out.extend_from_slice(&offset_table(flavor, num_tables));
    let mut table_offset = 12 + 16 * u64::from(num_tables);
    for e in &entries {
        out.extend_from_slice(&e.tag.to_be_bytes());
        out.extend_from_slice(&e.checksum.to_be_bytes());
        out.extend_from_slice(&(table_offset as u32).to_be_bytes());
        out.extend_from_slice(&e.orig_length.to_be_bytes());
        table_offset += round4(u64::from(e.orig_length));
    }
    for e in &entries {
        let start = e.offset as usize;
        let src = &data[start..start + e.comp_length as usize];
        if e.comp_length < e.orig_length {
            let plain = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                src,
                e.orig_length as usize,
            )
            .map_err(|_| super::Error("zlib stream is corrupt or too long".into()))?;
            if plain.len() != e.orig_length as usize {
                return err("inflated size does not match origLength");
            }
            out.extend_from_slice(&plain);
        } else {
            out.extend_from_slice(src);
        }
        out.resize(round4(out.len() as u64) as usize, 0);
    }
    Ok(out)
}
