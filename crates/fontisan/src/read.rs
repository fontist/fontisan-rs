//! `read`: the facade's `Reference.read` document for every face of a font file
//! (table directory, names, header fields, cmap / post / outline / layout /
//! variation digests), rendered as canonical JSON. A failure is the first
//! error Ruby's `read` would raise ([`error_json`] renders it).
use crate::json::{Json, obj};
use crate::sfnt::{
    AxisValue, DigestFailure, Error, ErrorClass, Font, GlyphDigests, HvarDigest, IvsDigest,
    LayoutTable, MapDigest, Options, Tag, Value, face_count,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn sha_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Digest of the whole file. Hashing the file is the costly step of `read` on a
/// large collection, so it runs once per file, never once per face.
fn file_digest(bytes: &[u8]) -> String {
    #[cfg(test)]
    FILE_HASHES.with(|c| c.set(c.get() + 1));
    sha_hex(bytes)
}

/// SHA-256 of table data, memoised by the table's location in the file. Faces of a
/// collection share tables (same offset and length), so each shared table is hashed
/// once per read rather than once per face.
#[derive(Default)]
struct TableDigests(
    std::collections::HashMap<(usize, usize), String>,
    GlyphDigests,
);

impl TableDigests {
    fn hex(&mut self, data: &[u8]) -> String {
        // `data` always borrows from the one input buffer, so address+length is its location.
        self.0
            .entry((data.as_ptr() as usize, data.len()))
            .or_insert_with(|| {
                #[cfg(test)]
                TABLE_BYTES.with(|c| c.set(c.get() + data.len()));
                sha_hex(data)
            })
            .clone()
    }
}

#[cfg(test)]
thread_local! {
    static TABLE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    static FILE_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `{class, message, causes: [{class, message}]}`: the error surface the 0e
/// malformed goldens record (`error_class` is `class` here).
pub fn error_json(e: &Error) -> Json {
    obj! {
        "class" => e.class.ruby_name(),
        "message" => e.message.clone(),
        "causes" => e.cause.iter()
            .map(|(c, m)| obj! {"class" => c.ruby_name(), "message" => m.clone()})
            .collect::<Vec<_>>(),
    }
}

/// A table that is gated out by the mode reads as absent, like the facade's `available_table`.
fn gated<T>(r: Result<Option<T>, Error>) -> Result<Option<T>, Error> {
    match r {
        Err(e) if e.class == ErrorClass::Argument => Ok(None),
        other => other,
    }
}

fn snapshot(s: &[(&'static str, Value)]) -> Json {
    fn v(x: &Value) -> Json {
        match x {
            Value::Int(i) => Json::Int(*i),
            Value::Text(t) => Json::Str(t.clone()),
            Value::List(l) => Json::Arr(l.iter().map(v).collect()),
        }
    }
    Json::Map(s.iter().map(|(k, x)| (k.to_string(), v(x))).collect())
}

fn tag(t: &Tag) -> String {
    String::from_utf8_lossy(t).into_owned()
}

fn layout_value(r: Result<Option<LayoutTable>, Error>) -> Json {
    match r {
        Ok(Some(t)) => {
            let per_script = t.script_features();
            let scripts: Vec<Tag> = per_script.iter().map(|(s, _)| *s).collect();
            let features: BTreeMap<String, Vec<String>> = per_script
                .iter()
                .map(|(s, f)| (tag(s), f.iter().map(tag).collect()))
                .collect();
            obj! {
                "scripts" => scripts.iter().map(tag).collect::<Vec<_>>(),
                "features" => Json::Map(features.into_iter().map(|(k, v)| (k, v.into())).collect()),
            }
        }
        _ => Json::Null,
    }
}

fn ver(v: (u16, u16)) -> Json {
    Json::Arr(vec![v.0.into(), v.1.into()])
}

fn ivs(s: &IvsDigest) -> Json {
    obj! {"region_count" => s.region_count, "item_data_count" => s.item_data_count,
    "regions_sha256" => s.regions_sha256.clone(), "data_digest" => s.data_digest.clone(),
    "data_sha256" => s.data_sha256.clone()}
}

fn map(m: &Option<MapDigest>) -> Json {
    m.as_ref().map_or(Json::Null, |m| {
        obj! {"format" => m.format, "entry_format" => m.entry_format,
        "map_count" => m.map_count, "sha256" => m.sha256.clone()}
    })
}

fn hvar(h: &HvarDigest) -> Json {
    obj! {"version" => ver(h.version), "item_variation_store" => ivs(&h.item_variation_store),
    "advance_mapping" => map(&h.advance_mapping), "lsb_mapping" => map(&h.lsb_mapping),
    "rsb_mapping" => map(&h.rsb_mapping)}
}

fn wrap<T>(r: Result<Option<T>, DigestFailure>, f: impl Fn(&T) -> Json) -> Json {
    match r {
        Ok(None) => Json::Null,
        Ok(Some(t)) => f(&t),
        Err(e) => obj! {"error" => e.ruby_name()},
    }
}

fn axis_value(v: &AxisValue) -> Json {
    match v {
        AxisValue::Fixed {
            format,
            axis_index,
            flags,
            value_name_id,
            values,
        } => {
            let mut o = vec![
                ("format".to_string(), Json::from(*format)),
                ("axis_index".into(), (*axis_index).into()),
                ("flags".into(), (*flags).into()),
                ("value_name_id".into(), (*value_name_id).into()),
            ];
            for (n, raw) in values {
                o.push((format!("{n}_raw"), (*raw).into()));
            }
            Json::Map(o)
        }
        AxisValue::Multi {
            flags,
            value_name_id,
            axis_values,
        } => obj! {"format" => 4, "flags" => *flags, "value_name_id" => *value_name_id,
        "axis_values" => axis_values.iter()
            .map(|(i, v)| obj! {"axis_index" => *i, "value" => *v}).collect::<Vec<_>>()},
        AxisValue::Unsupported { format } => obj! {"format" => *format, "unsupported" => true},
    }
}

fn variation(f: &Font) -> Json {
    obj! {
        "gvar" => wrap(f.gvar_digest(), |g| obj! {
            "version" => ver(g.version), "axis_count" => g.axis_count, "flags" => g.flags,
            "glyph_count" => g.glyph_count, "glyphs_with_data" => g.glyphs_with_data,
            "glyphs_digest" => g.glyphs_digest.clone(), "shared_tuple_count" => g.shared_tuple_count,
            "shared_tuples_sha256" => g.shared_tuples_sha256.clone()}),
        "HVAR" => wrap(f.hvar_digest(), hvar),
        "VVAR" => wrap(f.vvar_digest(), hvar),
        "MVAR" => wrap(f.mvar_digest(), |m| obj! {
            "version" => ver(m.version),
            "value_records" => m.value_records.iter()
                .map(|r| obj! {"tag" => r.tag.clone(), "outer" => r.outer, "inner" => r.inner})
                .collect::<Vec<_>>(),
            "item_variation_store" => m.item_variation_store.as_ref().map_or(Json::Null, ivs)}),
        "STAT" => wrap(f.stat_digest(), |s| obj! {
            "version" => ver(s.version), "elided_fallback_name_id" => s.elided_fallback_name_id,
            "axes" => s.axes.iter()
                .map(|a| obj! {"tag" => a.tag.clone(), "name_id" => a.name_id, "ordering" => a.ordering})
                .collect::<Vec<_>>(),
            "axis_values" => s.axis_values.iter().map(axis_value).collect::<Vec<_>>()}),
    }
}

fn fvar(f: &Font) -> Json {
    match f.fvar() {
        Err(e) => obj! {"error" => e.class.ruby_name()},
        Ok(None) => Json::Null,
        Ok(Some(t)) => obj! {
            "axes" => t.axes.iter().map(|a| obj! {
                "axis_tag" => a.tag.clone(), "axis_name_id" => a.name_id, "flags" => a.flags,
                "min_value_raw" => a.min_value_raw, "default_value_raw" => a.default_value_raw,
                "max_value_raw" => a.max_value_raw}).collect::<Vec<_>>(),
            "instances" => t.instances.iter().map(|i| obj! {
                "name_id" => i.name_id, "flags" => i.flags,
                "postscript_name_id" => i.postscript_name_id,
                "coordinates_raw" => i.coordinates_raw.clone()}).collect::<Vec<_>>(),
        },
    }
}

/// Digest of the decoded per-glyph horizontal metrics, as the facade's
/// `hmtx_metrics`: SHA-256 over every glyph's advance (u32 BE) then lsb (i32 BE).
/// `null` when `hmtx`/`hhea`/`maxp` is absent or gated out by the mode, `{error}`
/// when decoding fails (numberOfHMetrics 0 or above numGlyphs, short table).
fn hmtx_metrics(font: &Font) -> Json {
    fn err(e: &Error) -> Json {
        obj! {"error" => e.class.ruby_name()}
    }
    let hmtx = match font.available("hmtx") {
        Ok(Some(Some(d))) => d,
        Ok(_) | Err(_) => return Json::Null,
    };
    let (hhea, maxp) = match (gated(font.hhea()), gated(font.maxp())) {
        (Err(e), _) | (_, Err(e)) => return err(&e),
        (Ok(Some(h)), Ok(Some(m))) => (h, m),
        _ => return Json::Null,
    };
    let (nhm, glyphs) = (
        usize::from(hhea.number_of_h_metrics),
        usize::from(maxp.num_glyphs),
    );
    if nhm < 1 || glyphs < 1 || nhm > glyphs {
        return err(&Error::new(
            ErrorClass::Argument,
            "invalid hmtx context".to_string(),
        ));
    }
    let need = nhm * 4 + (glyphs - nhm) * 2;
    if hmtx.len() < need {
        return err(&Error::new(
            ErrorClass::CorruptedTable,
            "short hmtx".to_string(),
        ));
    }
    let mut packed = Vec::with_capacity(glyphs * 8);
    let mut advance = 0u32;
    for gid in 0..glyphs {
        let lsb = if gid < nhm {
            advance = u32::from(u16::from_be_bytes([hmtx[gid * 4], hmtx[gid * 4 + 1]]));
            i16::from_be_bytes([hmtx[gid * 4 + 2], hmtx[gid * 4 + 3]])
        } else {
            let o = nhm * 4 + (gid - nhm) * 2;
            i16::from_be_bytes([hmtx[o], hmtx[o + 1]])
        };
        packed.extend_from_slice(&advance.to_be_bytes());
        packed.extend_from_slice(&i32::from(lsb).to_be_bytes());
    }
    obj! {"count" => glyphs, "sha256" => sha_hex(&packed)}
}

/// One face of `bytes` as the facade's read entry, or the first error `read` raises.
pub fn read_face(bytes: &[u8], font: &Font, index: usize) -> Result<Json, Error> {
    read_face_hashed(
        &file_digest(bytes),
        &mut TableDigests::default(),
        font,
        index,
    )
}

/// [`read_face`] with the whole-file digest supplied, so a collection hashes the
/// file once rather than once per face.
fn read_face_hashed(
    file_sha256: &str,
    digests: &mut TableDigests,
    font: &Font,
    index: usize,
) -> Result<Json, Error> {
    let tables: Vec<Json> = font
        .table_summaries()
        .into_iter()
        .map(|t| {
            let sha = t
                .length
                .and_then(|_| font.table_data(&t.tag))
                .map(|d| digests.hex(d));
            obj! {"tag" => t.tag, "length" => t.length, "sha256" => sha,
            "error" => t.error.map(|c| c.ruby_name())}
        })
        .collect();
    let names: Vec<Json> = match gated(font.name())? {
        None => vec![],
        Some(n) => n
            .records
            .iter()
            .map(|r| {
                obj! {"platform_id" => r.platform_id, "encoding_id" => r.encoding_id,
                "language_id" => r.language_id, "name_id" => r.name_id,
                "string" => n.decode(r)}
            })
            .collect(),
    };
    let snap = |s: Option<Vec<(&'static str, Value)>>| s.map_or(Json::Null, |s| snapshot(&s));
    let header_fields = obj! {
        "head" => snap(gated(font.head())?.map(|t| t.snapshot())),
        "hhea" => snap(gated(font.hhea())?.map(|t| t.snapshot())),
        "maxp" => snap(gated(font.maxp())?.map(|t| t.snapshot())),
        "OS/2" => snap(gated(font.os2())?.map(|t| t.snapshot())),
    };
    let cmap = match gated(font.cmap())? {
        None => Json::Null,
        Some(c) => {
            let cps = c.codepoints();
            let be: Vec<u8> = cps.iter().flat_map(|c| c.to_be_bytes()).collect();
            obj! {"count" => cps.len(), "sha256" => sha_hex(&be),
            "subtable_formats" => c.subtable_formats()}
        }
    };
    let post = match gated(font.post())? {
        None => Json::Null,
        Some(p) => {
            let g = p.glyph_names();
            obj! {"glyph_name_count" => g.len(), "glyph_names_sha256" => sha_hex(g.join("\n").as_bytes()),
            "version_raw" => p.version_raw, "italic_angle_raw" => p.italic_angle_raw,
            "underline_position" => p.underline_position,
            "underline_thickness" => p.underline_thickness,
            "is_fixed_pitch" => p.is_fixed_pitch,
            "min_mem_type42" => p.min_mem_type42, "max_mem_type42" => p.max_mem_type42,
            "min_mem_type1" => p.min_mem_type1, "max_mem_type1" => p.max_mem_type1}
        }
    };
    let o = font.outline_summary_with(&mut digests.1);
    let mut outlines = vec![
        ("kind", Json::from(o.kind.map(|k| k.as_str()))),
        ("count", o.count.into()),
        ("digest", o.digest.into()),
    ];
    if let Some(e) = o.error {
        outlines.push(("error", e.ruby_name().into()));
    }
    Ok(obj! {
        "format" => font.format().as_str(),
        "file_sha256" => file_sha256,
        "index" => index,
        "mode" => font.mode().as_str(),
        "lazy" => font.lazy(),
        "tables" => tables,
        "names" => names,
        "header_fields" => header_fields,
        "cmap" => cmap,
        "post" => post,
        "fvar" => fvar(font),
        "variation" => variation(font),
        "outlines" => Json::Obj(outlines),
        "hmtx_metrics" => hmtx_metrics(font),
        "layout" => obj! {"GSUB" => layout_value(font.gsub()), "GPOS" => layout_value(font.gpos())},
    })
}

/// `Reference.read(bytes, mode:, lazy:)`: one entry per face (every collection
/// member), or the first error.
pub fn read(bytes: &[u8], options: Options) -> Result<Vec<Json>, Error> {
    let faces = face_count(bytes).unwrap_or(1);
    let file_sha256 = file_digest(bytes);
    let mut digests = TableDigests::default();
    (0..faces)
        .map(|i| {
            let font = Font::load(bytes, i, options)?;
            read_face_hashed(&file_sha256, &mut digests, &font, i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `faces` empty-directory sfnt faces in one collection.
    fn collection(faces: u32) -> Vec<u8> {
        let header = 12 + 4 * faces as usize;
        let mut b = b"ttcf\0\x01\0\0".to_vec();
        b.extend_from_slice(&faces.to_be_bytes());
        for i in 0..faces {
            b.extend_from_slice(&((header + 12 * i as usize) as u32).to_be_bytes());
        }
        for _ in 0..faces {
            b.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        b
    }

    #[test]
    fn a_collection_hashes_the_whole_file_once_and_every_face_carries_it() {
        let bytes = collection(3);
        FILE_HASHES.with(|c| c.set(0));
        let faces = read(&bytes, Options::default()).expect("read");
        assert_eq!(faces.len(), 3);
        assert_eq!(FILE_HASHES.with(|c| c.get()), 1, "file hashed per face");
        let want = sha_hex(&bytes);
        for face in &faces {
            let Json::Obj(m) = face else {
                panic!("not an object")
            };
            let got = m.iter().find(|(k, _)| *k == "file_sha256").map(|(_, v)| v);
            assert!(matches!(got, Some(Json::Str(s)) if *s == want), "{got:?}");
        }
    }

    /// `faces` faces whose directories all point one `DATA` table at the same bytes.
    fn shared_table_collection(faces: u32, data: &[u8]) -> Vec<u8> {
        let header = 12 + 4 * faces as usize;
        let face_len = 12 + 16;
        let data_at = header + face_len * faces as usize;
        let mut b = b"ttcf\0\x01\0\0".to_vec();
        b.extend_from_slice(&faces.to_be_bytes());
        for i in 0..faces as usize {
            b.extend_from_slice(&((header + face_len * i) as u32).to_be_bytes());
        }
        for _ in 0..faces {
            b.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
            b.extend_from_slice(b"DATA\0\0\0\0");
            b.extend_from_slice(&(data_at as u32).to_be_bytes());
            b.extend_from_slice(&(data.len() as u32).to_be_bytes());
        }
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn a_table_shared_by_faces_is_hashed_once_with_the_same_digest() {
        let data: Vec<u8> = (0..200u8).collect();
        let bytes = shared_table_collection(4, &data);
        TABLE_BYTES.with(|c| c.set(0));
        let faces = read(&bytes, Options::default()).expect("read");
        assert_eq!(faces.len(), 4);
        assert_eq!(
            TABLE_BYTES.with(|c| c.get()),
            data.len(),
            "shared table hashed per face"
        );
        let want = sha_hex(&data);
        for face in &faces {
            let rendered = format!("{face:?}");
            assert!(
                rendered.contains(&want),
                "face lacks the table digest {want}: {rendered}"
            );
        }
    }

    /// A TrueType sfnt holding only `hhea`, `maxp` and `hmtx`.
    fn metrics_font(nhm: u16, glyphs: u16, hmtx: &[u8]) -> Vec<u8> {
        let mut hhea = vec![0u8; 36];
        hhea[34..36].copy_from_slice(&nhm.to_be_bytes());
        let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
        maxp.extend_from_slice(&glyphs.to_be_bytes());
        let tables: [(&[u8; 4], &[u8]); 3] = [(b"hhea", &hhea), (b"hmtx", hmtx), (b"maxp", &maxp)];
        let mut b = vec![0, 1, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0];
        let mut at = 12 + 16 * tables.len();
        for (tag, d) in &tables {
            b.extend_from_slice(*tag);
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&(at as u32).to_be_bytes());
            b.extend_from_slice(&(d.len() as u32).to_be_bytes());
            at += d.len();
        }
        for (_, d) in &tables {
            b.extend_from_slice(d);
        }
        b
    }

    fn metrics_of(bytes: &[u8], options: Options) -> String {
        let faces = read(bytes, options).expect("read");
        let Json::Obj(m) = &faces[0] else {
            panic!("not an object")
        };
        let (_, v) = m.iter().find(|(k, _)| *k == "hmtx_metrics").expect("key");
        format!("{v:?}")
    }

    #[test]
    fn hmtx_metrics_digests_advance_u32_then_lsb_i32_with_the_last_advance_repeated() {
        // 2 hMetrics (advance 500/lsb -3, advance 700/lsb 40), then lsb -5 and 6 for gids 2, 3.
        let hmtx = [0x01, 0xF4, 0xFF, 0xFD, 0x02, 0xBC, 0, 40, 0xFF, 0xFB, 0, 6];
        let mut want = Vec::new();
        for (adv, lsb) in [(500u32, -3i32), (700, 40), (700, -5), (700, 6)] {
            want.extend_from_slice(&adv.to_be_bytes());
            want.extend_from_slice(&lsb.to_be_bytes());
        }
        let got = metrics_of(&metrics_font(2, 4, &hmtx), Options::default());
        assert!(got.contains(&sha_hex(&want)) && got.contains('4'), "{got}");
    }

    #[test]
    fn hmtx_metrics_reports_the_error_class_ruby_raises() {
        let bad_nhm = metrics_of(&metrics_font(0, 2, &[0; 8]), Options::default());
        assert!(bad_nhm.contains("ArgumentError"), "{bad_nhm}");
        let too_many = metrics_of(&metrics_font(3, 2, &[0; 12]), Options::default());
        assert!(too_many.contains("ArgumentError"), "{too_many}");
        let short = metrics_of(&metrics_font(2, 4, &[0; 11]), Options::default());
        assert!(short.contains("Fontisan::CorruptedTableError"), "{short}");
    }

    #[test]
    fn hmtx_metrics_is_null_in_metadata_mode_and_without_hmtx() {
        let font = metrics_font(1, 1, &[0; 4]);
        let meta = Options {
            mode: crate::sfnt::LoadingMode::Metadata,
            ..Options::default()
        };
        assert_eq!(metrics_of(&font, meta), "Null");
        let no_hmtx = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(metrics_of(&no_hmtx, Options::default()), "Null");
    }
}
