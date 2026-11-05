//! WOFF 2.0: one brotli stream, optional glyf/loca/hmtx transforms, optional
//! collections. Mirrors the pinned Google decoder's output layout.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use super::{Error, MAX_OUTPUT, Reader, Result, checksum, err, offset_table, round4};

const SIGNATURE: u32 = 0x774F_4632; // 'wOF2'
const TTCF: u32 = 0x7474_6366; // 'ttcf'
const TAG_GLYF: u32 = u32::from_be_bytes(*b"glyf");
const TAG_LOCA: u32 = u32::from_be_bytes(*b"loca");
const TAG_HMTX: u32 = u32::from_be_bytes(*b"hmtx");
const TAG_HHEA: u32 = u32::from_be_bytes(*b"hhea");
const TAG_HEAD: u32 = u32::from_be_bytes(*b"head");

pub(crate) const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

/// Same plausibility bound as the reference decoder.
const MAX_COMPRESSION_RATIO: f32 = 100.0;

#[derive(Clone)]
struct Table {
    tag: u32,
    transformed: bool,
    src_offset: u32,
    src_length: u32,
    dst_offset: u32,
    dst_length: u32,
}

struct TtcFont {
    flavor: u32,
    table_indices: Vec<usize>,
}

struct Header {
    flavor: u32,
    header_version: u32,
    compressed_offset: usize,
    compressed_length: usize,
    uncompressed_size: u32,
    tables: Vec<Table>,
    fonts: Vec<TtcFont>,
}

/// Does this look like a WOFF2 collection (flavor `ttcf`)?
pub fn is_woff2_collection(data: &[u8]) -> bool {
    data.len() >= 8 && data[0..4] == SIGNATURE.to_be_bytes() && data[4..8] == TTCF.to_be_bytes()
}

/// Decode a single-font WOFF2 file to sfnt bytes. A collection is an error
/// here; use [`decode_woff2_collection`].
pub fn decode_woff2(data: &[u8]) -> Result<Vec<u8>> {
    if is_woff2_collection(data) {
        return err("WOFF2 collection: use decode_woff2_collection");
    }
    convert(data)
}

/// Decode a WOFF2 collection to `ttcf` bytes (shared tables stored once).
pub fn decode_woff2_collection(data: &[u8]) -> Result<Vec<u8>> {
    if !is_woff2_collection(data) {
        return err("not a WOFF2 collection");
    }
    convert(data)
}

fn read_base128(r: &mut Reader) -> Result<u32> {
    let mut acc: u32 = 0;
    for i in 0..5 {
        let b = r.u8()?;
        if i == 0 && b == 0x80 {
            return err("base128 with leading zero");
        }
        if acc & 0xFE00_0000 != 0 {
            return err("base128 overflow");
        }
        acc = (acc << 7) | u32::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Ok(acc);
        }
    }
    err("base128 too long")
}

fn read_255_u16(r: &mut Reader) -> Result<u32> {
    let code = r.u8()?;
    Ok(match code {
        253 => u32::from(r.u16()?),
        255 => u32::from(r.u8()?) + 253,
        254 => u32::from(r.u8()?) + 506,
        c => u32::from(c),
    })
}

fn read_table_directory(r: &mut Reader, n: usize) -> Result<Vec<Table>> {
    let mut tables = Vec::with_capacity(n);
    let mut src_offset: u32 = 0;
    for _ in 0..n {
        let flag = r.u8()?;
        let tag = if flag & 0x3f == 0x3f {
            r.u32()?
        } else {
            u32::from_be_bytes(*KNOWN_TAGS[usize::from(flag & 0x3f)])
        };
        let version = (flag >> 6) & 3;
        // Version 0 means transformed for glyf/loca; non-zero for the rest.
        let transformed = if tag == TAG_GLYF || tag == TAG_LOCA {
            version == 0
        } else {
            version != 0
        };
        let dst_length = read_base128(r)?;
        let mut transform_length = dst_length;
        if transformed {
            transform_length = read_base128(r)?;
            if tag == TAG_LOCA && transform_length != 0 {
                return err("transformed loca must have zero length");
            }
        }
        let next = src_offset
            .checked_add(transform_length)
            .ok_or_else(|| Error("table offsets overflow".into()))?;
        tables.push(Table {
            tag,
            transformed,
            src_offset,
            src_length: transform_length,
            dst_offset: 0,
            dst_length,
        });
        src_offset = next;
    }
    Ok(tables)
}

