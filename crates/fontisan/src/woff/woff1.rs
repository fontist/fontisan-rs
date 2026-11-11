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

/// `(tag, table bytes)` in directory order.
pub(crate) type RawTables = Vec<([u8; 4], Vec<u8>)>;

/// Validated header and table directory of a WOFF 1.0 file.
struct Parsed {
    flavor: u32,
    entries: Vec<Entry>,
    /// Size of the decoded sfnt (header, directory and 4-byte aligned tables).
    total: u64,
}

fn parse(data: &[u8]) -> Result<Parsed> {
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
    Ok(Parsed {
        flavor,
        entries,
        total,
    })
}

/// Inflate one zlib-compressed table into `buf` (exactly `buf.len()` bytes).
fn inflate_into(src: &[u8], e: &Entry, buf: &mut [u8]) -> Result<()> {
    let want = e.orig_length as usize;
    let (status, _, written) = miniz_oxide::inflate::core::decompress(
        &mut miniz_oxide::inflate::core::DecompressorOxide::new(),
        src,
        buf,
        0,
        miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
            | miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    if status != miniz_oxide::inflate::TINFLStatus::Done {
        return Err(super::Error("zlib stream is corrupt or too long".into()));
    }
    if written != want {
        return err("inflated size does not match origLength");
    }
    Ok(())
}

fn compressed<'a>(data: &'a [u8], e: &Entry) -> &'a [u8] {
    let start = e.offset as usize;
    &data[start..start + e.comp_length as usize]
}

/// Deflate cannot exceed ~1032:1, so a larger claim is a lie that would otherwise
/// reserve and zero up to MAX_OUTPUT before the stream fails.
fn check_ratio(e: &Entry) -> Result<()> {
    if u64::from(e.orig_length) > u64::from(e.comp_length) * 1100 + 1024 {
        return err("implausible compression ratio");
    }
    Ok(())
}

/// Decode a WOFF 1.0 file to sfnt bytes. Table payloads (and checksums) are
/// the original ones; the sfnt is laid out with 4-byte aligned tables in
/// directory order.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>> {
    let Parsed {
        flavor,
        entries,
        total,
    } = parse(data)?;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&offset_table(flavor, entries.len() as u16));
    let mut table_offset = 12 + 16 * entries.len() as u64;
    for e in &entries {
        out.extend_from_slice(&e.tag.to_be_bytes());
        out.extend_from_slice(&e.checksum.to_be_bytes());
        out.extend_from_slice(&(table_offset as u32).to_be_bytes());
        out.extend_from_slice(&e.orig_length.to_be_bytes());
        table_offset += round4(u64::from(e.orig_length));
    }
    for e in &entries {
        let src = compressed(data, e);
        if e.comp_length < e.orig_length {
            // Inflate straight into `out`: a separate `Vec` plus the copy into `out`
            // tripled the peak resident size of a decompression-bomb table.
            check_ratio(e)?;
            let start = out.len();
            out.resize(start + e.orig_length as usize, 0);
            inflate_into(src, e, &mut out[start..])?;
        } else {
            out.extend_from_slice(src);
        }
        out.resize(round4(out.len() as u64) as usize, 0);
    }
    Ok(out)
}

/// The sfnt version (flavor) and `(tag, table bytes)` of a WOFF 1.0 file, each table
/// inflated into its own buffer. Equal to reading the tables out of [`decode_woff`]'s
/// output, without materialising that output beside them (which doubled the peak).
pub(crate) fn decode_tables(data: &[u8]) -> Result<(u32, RawTables)> {
    let Parsed {
        flavor, entries, ..
    } = parse(data)?;
    let mut tables = Vec::with_capacity(entries.len());
    for e in &entries {
        let src = compressed(data, e);
        let table = if e.comp_length < e.orig_length {
            check_ratio(e)?;
            let mut buf = vec![0u8; e.orig_length as usize];
            inflate_into(src, e, &mut buf)?;
            buf
        } else {
            src.to_vec()
        };
        tables.push((e.tag.to_be_bytes(), table));
    }
    Ok((flavor, tables))
}

#[cfg(test)]
mod tests {
    use super::decode_woff;

    /// One-table WOFF1 whose directory claims `orig` bytes for the zlib payload `z`.
    fn woff_with(z: &[u8], orig: u32) -> Vec<u8> {
        let total = 44 + 20 + z.len();
        let mut d = Vec::new();
        d.extend_from_slice(b"wOFF");
        d.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        d.extend_from_slice(&(total as u32).to_be_bytes());
        d.extend_from_slice(&1u16.to_be_bytes());
        d.resize(44, 0);
        d.extend_from_slice(b"glyf");
        d.extend_from_slice(&64u32.to_be_bytes());
        d.extend_from_slice(&(z.len() as u32).to_be_bytes());
        d.extend_from_slice(&orig.to_be_bytes());
        d.extend_from_slice(&0u32.to_be_bytes());
        d.extend_from_slice(z);
        d
    }

    #[test]
    fn inflates_into_the_output_with_the_declared_length() {
        let plain: Vec<u8> = (0..200_000u32).map(|i| (i % 7) as u8).collect();
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
        let sfnt = decode_woff(&woff_with(&z, plain.len() as u32)).unwrap();
        assert_eq!(&sfnt[12 + 16..12 + 16 + plain.len()], &plain[..]);
    }

    #[test]
    fn decode_tables_equals_the_tables_of_the_decoded_sfnt() {
        let plain: Vec<u8> = (0..1001u32).map(|i| (i % 7) as u8).collect();
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
        let file = woff_with(&z, plain.len() as u32);
        let (flavor, tables) = super::decode_tables(&file).unwrap();
        assert_eq!(flavor, 0x0001_0000);
        assert_eq!(tables, vec![(*b"glyf", plain)]);
    }

    #[test]
    fn rejects_a_length_claim_no_deflate_stream_can_reach() {
        // 256 MiB claimed from a 100-byte payload: must fail before reserving it.
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8; 100], 6);
        let e = decode_woff(&woff_with(&z, 1 << 28)).unwrap_err();
        assert!(
            e.to_string().contains("implausible compression ratio"),
            "{e}"
        );
    }

    #[test]
    fn rejects_a_size_mismatch_in_either_direction() {
        let plain = vec![7u8; 5000];
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
        assert!(decode_woff(&woff_with(&z, 4000)).is_err());
        assert!(decode_woff(&woff_with(&z, 6000)).is_err());
    }
}
