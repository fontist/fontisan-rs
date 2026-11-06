//! JSON renderings of the 1d-var digests, shared by the variation and WOFF read-parity tests.
#![allow(dead_code)]
use fontisan::sfnt::*;
use serde_json::{Value, json};

pub fn ver(v: (u16, u16)) -> Value {
    json!([v.0, v.1])
}

pub fn ivs(s: &IvsDigest) -> Value {
    json!({"region_count": s.region_count, "item_data_count": s.item_data_count,
           "regions_sha256": s.regions_sha256, "data_digest": s.data_digest,
           "data_sha256": s.data_sha256})
}

pub fn map(m: &Option<MapDigest>) -> Value {
    m.as_ref().map_or(Value::Null, |m| {
        json!({"format": m.format, "entry_format": m.entry_format,
               "map_count": m.map_count, "sha256": m.sha256})
    })
}

pub fn wrap<T>(r: Result<Option<T>, DigestFailure>, f: impl Fn(&T) -> Value) -> Value {
    match r {
        Ok(None) => Value::Null,
        Ok(Some(t)) => f(&t),
        Err(e) => json!({"error": e.ruby_name()}),
    }
}

pub fn axis_value(v: &AxisValue) -> Value {
    match v {
        AxisValue::Fixed {
            format,
            axis_index,
            flags,
            value_name_id,
            values,
        } => {
            let mut o = json!({"format": format, "axis_index": axis_index, "flags": flags,
                               "value_name_id": value_name_id});
            for (n, raw) in values {
                o[format!("{n}_raw")] = json!(raw);
            }
            o
        }
        AxisValue::Multi {
            flags,
            value_name_id,
            axis_values,
        } => json!({"format": 4, "flags": flags, "value_name_id": value_name_id,
            "axis_values": axis_values.iter()
                .map(|(i, v)| json!({"axis_index": i, "value": v})).collect::<Vec<_>>()}),
        AxisValue::Unsupported { format } => json!({"format": format, "unsupported": true}),
    }
}

pub fn variation(f: &Font) -> Value {
    json!({
        "gvar": wrap(f.gvar_digest(), |g| json!({
            "version": ver(g.version), "axis_count": g.axis_count, "flags": g.flags,
            "glyph_count": g.glyph_count, "glyphs_with_data": g.glyphs_with_data,
            "glyphs_digest": g.glyphs_digest, "shared_tuple_count": g.shared_tuple_count,
            "shared_tuples_sha256": g.shared_tuples_sha256})),
        "HVAR": wrap(f.hvar_digest(), hvar),
        "VVAR": wrap(f.vvar_digest(), hvar),
        "MVAR": wrap(f.mvar_digest(), |m| json!({
            "version": ver(m.version),
            "value_records": m.value_records.iter()
                .map(|r| json!({"tag": r.tag, "outer": r.outer, "inner": r.inner}))
                .collect::<Vec<_>>(),
            "item_variation_store": m.item_variation_store.as_ref().map_or(Value::Null, ivs)})),
        "STAT": wrap(f.stat_digest(), |s| json!({
            "version": ver(s.version), "elided_fallback_name_id": s.elided_fallback_name_id,
            "axes": s.axes.iter()
                .map(|a| json!({"tag": a.tag, "name_id": a.name_id, "ordering": a.ordering}))
                .collect::<Vec<_>>(),
            "axis_values": s.axis_values.iter().map(axis_value).collect::<Vec<_>>()})),
    })
}

pub fn hvar(h: &HvarDigest) -> Value {
    json!({"version": ver(h.version), "item_variation_store": ivs(&h.item_variation_store),
           "advance_mapping": map(&h.advance_mapping), "lsb_mapping": map(&h.lsb_mapping),
           "rsb_mapping": map(&h.rsb_mapping)})
}

pub fn fvar(f: &Font) -> Value {
    match f.fvar() {
        Err(e) => json!({"error": e.class.ruby_name()}),
        Ok(None) => Value::Null,
        Ok(Some(t)) => json!({
            "axes": t.axes.iter().map(|a| json!({
                "axis_tag": a.tag, "axis_name_id": a.name_id, "flags": a.flags,
                "min_value_raw": a.min_value_raw, "default_value_raw": a.default_value_raw,
                "max_value_raw": a.max_value_raw})).collect::<Vec<_>>(),
            "instances": t.instances.iter().map(|i| json!({
                "name_id": i.name_id, "flags": i.flags,
                "postscript_name_id": i.postscript_name_id,
                "coordinates_raw": i.coordinates_raw})).collect::<Vec<_>>()}),
    }
}