fn read_header(data: &[u8]) -> Result<Header> {
    let mut r = Reader::new(data);
    if r.u32()? != SIGNATURE {
        return err("not a WOFF2 file (bad signature)");
    }
    let flavor = r.u32()?;
    if u64::from(r.u32()?) != data.len() as u64 {
        return err("header length does not match file length");
    }
    let num_tables = r.u16()?;
    if num_tables == 0 {
        return err("no tables");
    }
    r.skip(6)?; // reserved, totalSfntSize
    let compressed_length = r.u32()?;
    r.skip(4)?; // major/minor version
    let meta_offset = r.u32()?;
    let meta_length = r.u32()?;
    r.skip(4)?; // metaOrigLength
    let priv_offset = r.u32()?;
    let priv_length = r.u32()?;
    let len = data.len() as u64;
    for (off, l) in [(meta_offset, meta_length), (priv_offset, priv_length)] {
        if off != 0 && (u64::from(off) >= len || len - u64::from(off) < u64::from(l)) {
            return err("metadata or private block out of range");
        }
    }
    let tables = read_table_directory(&mut r, usize::from(num_tables))?;
    let last = tables.last().ok_or_else(|| Error("no tables".into()))?;
    let uncompressed_size = last
        .src_offset
        .checked_add(last.src_length)
        .ok_or_else(|| Error("uncompressed size overflow".into()))?;

    let mut header_version = 0;
    let mut fonts = Vec::new();
    if flavor == TTCF {
        header_version = r.u32()?;
        if header_version != 0x0001_0000 && header_version != 0x0002_0000 {
            return err("unsupported collection header version");
        }
        let num_fonts = read_255_u16(&mut r)?;
        if num_fonts == 0 {
            return err("collection without fonts");
        }
        for _ in 0..num_fonts {
            let n = read_255_u16(&mut r)?;
            if n == 0 {
                return err("collection font without tables");
            }
            let font_flavor = r.u32()?;
            let (mut glyf_idx, mut loca_idx) = (0usize, 0usize);
            let mut table_indices = Vec::new();
            for _ in 0..n {
                let idx = read_255_u16(&mut r)? as usize;
                if idx >= tables.len() {
                    return err("collection table index out of range");
                }
                table_indices.push(idx);
                match tables[idx].tag {
                    TAG_LOCA => loca_idx = idx,
                    TAG_GLYF => glyf_idx = idx,
                    _ => {}
                }
            }
            if (glyf_idx > 0 || loca_idx > 0) && (glyf_idx > loca_idx || loca_idx - glyf_idx != 1) {
                return err("collection font has non-consecutive glyf/loca");
            }
            fonts.push(TtcFont {
                flavor: font_flavor,
                table_indices,
            });
        }
    }

    let compressed_offset = r.pos();
    let mut src_offset = round4(compressed_offset as u64 + u64::from(compressed_length));
    if src_offset > len {
        return err("compressed data out of range");
    }
    if meta_offset != 0 {
        if src_offset != u64::from(meta_offset) {
            return err("metadata block misplaced");
        }
        src_offset = round4(u64::from(meta_offset) + u64::from(meta_length));
    }
    if priv_offset != 0 {
        if src_offset != u64::from(priv_offset) {
            return err("private block misplaced");
        }
        src_offset = round4(u64::from(priv_offset) + u64::from(priv_length));
    }
    if src_offset != round4(len) {
        return err("trailing or missing bytes after last block");
    }
    Ok(Header {
        flavor,
        header_version,
        compressed_offset,
        compressed_length: compressed_length as usize,
        uncompressed_size,
        tables,
        fonts,
    })
}

#[derive(Default)]
struct FontInfo {
    num_glyphs: u16,
    index_format: u16,
    num_hmetrics: u16,
    x_mins: Vec<i16>,
    entry_by_tag: HashMap<u32, usize>,
}

fn put16(v: &mut Vec<u8>, x: i32) {
    v.extend_from_slice(&(x as u16).to_be_bytes());
}

pub(crate) struct Point {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) on_curve: bool,
}

fn with_sign(flag: u8, base: i32) -> i32 {
    if flag & 1 != 0 { base } else { -base }
}

/// Decode `n` points from the flag stream and triplet stream. Returns the
/// points and the number of triplet bytes consumed.
pub(crate) fn triplet_decode(flags: &[u8], input: &[u8], n: usize) -> Result<(Vec<Point>, usize)> {
    if n > input.len() {
        return err("too many points for glyph stream");
    }
    let (mut x, mut y) = (0i32, 0i32);
    let mut idx = 0usize;
    let mut pts = Vec::with_capacity(n);
    for &raw in &flags[..n] {
        let on_curve = raw >> 7 == 0;
        let flag = raw & 0x7f;
        let nb = match flag {
            0..=83 => 1,
            84..=119 => 2,
            120..=123 => 3,
            _ => 4,
        };
        if idx + nb > input.len() {
            return err("glyph stream exhausted");
        }
        let b = &input[idx..idx + nb];
        let (dx, dy) = match flag {
            0..=9 => (
                0,
                with_sign(flag, (i32::from(flag & 14) << 7) + i32::from(b[0])),
            ),
            10..=19 => (
                with_sign(flag, (i32::from((flag - 10) & 14) << 7) + i32::from(b[0])),
                0,
            ),
            20..=83 => {
                let b0 = i32::from(flag - 20);
                let b1 = i32::from(b[0]);
                (
                    with_sign(flag, 1 + (b0 & 0x30) + (b1 >> 4)),
                    with_sign(flag >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)),
                )
            }
            84..=119 => {
                let b0 = i32::from(flag - 84);
                (
                    with_sign(flag, 1 + ((b0 / 12) << 8) + i32::from(b[0])),
                    with_sign(flag >> 1, 1 + (((b0 % 12) >> 2) << 8) + i32::from(b[1])),
                )
            }
            120..=123 => {
                let b2 = i32::from(b[1]);
                (
                    with_sign(flag, (i32::from(b[0]) << 4) + (b2 >> 4)),
                    with_sign(flag >> 1, ((b2 & 0x0f) << 8) + i32::from(b[2])),
                )
            }
            _ => (
                with_sign(flag, (i32::from(b[0]) << 8) + i32::from(b[1])),
                with_sign(flag >> 1, (i32::from(b[2]) << 8) + i32::from(b[3])),
            ),
        };
        idx += nb;
        x = x
            .checked_add(dx)
            .ok_or_else(|| Error("coordinate overflow".into()))?;
        y = y
            .checked_add(dy)
            .ok_or_else(|| Error("coordinate overflow".into()))?;
        pts.push(Point { x, y, on_curve });
    }
    Ok((pts, idx))
}

/// Append flags and coordinates of a simple glyph (reference `StorePoints`).
fn store_points(points: &[Point], has_overlap: bool, glyph: &mut Vec<u8>) {
    let mut flags: Vec<u8> = Vec::new();
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let (mut last_flag, mut repeat) = (-1i32, 0u32);
    let (mut lx, mut ly) = (0i32, 0i32);
    for (i, p) in points.iter().enumerate() {
        let mut flag: i32 = if p.on_curve { 1 } else { 0 };
        if has_overlap && i == 0 {
            flag |= 0x40;
        }
        let dx = p.x - lx;
        let dy = p.y - ly;
        if dx == 0 {
            flag |= 0x10;
        } else if dx > -256 && dx < 256 {
            flag |= 0x02 | if dx > 0 { 0x10 } else { 0 };
            xs.push(dx.unsigned_abs() as u8);
        } else {
            put16(&mut xs, dx);
        }
        if dy == 0 {
            flag |= 0x20;
        } else if dy > -256 && dy < 256 {
            flag |= 0x04 | if dy > 0 { 0x20 } else { 0 };
            ys.push(dy.unsigned_abs() as u8);
        } else {
            put16(&mut ys, dy);
        }
        if flag == last_flag && repeat != 255 {
            let at = flags.len() - 1 - usize::from(repeat != 0);
            flags[at] |= 0x08;
            repeat += 1;
            // the repeat count byte is rewritten in place below
            if repeat == 1 {
                flags.push(1);
            } else {
                *flags.last_mut().expect("repeat byte present") = repeat as u8;
            }
        } else {
            repeat = 0;
            flags.push(flag as u8);
        }
        lx = p.x;
        ly = p.y;
        last_flag = flag;
    }
    glyph.extend_from_slice(&flags);
    glyph.extend_from_slice(&xs);
    glyph.extend_from_slice(&ys);
}

fn composite_size(mut s: Reader) -> Result<(usize, bool)> {
    let start = s.pos();
    let mut have_instructions = false;
    let mut flags: u16 = 0x0020;
    while flags & 0x0020 != 0 {
        flags = s.u16()?;
        have_instructions |= flags & 0x0100 != 0;
        let mut arg = 2;
        arg += if flags & 0x0001 != 0 { 4 } else { 2 };
        if flags & 0x0008 != 0 {
            arg += 2;
        } else if flags & 0x0040 != 0 {
            arg += 4;
        } else if flags & 0x0080 != 0 {
            arg += 8;
        }
        s.skip(arg)?;
    }
    Ok((s.pos() - start, have_instructions))
}

struct GlyfOut {
    glyf_length: u32,
    glyf_checksum: u32,
    loca_offset: u32,
    loca_length: u32,
    loca_checksum: u32,
}

fn reconstruct_glyf(
    data: &[u8],
    loca_dst_length: u32,
    info: &mut FontInfo,
    out: &mut Vec<u8>,
) -> Result<GlyfOut> {
    let glyf_start = out.len();
    let mut file = Reader::new(data);
    let _version = file.u16()?;
    let flags = file.u16()?;
    let has_overlap_bitmap = flags & 1 != 0;
    info.num_glyphs = file.u16()?;
    info.index_format = file.u16()?;
    let num_glyphs = usize::from(info.num_glyphs);
    let expected =
        (if info.index_format != 0 { 4u32 } else { 2 }) * (u32::from(info.num_glyphs) + 1);
    if loca_dst_length != expected {
        return err("loca length does not match glyf header");
    }
    let mut offset = 36usize;
    if offset > data.len() {
        return err("glyf transform header truncated");
    }
    let mut sub: Vec<&[u8]> = Vec::with_capacity(7);
    for _ in 0..7 {
        let size = file.u32()? as usize;
        if size > data.len() - offset {
            return err("glyf substream out of range");
        }
        sub.push(&data[offset..offset + size]);
        offset += size;
    }
    let mut n_contour_s = Reader::new(sub[0]);
    let mut n_points_s = Reader::new(sub[1]);
    let mut flag_s = Reader::new(sub[2]);
    let mut glyph_s = Reader::new(sub[3]);
    let mut composite_s = Reader::new(sub[4]);
    let mut bbox_s = Reader::new(sub[5]);
    let mut instr_s = Reader::new(sub[6]);

    let mut overlap: &[u8] = &[];
    if has_overlap_bitmap {
        let n = (num_glyphs + 7) >> 3;
        if n > data.len() - offset {
            return err("overlap bitmap out of range");
        }
        overlap = &data[offset..offset + n];
    }
    let bitmap_len = ((num_glyphs + 31) >> 5) << 2;
    let bbox_bitmap = bbox_s.bytes(bitmap_len)?;

    let mut loca_values = vec![0u32; num_glyphs + 1];
    info.x_mins = vec![0i16; num_glyphs];
    let mut glyf_checksum = 0u32;
    let mut glyph: Vec<u8> = Vec::new();
    for i in 0..num_glyphs {
        glyph.clear();
        let have_bbox = bbox_bitmap[i >> 3] & (0x80 >> (i & 7)) != 0;
        let n_contours = n_contour_s.u16()?;
        if n_contours == 0xffff {
            if !have_bbox {
                return err("composite glyph without bbox");
            }
            let (csize, have_instr) = composite_size(composite_s.clone())?;
            let instr_size = if have_instr {
                read_255_u16(&mut glyph_s)? as usize
            } else {
                0
            };
            glyph.extend_from_slice(&n_contours.to_be_bytes());
            glyph.extend_from_slice(bbox_s.bytes(8)?);
            glyph.extend_from_slice(composite_s.bytes(csize)?);
            if have_instr {
                put16(&mut glyph, instr_size as i32);
                glyph.extend_from_slice(instr_s.bytes(instr_size)?);
            }
        } else if n_contours > 0 {
            let mut per_contour = Vec::new();
            let mut total: u32 = 0;
            for _ in 0..n_contours {
                let n = read_255_u16(&mut n_points_s)?;
                per_contour.push(n);
                total = total
                    .checked_add(n)
                    .ok_or_else(|| Error("point count overflow".into()))?;
            }
            let total = total as usize;
            if total > flag_s.remaining() {
                return err("flag stream exhausted");
            }
            let flags_buf = flag_s.clone().bytes(total)?;
            let rest = glyph_s.clone().bytes(glyph_s.remaining())?;
            let (points, used) = triplet_decode(flags_buf, rest, total)?;
            flag_s.skip(total)?;
            glyph_s.skip(used)?;
            let instr_size = read_255_u16(&mut glyph_s)? as usize;
            if total >= 1 << 27 || instr_size >= 1 << 30 {
                return err("glyph too large");
            }
            glyph.extend_from_slice(&n_contours.to_be_bytes());
            if have_bbox {
                glyph.extend_from_slice(bbox_s.bytes(8)?);
            } else {
                let (mut x0, mut y0, mut x1, mut y1) = (0, 0, 0, 0);
                if let Some(p) = points.first() {
                    (x0, x1, y0, y1) = (p.x, p.x, p.y, p.y);
                }
                for p in points.iter().skip(1) {
                    x0 = x0.min(p.x);
                    x1 = x1.max(p.x);
                    y0 = y0.min(p.y);
                    y1 = y1.max(p.y);
                }
                for v in [x0, y0, x1, y1] {
                    put16(&mut glyph, v);
                }
            }
            let mut end_point: i64 = -1;
            for n in per_contour {
                end_point += i64::from(n);
                if end_point >= 65536 {
                    return err("end point out of range");
                }
                put16(&mut glyph, end_point as i32);
            }
            put16(&mut glyph, instr_size as i32);
            glyph.extend_from_slice(instr_s.bytes(instr_size)?);
            let has_overlap = has_overlap_bitmap && overlap[i >> 3] & (0x80 >> (i & 7)) != 0;
            store_points(&points, has_overlap, &mut glyph);
        } else if have_bbox {
            return err("empty glyph with bbox");
        }
        loca_values[i] = (out.len() - glyf_start) as u32;
        out.extend_from_slice(&glyph);
        out.resize(round4(out.len() as u64) as usize, 0);
        glyf_checksum = glyf_checksum.wrapping_add(checksum(&glyph));
        if n_contours > 0 {
            info.x_mins[i] = i16::from_be_bytes([glyph[2], glyph[3]]);
        }
    }
    let glyf_length = (out.len() - glyf_start) as u32;
    loca_values[num_glyphs] = glyf_length;
    let loca_offset = out.len();
    let mut loca = Vec::with_capacity(loca_values.len() * 4);
    for v in &loca_values {
        if info.index_format != 0 {
            loca.extend_from_slice(&v.to_be_bytes());
        } else {
            loca.extend_from_slice(&((v >> 1) as u16).to_be_bytes());
        }
    }
    let loca_checksum = checksum(&loca);
    out.extend_from_slice(&loca);
    Ok(GlyfOut {
        glyf_length,
        glyf_checksum,
        loca_offset: loca_offset as u32,
        loca_length: loca.len() as u32,
        loca_checksum,
    })
}

fn reconstruct_hmtx(data: &[u8], info: &FontInfo, out: &mut Vec<u8>) -> Result<u32> {
    let mut r = Reader::new(data);
    let hflags = r.u8()?;
    let proportional = hflags & 1 == 0;
    let monospace = hflags & 2 == 0;
    if hflags & 0xFC != 0 {
        return err("reserved hmtx flag bits set");
    }
    if proportional && monospace {
        return err("hmtx marked transformed but nothing was dropped");
    }
    let (ng, nh) = (usize::from(info.num_glyphs), usize::from(info.num_hmetrics));
    if nh > ng {
        return err("numberOfHMetrics exceeds glyph count");
    }
    if nh < 1 {
        return err("numberOfHMetrics is zero");
    }
    let mut advances = Vec::with_capacity(nh);
    for _ in 0..nh {
        advances.push(r.u16()?);
    }
    let mut lsbs: Vec<i16> = Vec::with_capacity(ng);
    for i in 0..ng {
        let explicit = if i < nh { proportional } else { monospace };
        lsbs.push(if explicit {
            r.u16()? as i16
        } else {
            info.x_mins[i]
        });
    }
    let mut table = Vec::with_capacity(2 * ng + 2 * nh);
    for (i, lsb) in lsbs.iter().enumerate() {
        if i < nh {
            table.extend_from_slice(&advances[i].to_be_bytes());
        }
        table.extend_from_slice(&lsb.to_be_bytes());
    }
    out.extend_from_slice(&table);
    Ok(checksum(&table))
}

fn convert(data: &[u8]) -> Result<Vec<u8>> {
    let mut hdr = read_header(data)?;
    let collection = hdr.header_version != 0;
    let nfonts = if collection { hdr.fonts.len() } else { 1 };

    // Font table lists, output (tag) order for the directory.
    let mut font_tables: Vec<Vec<usize>> = Vec::with_capacity(nfonts);
    if collection {
        for f in &hdr.fonts {
            let mut by_tag = BTreeMap::new();
            for &i in &f.table_indices {
                if by_tag.insert(hdr.tables[i].tag, i).is_some() {
                    return err("duplicate table tag within a font");
                }
            }
            font_tables.push(by_tag.into_values().collect());
        }
    } else {
        font_tables.push((0..hdr.tables.len()).collect());
    }

    // Headers.
    let mut infos: Vec<FontInfo> = (0..nfonts).map(|_| FontInfo::default()).collect();
    let mut header_sums: Vec<u32> = vec![0; nfonts];
    let first_table_offset = if collection {
        let mut n = if hdr.header_version == 0x0002_0000 {
            12
        } else {
            0
        } + 12
            + 4 * nfonts;
        n += 12 * nfonts;
        for t in &font_tables {
            n += 16 * t.len();
        }
        n
    } else {
        12 + 16 * hdr.tables.len()
    };
    let mut out: Vec<u8> = vec![0; first_table_offset];
    if collection {
        out[0..4].copy_from_slice(&hdr.flavor.to_be_bytes());
        out[4..8].copy_from_slice(&hdr.header_version.to_be_bytes());
        out[8..12].copy_from_slice(&(nfonts as u32).to_be_bytes());
        let mut offset_table_pos = 12;
        let mut offset = 12
            + 4 * nfonts
            + if hdr.header_version == 0x0002_0000 {
                12
            } else {
                0
            };
        for (fi, f) in hdr.fonts.iter().enumerate() {
            out[offset_table_pos..offset_table_pos + 4]
                .copy_from_slice(&(offset as u32).to_be_bytes());
            offset_table_pos += 4;
            let start = offset;
            let tags = &font_tables[fi];
            out[offset..offset + 12].copy_from_slice(&offset_table(f.flavor, tags.len() as u16));
            offset += 12;
            for &ti in tags {
                let tag = hdr.tables[ti].tag;
                out[offset..offset + 4].copy_from_slice(&tag.to_be_bytes());
                infos[fi].entry_by_tag.insert(tag, offset);
                offset += 16;
            }
            header_sums[fi] = checksum(&out[start..offset]);
        }
    } else {
        let mut sorted: Vec<u32> = hdr.tables.iter().map(|t| t.tag).collect();
        sorted.sort_unstable();
        if sorted.windows(2).any(|w| w[0] == w[1]) {
            return err("duplicate table tag");
        }
        out[0..12].copy_from_slice(&offset_table(hdr.flavor, hdr.tables.len() as u16));
        let mut offset = 12;
        for tag in sorted {
            out[offset..offset + 4].copy_from_slice(&tag.to_be_bytes());
            infos[0].entry_by_tag.insert(tag, offset);
            offset += 16;
        }
        header_sums[0] = checksum(&out);
    }
    let overall_header_sum = checksum(&out);

    if hdr.uncompressed_size < 1 {
        return err("empty table data");
    }
    if hdr.uncompressed_size as f32 / data.len() as f32 > MAX_COMPRESSION_RATIO {
        return err("implausible compression ratio");
    }
    if u64::from(hdr.uncompressed_size) > MAX_OUTPUT {
        return err("decoded size too large");
    }
    let mut buf = brotli_inflate(
        &data[hdr.compressed_offset..hdr.compressed_offset + hdr.compressed_length],
        hdr.uncompressed_size as usize,
    )?;

    let mut checksums: HashMap<(u32, u32), u32> = HashMap::new();
    for fi in 0..nfonts {
        let tables = &font_tables[fi];
        let (glyf_i, loca_i) = (
            find_tag(&hdr.tables, tables, TAG_GLYF),
            find_tag(&hdr.tables, tables, TAG_LOCA),
        );
        if glyf_i.is_some() != loca_i.is_some() {
            return err("glyf and loca must appear together");
        }
        if let (Some(g), Some(l)) = (glyf_i, loca_i) {
            if hdr.tables[g].transformed != hdr.tables[l].transformed {
                return err("glyf and loca must both be transformed or neither");
            }
            if !collection && g > l && hdr.tables[g].transformed {
                return err("loca before glyf in table directory");
            }
        }
        let mut font_sum = if collection {
            header_sums[fi]
        } else {
            overall_header_sum
        };
        let mut loca_sum = 0u32;
        for &ti in tables {
            let key = (hdr.tables[ti].tag, hdr.tables[ti].src_offset);
            let reused = checksums.contains_key(&key);
            if fi == 0 && reused {
                return err("table reused within first font");
            }
            let (tag, src_off, src_len) = {
                let t = &hdr.tables[ti];
                (t.tag, t.src_offset as usize, t.src_length as usize)
            };
            if src_off.checked_add(src_len).is_none_or(|e| e > buf.len()) {
                return err("table data out of range");
            }
            if tag == TAG_HHEA {
                let mut r = Reader::new(&buf[src_off..src_off + src_len]);
                r.skip(34)?;
                infos[fi].num_hmetrics = r.u16()?;
            }
            let sum;
            if reused {
                sum = checksums[&key];
            } else {
                if !hdr.tables[ti].transformed {
                    if tag == TAG_HEAD {
                        if src_len < 12 {
                            return err("head table too short");
                        }
                        buf[src_off + 8..src_off + 12].fill(0);
                    }
                    hdr.tables[ti].dst_offset = out.len() as u32;
                    sum = checksum(&buf[src_off..src_off + src_len]);
                    out.extend_from_slice(&buf[src_off..src_off + src_len]);
                } else if tag == TAG_GLYF {
                    hdr.tables[ti].dst_offset = out.len() as u32;
                    let li = loca_i.ok_or_else(|| Error("glyf without loca".into()))?;
                    let g = reconstruct_glyf(
                        &buf[src_off..src_off + src_len],
                        hdr.tables[li].dst_length,
                        &mut infos[fi],
                        &mut out,
                    )?;
                    hdr.tables[ti].dst_length = g.glyf_length;
                    hdr.tables[li].dst_offset = g.loca_offset;
                    hdr.tables[li].dst_length = g.loca_length;
                    loca_sum = g.loca_checksum;
                    sum = g.glyf_checksum;
                } else if tag == TAG_LOCA {
                    sum = loca_sum;
                } else if tag == TAG_HMTX {
                    hdr.tables[ti].dst_offset = out.len() as u32;
                    sum = reconstruct_hmtx(&buf[src_off..src_off + src_len], &infos[fi], &mut out)?;
                } else {
                    return err("unknown table transform");
                }
                checksums.insert(key, sum);
            }
            font_sum = font_sum.wrapping_add(sum);
            let t = &hdr.tables[ti];
            let entry_off = *infos[fi]
                .entry_by_tag
                .get(&tag)
                .ok_or_else(|| Error("table missing from directory".into()))?
                + 4;
            let mut entry = [0u8; 12];
            entry[0..4].copy_from_slice(&sum.to_be_bytes());
            entry[4..8].copy_from_slice(&t.dst_offset.to_be_bytes());
            entry[8..12].copy_from_slice(&t.dst_length.to_be_bytes());
            out[entry_off..entry_off + 12].copy_from_slice(&entry);
            font_sum = font_sum.wrapping_add(checksum(&entry));
            out.resize(round4(out.len() as u64) as usize, 0);
            if u64::from(t.dst_offset) + u64::from(t.dst_length) > out.len() as u64 {
                return err("table exceeds output");
            }
        }
        if let Some(h) = find_tag(&hdr.tables, tables, TAG_HEAD) {
            let t = &hdr.tables[h];
            if t.dst_length < 12 {
                return err("head table too short");
            }
            let adj = 0xB1B0_AFBAu32.wrapping_sub(font_sum);
            let at = t.dst_offset as usize + 8;
            out[at..at + 4].copy_from_slice(&adj.to_be_bytes());
        }
    }
    Ok(out)
}

fn find_tag(all: &[Table], idxs: &[usize], tag: u32) -> Option<usize> {
    idxs.iter().copied().find(|&i| all[i].tag == tag)
}

pub(crate) fn brotli_inflate(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut dec = brotli_decompressor::Decompressor::new(src, 4096).take(expected as u64 + 1);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|_| Error("brotli stream is corrupt".into()))?;
    if out.len() != expected {
        return err("brotli output size does not match table directory");
    }
    Ok(out)
}
